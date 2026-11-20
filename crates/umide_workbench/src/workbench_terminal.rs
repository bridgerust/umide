//! Interactive terminal for the selected checkout. The view paints Alacritty's
//! grid with the same fixed-cell geometry used when resizing its PTY.
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    thread,
    time::Duration,
};

use crate::{
    design::{
        self, Glyph, MONO, UiText, accent, action_button, faint, ink, muted,
        panel as panel_color,
    },
    state::{Workbench, Workspace},
    terminal_session::{TerminalSession, TerminalSnapshot, TerminalStatus},
};
use alacritty_terminal::{
    term::{
        TermMode,
        cell::{Cell as GridCell, Flags},
        color::Colors,
    },
    vte::ansi::{Color as Ansi, CursorShape, NamedColor},
};
use floem::{
    Renderer,
    action::{exec_after, set_ime_allowed, set_ime_cursor_area},
    context::PaintCx,
    event::{Event, EventListener, EventPropagation},
    ext_event::create_ext_action,
    peniko::kurbo::{Point, Rect, Size, Stroke},
    prelude::*,
    reactive::{Memo, Scope},
    text::{Attrs, AttrsList, FamilyOwned, TextLayout, Weight},
    ui_events::pointer::{PointerButton, PointerEvent},
};

#[derive(Clone)]
enum Session {
    Opening,
    Running(Rc<TerminalSession>),
    Failed(String),
    Stopped,
}

#[derive(Clone, Copy)]
pub(crate) struct Terminals {
    scope: Scope,
    sessions: RwSignal<HashMap<Workspace, RwSignal<Session>>>,
}

impl Terminals {
    pub(crate) fn new() -> Self {
        Self {
            scope: Scope::current(),
            sessions: RwSignal::new(HashMap::new()),
        }
    }

    pub(crate) fn close(self) {
        self.sessions.update(|sessions| {
            for (_, state) in sessions.drain() {
                if let Session::Running(session) = state.get_untracked() {
                    session.close();
                }
                state.set(Session::Stopped);
            }
        })
    }

    fn open(self, workspace: Workspace) -> RwSignal<Session> {
        if let Some(state) = self
            .sessions
            .with_untracked(|sessions| sessions.get(&workspace).copied())
        {
            return state;
        }
        let state = self.scope.create_rw_signal(Session::Opening);
        self.sessions.update(|sessions| {
            sessions.insert(workspace.clone(), state);
        });
        self.launch(workspace, state);
        state
    }

    fn launch(self, workspace: Workspace, state: RwSignal<Session>) {
        state.set(Session::Opening);
        let done = create_ext_action(
            self.scope,
            move |path: Result<std::path::PathBuf, String>| {
                if !state.try_with_untracked(|s| matches!(s, Some(Session::Opening)))
                {
                    return;
                }
                state.set(
                    match path.and_then(|path| TerminalSession::start(&path, 80, 24))
                    {
                        Ok(session) => Session::Running(Rc::new(session)),
                        Err(message) => Session::Failed(message),
                    },
                );
            },
        );
        thread::spawn(move || done(workspace.path()));
    }
}

const FONT: f32 = 13.;
const LINE: f64 = 20.;
const PAD: f64 = 8.;

fn glyph_layout(text: &str, color: Color, flags: Flags) -> TextLayout {
    let family: Vec<_> = FamilyOwned::parse_list(MONO).collect();
    let mut attrs = Attrs::new().family(&family).font_size(FONT).color(color);
    if flags.contains(Flags::BOLD) {
        attrs = attrs.weight(Weight::BOLD);
    }
    if flags.contains(Flags::ITALIC) {
        attrs = attrs.style(floem::text::Style::Italic);
    }
    let mut layout = TextLayout::new();
    layout.set_text(text, AttrsList::new(attrs), None);
    layout
}

