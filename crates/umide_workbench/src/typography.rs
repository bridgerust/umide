//! Use the platform's UI and fixed-width faces for all native text controls.
use floem::text::{
    FONT_SYSTEM,
    fontdb::{Database, Family, Query},
};

pub const SANS: &str = "sans-serif";
pub const MONO: &str = "monospace";

// Floem’s shaper requires a static weight match. The built-in SF Pro face is
// variable, so use the native Helvetica Neue Bold face for emphasized UI text.
#[cfg(target_os = "macos")]
const EMPHASIS: &str = "Helvetica Neue";
#[cfg(not(target_os = "macos"))]
const EMPHASIS: &str = SANS;

pub trait UiText {
    fn ui_bold(self) -> Self;
}

impl UiText for floem::style::Style {
    fn ui_bold(self) -> Self {
        self.font_family(EMPHASIS.to_owned()).font_bold()
    }
}

fn installed<'a>(db: &Database, families: &'a [&str]) -> Option<&'a str> {
    families.iter().copied().find(|name| {
        db.query(&Query {
            families: &[Family::Name(name)],
            ..Query::default()
        })
        .is_some()
    })
}

pub fn configure() {
    // Apple exposes the built-in SF Pro under "System Font" in its font files.
    #[cfg(target_os = "macos")]
    let (ui, code): (&[&str], &[&str]) = (
        &["System Font", "SF Pro Text", "Helvetica Neue"],
        &[".SF NS Mono", "SF Mono", "Menlo"],
    );
    #[cfg(target_os = "windows")]
    let (ui, code): (&[&str], &[&str]) = (
        &["Segoe UI", "Segoe UI Variable Text"],
        &["Cascadia Mono", "Consolas"],
    );
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let (ui, code): (&[&str], &[&str]) = (
        &["Adwaita Sans", "Cantarell", "Noto Sans", "DejaVu Sans"],
        &["Noto Sans Mono", "DejaVu Sans Mono"],
    );

    let mut fonts = FONT_SYSTEM.lock();
    let db = fonts.db_mut();
    if let Some(family) = installed(db, ui) {
        db.set_sans_serif_family(family);
    }
    if let Some(family) = installed(db, code) {
        db.set_monospace_family(family);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use floem::text::{Attrs, AttrsList, FamilyOwned, TextLayout, Weight};

    #[test]
    fn macos_renders_proportional_ui_text_and_monospaced_source() {
        configure();
        for (name, weight, monospace) in [
            (SANS, Weight::NORMAL, false),
            (EMPHASIS, Weight::BOLD, false),
            (MONO, Weight::NORMAL, true),
        ] {
            let family = FamilyOwned::parse_list(name).collect::<Vec<_>>();
            let mut layout = TextLayout::new();
            layout.set_text(
                "Umide",
                AttrsList::new(Attrs::new().family(&family).weight(weight)),
                None,
            );
            let id = layout.layout_runs().next().unwrap().glyphs[0].font_id;
            let fonts = FONT_SYSTEM.lock();
            let face = fonts.db().face(id).unwrap();
            assert_eq!(face.monospaced, monospace, "{}", face.post_script_name);
            if monospace {
                assert!(
                    face.post_script_name.contains("Mono")
                        || face.post_script_name.contains("Menlo"),
                    "{}",
                    face.post_script_name
                );
            } else {
                assert!(
                    face.post_script_name.contains("SF")
                        || face.post_script_name.contains("HelveticaNeue"),
                    "{}",
                    face.post_script_name
                );
            }
        }
    }
}
