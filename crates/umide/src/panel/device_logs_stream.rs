//! Streaming backend for the Device Logs panel: follow `adb logcat` for the
//! device the user is viewing and hand parsed lines to the UI.
//!
//! Mirrors the frame stream's shape (`emulator_stream.rs`): a background
//! thread produces items into a std `mpsc` channel and floem's
//! `update_signal_from_channel` applies them to a signal on the UI thread.
//! Lines are sent in small batches so a logcat burst wakes the UI once per
//! flush, not once per line. The returned [`LogcatHandle`] owns the `adb`
//! child; dropping it (or calling [`LogcatHandle::stop`]) kills the child,
//! which EOFs the reader thread, which drops the sender, which ends the
//! signal bridge — no orphaned `adb` after the panel closes or the app exits.

use std::io::BufRead;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use floem::ext_event::update_signal_from_channel;
use floem::reactive::RwSignal;

/// Severity of a logcat line, from the `-v time` tag (`I/Tag(pid): …`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSeverity {
    Verbose,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl LogSeverity {
    fn from_tag(c: char) -> Option<Self> {
        Some(match c {
            'V' => LogSeverity::Verbose,
            'D' => LogSeverity::Debug,
            'I' => LogSeverity::Info,
            'W' => LogSeverity::Warn,
            'E' => LogSeverity::Error,
            'F' => LogSeverity::Fatal,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            LogSeverity::Verbose => "V",
            LogSeverity::Debug => "D",
            LogSeverity::Info => "I",
            LogSeverity::Warn => "W",
            LogSeverity::Error => "E",
            LogSeverity::Fatal => "F",
        }
    }
}

/// One parsed logcat line. The full text is kept verbatim for display and
/// filtering; the severity drives per-line coloring in the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub severity: LogSeverity,
    pub text: String,
}

/// Parse a `logcat -v time` line, e.g.
/// `07-02 11:22:33.456 W/ActivityManager( 1234): message`.
/// The severity tag is the first standalone `X/` after the timestamp; lines
/// that don't match (logcat banners like `--------- beginning of main`,
/// continuation output) default to `Info` so nothing is dropped.
pub fn parse_log_line(line: &str) -> LogLine {
    // `-v time` prefix is `MM-DD HH:MM:SS.mmm ` (18 chars + space); be lenient
    // and just scan for the first `X/` where X is a known severity letter and
    // the char before it (if any) is a space.
    let bytes = line.as_bytes();
    let mut severity = None;
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i + 1] == b'/'
            && (i == 0 || bytes[i - 1] == b' ')
            && let Some(s) = LogSeverity::from_tag(bytes[i] as char)
        {
            severity = Some(s);
            break;
        }
    }
    LogLine {
        severity: severity.unwrap_or(LogSeverity::Info),
        text: line.to_string(),
    }
}

/// Handle to a running logcat stream. Dropping it stops the stream and kills
/// the `adb` child.
pub struct LogcatHandle {
    child: Arc<Mutex<Option<std::process::Child>>>,
}