fn ansi_color(color: Ansi, palette: &Colors) -> Color {
    let index = match color {
        Ansi::Spec(rgb) => return Color::from_rgb8(rgb.r, rgb.g, rgb.b),
        Ansi::Indexed(i) => usize::from(i),
        Ansi::Named(c) => c as usize,
    };
    if let Some(rgb) = palette[index] {
        return Color::from_rgb8(rgb.r, rgb.g, rgb.b);
    }
    let basic = || {
        if crate::appearance::is_light() {
            [
                [35, 42, 39],
                [157, 48, 45],
                [25, 103, 69],
                [127, 88, 14],
                [31, 86, 146],
                [126, 58, 139],
                [0, 102, 111],
                [211, 217, 213],
                [88, 98, 94],
                [176, 49, 46],
                [19, 115, 73],
                [150, 96, 0],
                [35, 99, 168],
                [142, 71, 156],
                [0, 116, 130],
                [247, 249, 248],
            ]
        } else {
            [
                [22, 25, 29],
                [220, 126, 125],
                [133, 207, 163],
                [225, 193, 126],
                [116, 166, 227],
                [185, 153, 224],
                [126, 203, 207],
                [208, 215, 218],
                [127, 143, 150],
                [250, 155, 151],
                [161, 232, 188],
                [248, 216, 150],
                [151, 192, 249],
                [213, 183, 245],
                [155, 230, 234],
                [244, 247, 246],
            ]
        }
    };
    let [r, g, b] = match index {
        0..16 => basic()[index],
        16..232 => {
            let v = index - 16;
            let vals = [0, 95, 135, 175, 215, 255];
            [vals[v / 36], vals[(v / 6) % 6], vals[v % 6]]
        }
        232..256 => [8 + 10 * (index as u8 - 232); 3],
        259..267 => basic()[index - 259],
        n if n == NamedColor::Background as usize => return design::canvas(),
        n if n == NamedColor::Cursor as usize => return accent(),
        n if n == NamedColor::DimForeground as usize => return faint(),
        _ => return ink(),
    };
    Color::from_rgb8(r, g, b)
}

fn text_for_cell(cell: &GridCell) -> String {
    let mut text = cell.c.to_string();
    if let Some(zero) = cell.zerowidth() {
        text.extend(zero);
    }
    text
}

type Layouts = HashMap<(String, [u8; 4], u16), TextLayout>;
type Selection = Option<(usize, usize)>;

