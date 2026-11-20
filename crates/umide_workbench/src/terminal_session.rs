//! A workspace shell on a native PTY, with Alacritty's terminal emulation.
use std::{
    path::Path,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
};

use alacritty_terminal::{
    Term,
    event::{Event, EventListener, WindowSize},
    event_loop::{EventLoop, EventLoopSender, Msg},
    grid::{Dimensions, Scroll},
    sync::FairMutex,
    term::{Config, TermMode, cell::Cell, color::Colors},
    tty::{self, Options, Shell},
    vte::ansi::CursorShape,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalStatus {
    Running,
    Exited(Option<i32>),
}

#[derive(Clone)]
pub struct TerminalSnapshot {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
    pub cursor: Option<(usize, usize, CursorShape)>,
    pub mode: TermMode,
    pub colors: Colors,
    pub status: TerminalStatus,
}

#[derive(Clone, Copy)]
struct Size(WindowSize);

impl Size {
    fn new(cols: u16, rows: u16, width: u16, height: u16) -> Self {
        let cols = cols.clamp(2, 512);
        let rows = rows.clamp(1, 256);
        Self(WindowSize {
            num_cols: cols,
            num_lines: rows,
            cell_width: width.clamp(1, u16::MAX / cols),
            cell_height: height.clamp(1, u16::MAX / rows),
        })
    }
}

impl Dimensions for Size {
    fn columns(&self) -> usize {
        self.0.num_cols.into()
    }
    fn screen_lines(&self) -> usize {
        self.0.num_lines.into()
    }
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }
}

struct Events {
    revision: AtomicU64,
    channel: OnceLock<EventLoopSender>,
    size: Mutex<Size>,
    status: Mutex<TerminalStatus>,
}

impl Events {
    fn update(&self) {
        self.revision.fetch_add(1, Ordering::Release);
    }

    fn send(&self, message: Msg) {
        if let Some(channel) = self.channel.get() {
            let _ = channel.send(message);
        }
    }
}

#[derive(Clone)]
struct Listener(Arc<Events>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => {
                self.0.send(Msg::Input(text.into_bytes().into()))
            }
            Event::TextAreaSizeRequest(format) => {
                let size = self.0.size.lock().unwrap().0;
                self.0.send(Msg::Input(format(size).into_bytes().into()));
            }
            Event::ChildExit(code) => {
                *self.0.status.lock().unwrap() = TerminalStatus::Exited(Some(code));
                self.0.update();
            }
            Event::Wakeup | Event::CursorBlinkingChange => self.0.update(),
            _ => {}
        }
    }
}

struct ShellProcess {
    #[cfg(unix)]
    pid: i32,
    #[cfg(unix)]
    terminal: std::fs::File,
    #[cfg(windows)]
    pid: Option<u32>,
    closed: OnceLock<()>,
}

impl ShellProcess {
    fn close(&self) {
        self.closed.get_or_init(|| {
            #[cfg(unix)]
            {
                use std::os::fd::AsRawFd;
                // tty::new calls setsid() in the child. Interactive shells
                // also give foreground and background jobs separate groups.
                let fd = self.terminal.as_raw_fd();
                let tty_session = unsafe { libc::tcgetsid(fd) };
                let leader_session = unsafe { libc::getsid(self.pid) };
                // A natural shell exit can dissociate the tty before the PTY
                // loop returns. A live PID at that point may have been reused.
                // Remaining original jobs keep their session ID reserved.
                if tty_session != self.pid && leader_session >= 0 {
                    return;
                }
                if tty_session == self.pid {
                    let fg = unsafe { libc::tcgetpgrp(fd) };
                    if fg > 0 && unsafe { libc::getsid(fg) } == self.pid {
                        unsafe {
                            libc::kill(-fg, libc::SIGKILL);
                        }
                    }
                    if leader_session == self.pid {
                        unsafe {
                            libc::kill(-self.pid, libc::SIGKILL);
                        }
                    }
                }
                for pid in process_ids() {
                    if pid > 0 && unsafe { libc::getsid(pid) } == self.pid {
                        unsafe {
                            libc::kill(pid, libc::SIGKILL);
                        }
                    }
                }
            }
            #[cfg(windows)]
            if let Some(pid) = self.pid {
                use std::process::{Command, Stdio};
                let _ = Command::new("taskkill")
                    .args(["/T", "/F", "/PID", &pid.to_string()])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        });
    }
}

#[cfg(target_os = "macos")]
fn process_ids() -> Vec<i32> {
    // libproc returns a count of pid_t values, but takes its capacity in bytes.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return Vec::new();
    }
    let mut ids = vec![0i32; count as usize + 128];
    let bytes = (ids.len() * size_of::<i32>()) as i32;
    let actual = unsafe { libc::proc_listallpids(ids.as_mut_ptr().cast(), bytes) };
    ids.truncate(actual.max(0) as usize);
    ids
}

#[cfg(all(unix, not(target_os = "macos")))]
fn process_ids() -> Vec<i32> {
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .collect()
}

pub struct TerminalSession {
    term: Arc<FairMutex<Term<Listener>>>,
    events: Arc<Events>,
    process: Arc<ShellProcess>,
    reaper: Mutex<Option<JoinHandle<()>>>,
}