impl LogcatHandle {
    /// Kill the `adb logcat` child (idempotent). The reader thread then hits
    /// EOF and winds the channel down.
    pub fn stop(&self) {
        if let Ok(mut guard) = self.child.lock()
            && let Some(mut child) = guard.take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for LogcatHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// How many lines a single flush may carry; a burst beyond this still arrives,
/// just across several UI wakes.
const BATCH_MAX: usize = 256;
/// How long the reader waits to accumulate a batch before flushing.
const BATCH_WINDOW: Duration = Duration::from_millis(50);

/// Start following `adb -s <serial> logcat` and deliver parsed lines to
/// `batch_signal` in arrival order (each signal update is one batch). Returns
/// `None` if adb couldn't be spawned (no SDK); the panel shows its empty state.
pub fn start_logcat_stream(
    serial: &str,
    batch_signal: RwSignal<Option<Vec<LogLine>>>,
) -> Option<LogcatHandle> {
    let mut cmd = umide_emulator::AndroidEmulator::logcat_command(serial);
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| tracing::error!("logcat spawn failed: {e}"))
        .ok()?;
    let stdout = child.stdout.take()?;

    // floem owns a reader thread on `rx` and applies each batch to the signal
    // on the UI thread (same bridge as the emulator frame stream).
    let (tx, rx) = mpsc::channel::<Vec<LogLine>>();
    update_signal_from_channel(batch_signal.write_only(), rx);

    std::thread::Builder::new()
        .name("umide-logcat-reader".into())
        .spawn(move || {
            let reader = std::io::BufReader::new(stdout);
            let mut batch: Vec<LogLine> = Vec::new();
            let mut last_flush = std::time::Instant::now();
            for line in reader.lines() {
                let Ok(line) = line else { break }; // EOF: child killed/died
                batch.push(parse_log_line(&line));
                if batch.len() >= BATCH_MAX || last_flush.elapsed() >= BATCH_WINDOW {
                    if tx.send(std::mem::take(&mut batch)).is_err() {
                        return; // UI receiver dropped — stop reading
                    }
                    last_flush = std::time::Instant::now();
                }
            }
            if !batch.is_empty() {
                let _ = tx.send(batch);
            }
        })
        .ok()?;

    Some(LogcatHandle {
        child: Arc::new(Mutex::new(Some(child))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_severities_from_time_format() {
        let w =
            parse_log_line("07-02 11:22:33.456 W/ActivityManager( 1234): low mem");
        assert_eq!(w.severity, LogSeverity::Warn);
        let e = parse_log_line("07-02 11:22:33.456 E/AndroidRuntime( 999): crash");
        assert_eq!(e.severity, LogSeverity::Error);
        assert!(e.text.contains("AndroidRuntime"));
    }

    #[test]
    fn banners_and_odd_lines_default_to_info() {
        assert_eq!(
            parse_log_line("--------- beginning of main").severity,
            LogSeverity::Info
        );
        assert_eq!(parse_log_line("").severity, LogSeverity::Info);
        // A path with slashes must not be misread as a severity tag.
        assert_eq!(
            parse_log_line("some random x/y text").severity,
            LogSeverity::Info
        );
    }

    #[test]
    fn severity_tag_requires_word_boundary() {
        // `W/` mid-word (no preceding space) is not a tag...
        assert_eq!(parse_log_line("fooW/bar").severity, LogSeverity::Info);
        // ...but at the very start of a line it is (e.g. `-v brief` style).
        assert_eq!(
            parse_log_line("W/Tag( 123): brief style").severity,
            LogSeverity::Warn
        );
    }

    /// Live: spawn the real follow-mode logcat against a running emulator,
    /// read a handful of lines, parse them, then kill the child (the reader
    /// must hit EOF and stop). Run with:
    ///   cargo test -p umide-app --lib -- --ignored live_logcat --nocapture
    #[test]
    #[ignore = "needs a running Android emulator"]
    fn live_logcat_streams_lines() {
        let serial = umide_emulator::AndroidEmulator::get_running_serials()
            .into_iter()
            .next()
            .expect("a running emulator");
        let mut child = umide_emulator::AndroidEmulator::logcat_command(&serial)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn adb logcat");
        let reader = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut lines = Vec::new();
        for line in reader.lines().take(20) {
            lines.push(parse_log_line(&line.expect("read line")));
        }
        child.kill().expect("kill logcat");
        child.wait().expect("reap logcat");
        eprintln!(
            "live logcat: {} lines; first: {}",
            lines.len(),
            lines.first().map(|l| l.text.as_str()).unwrap_or("")
        );
        assert_eq!(lines.len(), 20, "follow-mode logcat should keep producing");
        // A real device log always contains severities beyond the default.
        assert!(
            lines
                .iter()
                .any(|l| l.severity != LogSeverity::Info || l.text.contains('/')),
            "expected recognizable logcat content"
        );
    }
}