fn paint_grid(
    cx: &mut PaintCx,
    size: Size,
    grid: &TerminalSnapshot,
    w: f64,
    selection: Selection,
    focused: bool,
    layouts: &mut Layouts,
) {
    let background = ansi_color(Ansi::Named(NamedColor::Background), &grid.colors);
    let foreground = ansi_color(Ansi::Named(NamedColor::Foreground), &grid.colors);
    cx.fill(&Rect::ZERO.with_size(size), background, 0.);
    if grid.cols == 0 {
        return;
    }
    if layouts.len() > 4096 {
        layouts.clear();
    }
    let selection = selection.map(|(a, b)| a.min(b)..=a.max(b));
    for (i, cell) in grid.cells.iter().enumerate() {
        let (col, row) = (i % grid.cols, i / grid.cols);
        let (x, y) = (PAD + col as f64 * w, PAD + row as f64 * LINE);
        if y >= size.height || x >= size.width {
            continue;
        }
        let f = cell.flags;
        let mut fg = if cell.fg == Ansi::Named(NamedColor::Foreground) {
            foreground
        } else {
            ansi_color(cell.fg, &grid.colors)
        };
        let mut bg = if cell.bg == Ansi::Named(NamedColor::Background) {
            background
        } else {
            ansi_color(cell.bg, &grid.colors)
        };
        if f.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if selection.as_ref().is_some_and(|s| s.contains(&i)) {
            bg = design::mint_wash();
            fg = ink();
        }
        let rect = Rect::new(x, y, x + w, y + LINE);
        if bg != background {
            cx.fill(&rect, bg, 0.);
        }
        if let Some((cx_col, cx_row, shape)) = grid.cursor
            && cx_col == col
            && cx_row == row
            && shape != CursorShape::Hidden
        {
            let cursor_color =
                ansi_color(Ansi::Named(NamedColor::Cursor), &grid.colors);
            match (focused, shape) {
                (true, CursorShape::Block) => {
                    cx.fill(&rect, cursor_color, 0.);
                    fg = background;
                }
                (true, CursorShape::Beam) => {
                    cx.fill(&Rect::new(x, y, x + 2., y + LINE), cursor_color, 0.)
                }
                (true, CursorShape::Underline) => cx.fill(
                    &Rect::new(x, y + LINE - 2., x + w, y + LINE),
                    cursor_color,
                    0.,
                ),
                _ => cx.stroke(&rect, cursor_color, &Stroke::new(1.)),
            }
        }
        if f.intersects(
            Flags::HIDDEN
                | Flags::WIDE_CHAR_SPACER
                | Flags::LEADING_WIDE_CHAR_SPACER,
        ) {
            continue;
        }
        if f.intersects(Flags::ALL_UNDERLINES) {
            cx.fill(&Rect::new(x, y + LINE - 2., x + w, y + LINE - 1.), fg, 0.);
        }
        if f.contains(Flags::STRIKEOUT) {
            cx.fill(
                &Rect::new(x, y + LINE * 0.52, x + w, y + LINE * 0.52 + 1.),
                fg,
                0.,
            );
        }
        if cell.c == ' ' && cell.zerowidth().is_none() {
            continue;
        }
        let style = f & (Flags::BOLD | Flags::ITALIC);
        let text = text_for_cell(cell);
        let layout = layouts
            .entry((text, fg.to_rgba8().to_u8_array(), style.bits()))
            .or_insert_with_key(|(text, _, _)| glyph_layout(text, fg, style));
        cx.draw_text(
            layout,
            Point::new(x, y + (LINE - layout.size().height) / 2.),
        );
    }
}

fn copy_text(grid: &TerminalSnapshot, selection: Selection) -> String {
    if grid.cells.is_empty() || grid.cols == 0 {
        return String::new();
    }
    let (a, b) = selection.unwrap_or((0, grid.cells.len() - 1));
    let mut text = String::new();
    let mut row = String::new();
    for index in a.min(b)..=a.max(b).min(grid.cells.len() - 1) {
        let cell = &grid.cells[index];
        if !cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            row.push_str(&text_for_cell(cell));
        }
        if (index + 1) % grid.cols == 0 && !cell.flags.contains(Flags::WRAPLINE) {
            text.push_str(row.trim_end());
            text.push('\n');
            row.clear();
        }
    }
    text.push_str(row.trim_end());
    text.trim_end_matches('\n').to_owned()
}