impl TerminalSession {
    pub fn start(root: &Path, cols: u16, rows: u16) -> Result<Self, String> {
        #[cfg(unix)]
        let shell = {
            let fallback = if cfg!(target_os = "macos") {
                "/bin/zsh"
            } else {
                "/bin/sh"
            };
            let program = std::env::var("SHELL")
                .ok()
                .filter(|s| Path::new(s).is_absolute() && Path::new(s).is_file())
                .unwrap_or_else(|| fallback.to_owned());
            Some(Shell::new(program, vec!["-l".into()]))
        };
        #[cfg(windows)]
        let shell = None;
        Self::open(root, Size::new(cols, rows, 8, 16), shell)
    }

    fn open(root: &Path, size: Size, shell: Option<Shell>) -> Result<Self, String> {
        let root = root
            .canonicalize()
            .map_err(|e| format!("Cannot open the terminal directory: {e}"))?;
        let options = Options {
            working_directory: Some(root),
            shell,
            env: [
                ("TERM".into(), "xterm-256color".into()),
                ("COLORTERM".into(), "truecolor".into()),
            ]
            .into(),
            ..Options::default()
        };
        let events = Arc::new(Events {
            revision: AtomicU64::new(1),
            channel: OnceLock::new(),
            size: Mutex::new(size),
            status: Mutex::new(TerminalStatus::Running),
        });
        let listener = Listener(events.clone());
        let term = Arc::new(FairMutex::new(Term::new(
            Config::default(),
            &size,
            listener.clone(),
        )));
        let pty = tty::new(&options, size.0, 0)
            .map_err(|e| format!("Cannot start the terminal: {e}"))?;
        let process = Arc::new(ShellProcess {
            #[cfg(unix)]
            pid: pty.child().id() as i32,
            #[cfg(unix)]
            terminal: pty.file().try_clone().map_err(|e| e.to_string())?,
            #[cfg(windows)]
            pid: pty.child_watcher().pid().map(|p| p.get()),
            closed: OnceLock::new(),
        });
        // Hold until the PTY has drained so the last output survives shell exit.
        let io = EventLoop::new(term.clone(), listener, pty, true, false)
            .map_err(|e| e.to_string())?;
        let _ = events.channel.set(io.channel());
        let worker = io.spawn();
        let notify = events.clone();
        let cleanup = process.clone();
        let reaper = thread::spawn(move || {
            let result = worker.join();
            cleanup.close();
            drop(result); // The PTY's destructor waits for the shell.
            let mut status = notify.status.lock().unwrap();
            if *status == TerminalStatus::Running {
                *status = TerminalStatus::Exited(None);
            }
            notify.update();
        });
        Ok(Self {
            term,
            events,
            process,
            reaper: Mutex::new(Some(reaper)),
        })
    }

    pub fn revision(&self) -> u64 {
        self.events.revision.load(Ordering::Acquire)
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        let term = self.term.lock();
        let content = term.renderable_content();
        let cursor = content.cursor;
        let line = cursor.point.line.0 + content.display_offset as i32;
        let visible = cursor.shape != CursorShape::Hidden
            && (0..term.screen_lines() as i32).contains(&line);
        TerminalSnapshot {
            cols: term.columns(),
            rows: term.screen_lines(),
            cells: content.display_iter.map(|p| p.cell.clone()).collect(),
            cursor: visible.then_some((
                cursor.point.column.0,
                line as usize,
                cursor.shape,
            )),
            mode: content.mode,
            colors: *content.colors,
            status: *self.events.status.lock().unwrap(),
        }
    }

    pub fn write(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        {
            let mut term = self.term.lock();
            if term.grid().display_offset() != 0 {
                term.scroll_display(Scroll::Bottom);
                self.events.update();
            }
        }
        self.events.send(Msg::Input(bytes.into()));
    }

