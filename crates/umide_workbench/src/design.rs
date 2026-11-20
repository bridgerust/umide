//! Shared colors, type and controls for the native workbench.
use floem::prelude::*;

pub use crate::typography::{MONO, SANS, UiText};

fn themed(dark: [u8; 3], light: [u8; 3]) -> Color {
    let [r, g, b] = if crate::appearance::is_light() {
        light
    } else {
        dark
    };
    Color::from_rgb8(r, g, b)
}

// Keep each color's dark and light values together so a role stays consistent
// across controls, text, icons and diff rows.
macro_rules! palette {
    ($($name:ident: $dark:expr => $light:expr),+ $(,)?) => {
        $(pub fn $name() -> Color { themed($dark, $light) })+
    };
}

palette! {
    canvas:      [22, 25, 29]    => [247, 249, 248],
    rail:        [16, 19, 23]    => [235, 240, 237],
    panel:       [25, 29, 34]    => [253, 254, 253],
    surface:     [32, 37, 43]    => [239, 243, 240],
    raised:      [43, 50, 57]    => [226, 233, 229],
    line:        [54, 63, 70]    => [167, 178, 171],
    subtle_line: [43, 49, 55]    => [211, 219, 214],
    ink:         [238, 241, 242] => [31, 43, 38],
    muted:       [177, 188, 193] => [66, 83, 75],
    faint:       [143, 155, 161] => [82, 98, 90],
    accent:      [154, 232, 201] => [17, 99, 70],
    on_accent:   [22, 25, 29]    => [255, 255, 255],
    mint_wash:   [37, 55, 51]    => [218, 240, 229],
    lilac:       [188, 181, 241] => [92, 73, 152],
    peach:       [233, 173, 144] => [134, 69, 41],
    danger:      [245, 159, 158] => [161, 54, 63],
    diff_hunk:   [42, 39, 55]    => [240, 236, 250],
    diff_added:  [27, 47, 40]    => [229, 244, 234],
    diff_removed: [52, 34, 38]    => [251, 235, 236],
}

#[derive(Clone, Copy)]
pub enum Glyph {
    Brand,
    Chat,
    Terminal,
    Code,
    Branch,
    Folder,
    Image,
    Send,
    Stop,
    Check,
    Chevron,
    Plus,
    Device,
    Globe,
    Shield,
    Claude,
    Refresh,
    Play,
    Android,
    Apple,
    Rail,
    Diff,
    Logs,
    Build,
    Open,
    Search,
}

macro_rules! codicon {
    ($name:literal) => {
        include_str!(concat!("../../../icons/codicons/", $name, ".svg"))
    };
}

impl Glyph {
    fn svg(self) -> &'static str {
        match self {
            Self::Brand => include_str!("../../../icons/umide/umide_logo.svg"),
            Self::Terminal => codicon!("terminal"),
            Self::Code => codicon!("file-code"),
            Self::Branch => codicon!("source-control"),
            Self::Folder => codicon!("folder-opened"),
            Self::Image => codicon!("open-preview"),
            Self::Send => codicon!("arrow-up"),
            Self::Stop => codicon!("debug-stop"),
            Self::Chevron => codicon!("chevron-down"),
            Self::Plus => codicon!("add"),
            Self::Refresh => codicon!("refresh"),
            Self::Play => codicon!("debug-start"),
            Self::Rail => codicon!("layout-sidebar-left"),
            Self::Diff => codicon!("git-compare"),
            Self::Build => codicon!("symbol-method"),
            Self::Search => codicon!("search"),
            Self::Open => codicon!("link-external"),
            Self::Logs => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><path d="M3 12h4l3-7 4 14 3-7h4"/></svg>"#
            }
            Self::Chat => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round"><path d="M5 4.5h14a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2h-8l-5.5 3v-3H5a2 2 0 0 1-2-2v-9a2 2 0 0 1 2-2z"/></svg>"#
            }
            Self::Globe => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" stroke="currentColor" fill="none" stroke-width="1.6"><circle cx="12" cy="12" r="9"/><ellipse cx="12" cy="12" rx="4" ry="9"/><path d="M3 12h18"/></svg>"#
            }
            Self::Device => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><rect x="6" y="2" width="12" height="20" rx="3"/><path d="M10 5h4m-3 14h2"/></svg>"#
            }
            Self::Check => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m4 12 5 5 11-11"/></svg>"#
            }
            Self::Shield => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round"><path d="m12 2 8 3v6c0 5-3 8-8 11-5-3-8-6-8-11V5z"/></svg>"#
            }
            Self::Claude => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><path d="m12 3 .4 18M4.3 7.4l15.4 9.2M4 16.5l16-9M7.5 4l9 16M3 12h18M7.5 20l9-16"/></svg>"#
            }
            Self::Android => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><path d="m6 4 2 3m10-3-2 3M3 16a9 9 0 1 1 18 0z"/><path d="M8.5 12h.1m6.8 0h.1" stroke-width="2.4"/></svg>"#
            }
            Self::Apple => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor"><path d="M15.7 2c.2 2.5-1.7 4.5-4 4.6-.3-2.1 1.8-4.3 4-4.6zM18.9 7.8c-2.2 1.5-2.4 4.8.3 6.4-.5 2-2.4 5.9-4.3 5.9-1.1 0-1.6-.8-3-.8s-2 .8-3 .8C6.7 20.1 4 15.6 4 11.8c0-3 1.7-5.1 4.4-5.1 1.5 0 2.5.9 3.4.9.9 0 2.2-.9 3.8-.9 1.2 0 2.5.5 3.3 1.1z"/></svg>"#
            }
        }
    }
}

pub fn icon(
    glyph: Glyph,
    size: f64,
    color: impl Fn() -> Color + 'static,
) -> floem::views::Svg {
    svg(glyph.svg())
        .style(move |s| s.size(size, size).flex_shrink(0_f32).color(color()))
}

pub fn button(title: &'static str, action: impl Fn() + 'static) -> impl IntoView {
    Button::new(title).action(action).style(|s| {
        s.padding_horiz(10)
            .padding_vert(5)
            .min_height(28)
            .color(ink())
            .background(surface())
            .font_size(12)
            .border(1)
            .border_radius(6)
            .border_color(line())
            .hover(|s| s.background(raised()))
            .focus_visible(|s| s.outline(2).outline_color(accent()))
            .disabled(|s| s.color(faint()).background(panel()))
    })
}

pub fn action_button(
    glyph: Glyph,
    label: &'static str,
    action: impl Fn() + 'static,
) -> impl IntoView {
    Button::new(
        Stack::horizontal((icon(glyph, 14., muted), label))
            .style(|s| s.items_center().gap(7)),
    )
    .action(action)
    .style(|s| {
        s.padding_horiz(10)
            .height(30)
            .border(1)
            .border_color(line())
            .border_radius(6)
            .color(ink())
            .background(surface())
            .font_size(12)
            .hover(|s| s.background(raised()))
            .focus_visible(|s| s.outline(2).outline_color(accent()))
            .disabled(|s| s.color(faint()).background(panel()))
    })
}