fn key_bytes(key: &Key, mods: Modifiers, mode: TermMode) -> Option<Vec<u8>> {
    if mods.intersects(Modifiers::META) {
        return None;
    }
    let control = mods.contains(Modifiers::CONTROL);
    let alt = mods.contains(Modifiers::ALT);
    let shift = mods.contains(Modifiers::SHIFT);
    let parameter = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(control);
    if let Key::Character(text) = key {
        let mut bytes = if control {
            let mut chars = text.chars();
            let c = chars.next()?.to_ascii_uppercase();
            if chars.next().is_some() {
                return None;
            }
            vec![match c {
                '@'..='_' => c as u8 & 31,
                ' ' | '2' => 0,
                '?' => b'\x7f',
                _ => return None,
            }]
        } else {
            text.as_bytes().to_vec()
        };
        if alt && text.is_ascii() {
            bytes.insert(0, 27);
        }
        return Some(bytes);
    }
    let Key::Named(key) = key else { return None };
    let final_char = match key {
        NamedKey::ArrowUp => Some('A'),
        NamedKey::ArrowDown => Some('B'),
        NamedKey::ArrowRight => Some('C'),
        NamedKey::ArrowLeft => Some('D'),
        NamedKey::Home => Some('H'),
        NamedKey::End => Some('F'),
        _ => None,
    };
    if let Some(ch) = final_char {
        let result = if parameter > 1 {
            format!("\x1b[1;{parameter}{ch}")
        } else if mode.contains(TermMode::APP_CURSOR) {
            format!("\x1bO{ch}")
        } else {
            format!("\x1b[{ch}")
        };
        return Some(result.into_bytes());
    }
    let tilde = match key {
        NamedKey::Insert => Some(2),
        NamedKey::Delete => Some(3),
        NamedKey::PageUp => Some(5),
        NamedKey::PageDown => Some(6),
        NamedKey::F5 => Some(15),
        NamedKey::F6 => Some(17),
        NamedKey::F7 => Some(18),
        NamedKey::F8 => Some(19),
        NamedKey::F9 => Some(20),
        NamedKey::F10 => Some(21),
        NamedKey::F11 => Some(23),
        NamedKey::F12 => Some(24),
        _ => None,
    };
    if let Some(n) = tilde {
        return Some(
            if parameter == 1 {
                format!("\x1b[{n}~")
            } else {
                format!("\x1b[{n};{parameter}~")
            }
            .into_bytes(),
        );
    }
    let mut text = match key {
        NamedKey::Enter => "\r",
        NamedKey::Backspace => "\x7f",
        NamedKey::Escape => "\x1b",
        NamedKey::Tab if shift => "\x1b[Z",
        NamedKey::Tab => "\t",
        NamedKey::F1 => "\x1bOP",
        NamedKey::F2 => "\x1bOQ",
        NamedKey::F3 => "\x1bOR",
        NamedKey::F4 => "\x1bOS",
        _ => return None,
    }
    .as_bytes()
    .to_vec();
    if alt && matches!(key, NamedKey::Backspace | NamedKey::Enter) {
        text.insert(0, 27);
    }
    Some(text)
}

fn paste(session: &TerminalSession, mode: TermMode) {
    if let Ok(text) = floem::Clipboard::get_contents() {
        let data = if mode.contains(TermMode::BRACKETED_PASTE) {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
        } else {
            text.replace("\r\n", "\n").replace('\n', "\r")
        };
        session.write(data.into_bytes());
    }
}

fn poll(
    session: Rc<TerminalSession>,
    grid: RwSignal<Rc<TerminalSnapshot>>,
    revision: u64,
    alive: Rc<Cell<bool>>,
) {
    exec_after(Duration::from_millis(32), move |_| {
        if !alive.get() || grid.try_with_untracked(|current| current.is_none()) {
            return;
        }
        let current = session.revision();
        if current != revision {
            grid.set(Rc::new(session.snapshot()));
        }
        if grid.try_with_untracked(|s| {
            s.is_some_and(|s| matches!(s.status, TerminalStatus::Running))
        }) {
            poll(session, grid, current, alive);
        }
    });
}

fn dock(
    wb: Workbench,
    status: impl IntoView,
    actions: impl IntoView,
    body: impl IntoView,
    hide: impl Fn() + Copy + 'static,
) -> impl IntoView {
    let directory = Label::derived(move || {
        wb.root.with(|root| {
            let Some(root) = root else {
                return String::new();
            };
            let name = root
                .file_name()
                .unwrap_or(root.as_os_str())
                .to_string_lossy();
            if wb.isolated.get() {
                crate::i18n::fmt("{name} · Isolated", &[("name", name.as_ref())])
            } else {
                name.into_owned()
            }
        })
    })
    .style(|s| {
        s.min_width(0)
            .text_ellipsis()
            .font_size(11)
            .color(muted())
            .flex_grow(1_f32)
    });
    let bar = Stack::horizontal((
        design::icon(Glyph::Terminal, 14., accent),
        crate::i18n::label("Terminal")
            .style(|s| s.font_size(13).ui_bold().color(ink())),
        directory,
        status,
        actions,
        action_button(Glyph::Chevron, "Hide", hide)
            .tooltip(|| crate::i18n::tr("Hide terminal")),
    ))
    .style(|s| {
        s.width_full()
            .min_width(0)
            .height(36)
            .flex_shrink(0_f32)
            .items_center()
            .gap(6)
            .padding_horiz(8)
            .background(design::surface())
            .border_bottom(1)
            .border_color(design::subtle_line())
    });
    Stack::vertical((
        bar,
        body.into_view()
            .style(|s| s.width_full().min_height(0).flex_grow(1_f32).flex_basis(0)),
    ))
    .style(|s| s.size_full().min_size(0, 0).background(design::canvas()))
}