    pub fn resize(&self, cols: u16, rows: u16, width: u16, height: u16) {
        let size = Size::new(cols, rows, width, height);
        // The terminal can emit a size query while processing a control
        // sequence. Never lock the grid with the shared size lock held.
        {
            let mut current = self.events.size.lock().unwrap();
            let a = current.0;
            let b = size.0;
            if (a.num_cols, a.num_lines, a.cell_width, a.cell_height)
                == (b.num_cols, b.num_lines, b.cell_width, b.cell_height)
            {
                return;
            }
            *current = size;
        }
        self.term.lock().resize(size);
        self.events.send(Msg::Resize(size.0));
        self.events.update();
    }

    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
        self.events.update();
    }

    pub fn close(&self) {
        self.process.close();
        self.events.send(Msg::Shutdown);
        if let Some(reaper) = self.reaper.lock().unwrap().take() {
            let _ = reaper.join();
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use alacritty_terminal::{
        term::cell::Flags,
        vte::ansi::{Color, NamedColor},
    };
    use std::{
        fs,
        time::{Duration, Instant},
    };

    fn shell(root: &Path) -> TerminalSession {
        TerminalSession::open(
            root,
            Size::new(40, 8, 8, 16),
            Some(Shell::new("/bin/sh".into(), vec!["-i".into()])),
        )
        .unwrap()
    }

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "Timed out waiting for fixture shell"
            );
            thread::sleep(Duration::from_millis(15));
        }
    }

    #[test]
    fn real_shell_draws_ansi_and_unicode_and_reports_new_size() {
        let dir = tempfile::tempdir().unwrap();
        let term = shell(dir.path());
        term.write(b"stty -echo; pwd -P > where; printf '\\033[2J\\033[H\\033[1;31mRED\\033[0m\\r\\n'\r".to_vec());
        term.write("printf '界e\u{301}\\r\\n'\r".as_bytes().to_vec());
        wait_until(|| {
            let frame = term.snapshot();
            frame
                .cells
                .iter()
                .any(|c| c.c == 'R' && c.flags.contains(Flags::BOLD))
                && frame
                    .cells
                    .iter()
                    .any(|c| c.zerowidth() == Some(&['\u{301}'][..]))
        });
        let view = term.snapshot();
        assert_eq!(view.cells.len(), view.cols * view.rows);
        assert!(view.cells.iter().any(|c| c.c == 'R'
            && c.fg == Color::Named(NamedColor::Red)
            && c.flags.contains(Flags::BOLD)));
        assert!(
            view.cells
                .iter()
                .any(|c| c.c == '界' && c.flags.contains(Flags::WIDE_CHAR))
        );
        assert!(
            view.cells
                .iter()
                .any(|c| c.c == 'e' && c.zerowidth() == Some(&['\u{301}'][..]))
        );
        assert!(view.cursor.is_some());
        assert_eq!(
            Path::new(fs::read_to_string(dir.path().join("where")).unwrap().trim()),
            dir.path().canonicalize().unwrap()
        );
        term.resize(72, 13, 8, 16);
        term.write(b"stty size > size\r".to_vec());
        wait_until(|| {
            fs::read_to_string(dir.path().join("size"))
                .is_ok_and(|s| s.trim() == "13 72")
        });
        let view = term.snapshot();
        assert_eq!((view.cols, view.rows, view.cells.len()), (72, 13, 72 * 13));
        term.write(b"exit 7\r".to_vec());
        wait_until(|| term.snapshot().status == TerminalStatus::Exited(Some(7)));
    }

    #[test]
    fn ctrl_c_interrupts_foreground_program_and_shell_accepts_more_input() {
        let dir = tempfile::tempdir().unwrap();
        let term = shell(dir.path());
        term.write(
            b"/bin/sh -c 'echo running > busy; exec /bin/sleep 20'\r".to_vec(),
        );
        wait_until(|| {
            fs::read_to_string(dir.path().join("busy"))
                .is_ok_and(|s| s.trim() == "running")
        });
        term.write(vec![0x03]);
        term.write(b"echo resumed > result\r".to_vec());
        wait_until(|| {
            fs::read_to_string(dir.path().join("result"))
                .is_ok_and(|s| s.trim() == "resumed")
        });
        assert_eq!(term.snapshot().status, TerminalStatus::Running);
    }

    #[test]
    fn natural_exit_reaps_jobs_before_the_closed_session_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let term = shell(dir.path());
        term.write(b"/bin/sh -c 'trap \"\" HUP; echo started > job; /bin/sleep 1; echo leak > leaked' </dev/null >/dev/null 2>&1 &\r".to_vec());
        wait_until(|| dir.path().join("job").exists());
        term.write(b"exit 0\r".to_vec());
        wait_until(|| term.reaper.lock().unwrap().as_ref().unwrap().is_finished());
        assert_eq!(term.snapshot().status, TerminalStatus::Exited(Some(0)));
        let next = shell(dir.path());
        term.close();
        term.close();
        next.write(b"echo alive > next\r".to_vec());
        wait_until(|| dir.path().join("next").exists());
        thread::sleep(Duration::from_millis(1050));
        assert!(!dir.path().join("leaked").exists());
    }

    #[test]
    fn scrolls_into_history_and_shutdown_cleans_background_job_groups() {
        let dir = tempfile::tempdir().unwrap();
        let term = shell(dir.path());
        term.write(b"stty -echo; i=0; while [ $i -lt 30 ]; do echo marker-$i; i=$((i+1)); done; echo yes > lines\r".to_vec());
        wait_until(|| dir.path().join("lines").is_file());
        let rendered =
            |s: TerminalSnapshot| s.cells.iter().map(|c| c.c).collect::<String>();
        wait_until(|| rendered(term.snapshot()).contains("marker-29"));
        term.scroll(7);
        assert!(!rendered(term.snapshot()).contains("marker-29"));
        term.write(b"/bin/sh -c 'trap \"\" HUP; echo started > job; /bin/sleep 1; echo leak > leaked' </dev/null >/dev/null 2>&1 &\r".to_vec());
        wait_until(|| dir.path().join("job").exists());
        term.close();
        assert!(matches!(term.snapshot().status, TerminalStatus::Exited(_)));
        thread::sleep(Duration::from_millis(1150));
        assert!(!dir.path().join("leaked").exists());
    }
}