fn placeholder(text: String) -> impl IntoView {
    Label::derived(move || crate::i18n::message(&text)).style(|s| {
        s.width_full()
            .padding(10)
            .font_size(13)
            .color(muted())
            .text_overflow(floem::style::TextOverflow::Wrap)
    })
}

fn screen(
    wb: Workbench,
    session: Rc<TerminalSession>,
    workspace: Workspace,
    terminals: Terminals,
    state: RwSignal<Session>,
    hide: impl Fn() + Copy + 'static,
) -> impl IntoView {
    let revision = session.revision();
    let grid = RwSignal::new(Rc::new(session.snapshot()));
    let alive = Rc::new(Cell::new(true));
    poll(session.clone(), grid, revision, alive.clone());
    let width = glyph_layout("W", ink(), Flags::empty())
        .size()
        .width
        .max(1.);
    let focused = RwSignal::new(false);
    let preedit = RwSignal::new(String::new());
    let selection = RwSignal::new(None::<(usize, usize)>);
    let drag = RwSignal::new(None::<usize>);
    let layouts = RefCell::new(HashMap::new());
    let paint_scope = Scope::current();
    let term = floem::views::canvas(move |cx, size| {
        paint_scope.enter(|| {
            grid.with(|grid| {
                paint_grid(
                    cx,
                    size,
                    grid,
                    width,
                    selection.get(),
                    focused.get(),
                    &mut layouts.borrow_mut(),
                );
                preedit.with(|text| {
                    if !text.is_empty()
                        && let Some((col, row, _)) = grid.cursor
                    {
                        let pos = Point::new(
                            PAD + col as f64 * width,
                            PAD + row as f64 * LINE,
                        );
                        let text = glyph_layout(text, ink(), Flags::empty());
                        cx.fill(
                            &Rect::from_origin_size(
                                pos,
                                Size::new(text.size().width, LINE),
                            ),
                            panel_color(),
                            0.,
                        );
                        cx.draw_text(&text, pos);
                    }
                });
            })
        });
    })
    .style(|s| s.size_full().min_size(0, 0).focusable(true));
    let id = term.id();
    let pos = move |p: Point| {
        grid.with_untracked(|s| {
            let col = (((p.x - PAD) / width).floor().max(0.) as usize)
                .min(s.cols.saturating_sub(1));
            let row = (((p.y - PAD) / LINE).floor().max(0.) as usize)
                .min(s.rows.saturating_sub(1));
            row * s.cols + col
        })
    };
    let term = term
        .on_event_stop(EventListener::PointerDown, move |e| {
            if let Event::Pointer(PointerEvent::Down(p)) = e
                && p.button == Some(PointerButton::Primary)
            {
                id.request_focus();
                if let Some(pid) = p.pointer.pointer_id {
                    id.set_pointer_capture(pid);
                }
                drag.set(Some(pos(p.state.logical_point())));
                selection.set(None);
            }
        })
        .on_event_cont(EventListener::PointerMove, move |e| {
            if let Some(a) = drag.get_untracked()
                && let Some(p) = e.point()
            {
                let b = pos(p);
                selection.set((a != b).then_some((a, b)));
            }
        })
        .on_event_cont(EventListener::PointerUp, move |e| {
            if let Event::Pointer(PointerEvent::Up(p)) = e
                && let Some(pid) = p.pointer.pointer_id
            {
                id.release_pointer_capture(pid);
            }
            drag.set(None);
        })
        .on_event_cont(EventListener::LostPointerCapture, move |_| drag.set(None))
        .on_event_cont(EventListener::FocusGained, move |_| {
            focused.set(true);
            set_ime_allowed(true);
        })
        .on_event_cont(EventListener::FocusLost, move |_| {
            focused.set(false);
            preedit.set(String::new());
            set_ime_allowed(false);
        });
    let paste_session = session.clone();
    let key_session = session.clone();
    let ime_session = session.clone();
    let wheel = session.clone();
    let resize = session.clone();
    let wheel_remainder = Cell::new(0.);
    let copy = move || {
        let text = grid.with_untracked(|s| copy_text(s, selection.get_untracked()));
        if !text.is_empty() {
            wb.status.set(
                if floem::Clipboard::set_contents(text).is_ok() {
                    "Terminal text copied"
                } else {
                    "Could not copy terminal text"
                }
                .into(),
            );
        }
    };
    let term = term
        .on_event(EventListener::KeyDown, move |e| {
            let Event::Key(key) = e else {
                return EventPropagation::Continue;
            };
            if key.is_composing || !preedit.with_untracked(String::is_empty) {
                return EventPropagation::Stop;
            }
            let clipboard_mod = if cfg!(target_os = "macos") {
                key.modifiers.intersects(Modifiers::META)
            } else {
                key.modifiers
                    .contains(Modifiers::CONTROL | Modifiers::SHIFT)
            };
            if clipboard_mod && let Key::Character(ch) = &key.key {
                if ch.eq_ignore_ascii_case("c") {
                    copy();
                    return EventPropagation::Stop;
                }
                if ch.eq_ignore_ascii_case("v") {
                    paste(&key_session, grid.with_untracked(|g| g.mode));
                    return EventPropagation::Stop;
                }
            }
            if key.modifiers == Modifiers::SHIFT
                && matches!(
                    key.key,
                    Key::Named(NamedKey::PageUp | NamedKey::PageDown)
                )
            {
                let rows = grid.with_untracked(|s| s.rows) as i32;
                key_session.scroll(if key.key == Key::Named(NamedKey::PageUp) {
                    rows
                } else {
                    -rows
                });
                selection.set(None);
                return EventPropagation::Stop;
            }
            if let Some(bytes) =
                key_bytes(&key.key, key.modifiers, grid.with_untracked(|g| g.mode))
            {
                selection.set(None);
                key_session.write(bytes);
                EventPropagation::Stop
            } else {
                EventPropagation::Continue
            }
        })
        .on_event_stop(EventListener::ImePreedit, move |e| {
            if let Event::ImePreedit { text, .. } = e {
                preedit.set(text.clone());
                if let Some((col, row, _)) = grid.with_untracked(|g| g.cursor) {
                    let origin = id.get_window_origin();
                    set_ime_cursor_area(
                        Point::new(
                            origin.x + PAD + col as f64 * width,
                            origin.y + PAD + row as f64 * LINE,
                        ),
                        Size::new(width, LINE),
                    );
                }
            }
        })
        .on_event_stop(EventListener::ImeCommit, move |e| {
            if let Event::ImeCommit(text) = e {
                preedit.set(String::new());
                selection.set(None);
                ime_session.write(text.as_bytes().to_vec());
            }
        })
        .on_event_stop(EventListener::PointerWheel, move |e| {
            if let Some(delta) = e.pixel_scroll_delta_vec2() {
                let total = wheel_remainder.get() - delta.y / LINE;
                let lines = total.trunc() as i32;
                wheel_remainder.set(total.fract());
                if lines != 0 {
                    selection.set(None);
                    wheel.scroll(lines);
                }
            }
        })
        .on_resize(move |r| {
            let cols = ((r.width() - 2. * PAD) / width)
                .floor()
                .clamp(2., u16::MAX as f64) as u16;
            let rows = ((r.height() - 2. * PAD) / LINE)
                .floor()
                .clamp(1., u16::MAX as f64) as u16;
            resize.resize(cols, rows, width.round() as u16, LINE as u16);
        })
        .on_cleanup(move || alive.set(false));
    id.request_focus();
    let stopped = Memo::new(move |_| {
        grid.with(|g| matches!(g.status, TerminalStatus::Exited(_)))
    });
    let controls = dyn_container(
        move || stopped.get(),
        move |done| {
            let handle = session.clone();
            let owner = workspace.clone();
            action_button(
                if done { Glyph::Refresh } else { Glyph::Stop },
                if done { "Restart shell" } else { "Stop shell" },
                move || {
                    handle.close();
                    if done {
                        terminals.launch(owner.clone(), state)
                    } else {
                        state.set(Session::Stopped);
                    }
                },
            )
        },
    );
    let status = Label::derived(move || {
        grid.with(|g| match g.status {
            TerminalStatus::Running => String::new(),
            TerminalStatus::Exited(Some(code)) => {
                crate::i18n::fmt("Exited ({code})", &[("code", &code.to_string())])
            }
            TerminalStatus::Exited(None) => crate::i18n::tr("Exited").into(),
        })
    })
    .style(|s| s.font_size(11).color(muted()));
    let actions = Stack::horizontal((
        design::button("Copy", copy)
            .tooltip(|| crate::i18n::tr("Copy selection or visible screen")),
        design::button("Paste", move || {
            paste(&paste_session, grid.with_untracked(|g| g.mode));
            id.request_focus();
        })
        .style(move |s| s.set_disabled(stopped.get())),
        controls,
    ))
    .style(|s| s.items_center().gap(5));
    dock(wb, status, actions, term, hide)
}

pub(crate) fn panel(
    wb: Workbench,
    terminals: Terminals,
    hide: impl Fn() + Copy + 'static,
) -> impl IntoView {
    let selected = wb.workspace_selection();
    dyn_container(
        move || selected.get(),
        move |workspace| {
            let workspace = match workspace {
                Ok(w) => w,
                Err(e) => {
                    return dock(
                        wb,
                        Empty::new(),
                        Empty::new(),
                        placeholder(e),
                        hide,
                    )
                    .into_any();
                }
            };
            let state = terminals.open(workspace.clone());
            dyn_container(
                move || state.get(),
                move |session| match session {
                    Session::Running(handle) => {
                        screen(wb, handle, workspace.clone(), terminals, state, hide)
                            .into_any()
                    }
                    Session::Opening => dock(
                        wb,
                        Empty::new(),
                        Empty::new(),
                        placeholder("Opening shell…".into()),
                        hide,
                    )
                    .into_any(),
                    Session::Failed(_) | Session::Stopped => {
                        let text = match session {
                            Session::Failed(e) => e,
                            _ => "Shell stopped".into(),
                        };
                        let owner = workspace.clone();
                        let restart = action_button(
                            Glyph::Refresh,
                            "Restart shell",
                            move || terminals.launch(owner.clone(), state),
                        );
                        dock(wb, Empty::new(), restart, placeholder(text), hide)
                            .into_any()
                    }
                },
            )
            .style(|s| s.size_full().min_size(0, 0))
            .into_any()
        },
    )
    .style(|s| s.size_full().min_size(0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sends_shell_controls_and_application_cursor_keys() {
        let mode = TermMode::empty();
        assert_eq!(
            key_bytes(&Key::Character("c".into()), Modifiers::CONTROL, mode),
            Some(vec![3])
        );
        assert_eq!(
            key_bytes(&Key::Character("d".into()), Modifiers::CONTROL, mode),
            Some(vec![4])
        );
        assert_eq!(
            key_bytes(
                &Key::Named(NamedKey::ArrowLeft),
                Modifiers::empty(),
                TermMode::APP_CURSOR
            )
            .unwrap(),
            b"\x1bOD"
        );
        assert_eq!(
            key_bytes(&Key::Named(NamedKey::ArrowRight), Modifiers::CONTROL, mode)
                .unwrap(),
            b"\x1b[1;5C"
        );
        assert_eq!(
            key_bytes(&Key::Character("é".into()), Modifiers::empty(), mode)
                .unwrap(),
            "é".as_bytes()
        );
    }
}
