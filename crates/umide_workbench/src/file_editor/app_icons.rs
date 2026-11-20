use floem::{prelude::*, style::TextColor};

use crate::{
    design::{Glyph, icon, muted},
    external_editor::Editor,
};

// Original vector fallbacks for the ribbon, drafting compass and builder's hammer.
const CODE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="#1275BB" d="m2 7 3-2 14 12-2 3z"/><path fill="#229FE2" d="m2 17 3 2L21 5l-4-2z"/><path fill="#1380C3" d="m17 3 5 2v14l-5 2z"/></svg>"##;
const STUDIO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="11" fill="#246954"/><path fill="none" stroke="#A7F2BF" stroke-width="1.8" stroke-linecap="round" d="M5 13q7 7 14 0"/><path fill="none" stroke="#F6FAFA" stroke-width="2.4" stroke-linecap="round" d="M11 7 6.5 19m6.5-12 4.5 12"/><circle cx="12" cy="6.5" r="2.1" fill="#A5E8BB"/></svg>"##;
#[cfg(target_os = "macos")]
const XCODE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect x="1" y="1" width="22" height="22" rx="5" fill="#187DC2"/><path fill="none" stroke="#82C3E7" stroke-width=".8" d="M3 7h18M3 12h18M3 17h18M7 3v18m5-18v18m5-18v18"/><path fill="none" stroke="#FFF" stroke-width="3" stroke-linecap="round" d="m7 18 8-9"/><path fill="#DBE7ED" d="m11 6 3-2 5 4 1 4-3-2-1 1z"/></svg>"##;

pub(super) fn view(editor: Editor) -> floem::view::AnyView {
    if editor == Editor::Custom {
        return icon(Glyph::Open, 16., muted).into_any();
    }
    #[cfg(target_os = "macos")]
    if let Some(bytes) = installed(editor) {
        return floem::views::img(move || bytes.to_vec())
            .style(|s| s.size(18, 18).flex_shrink(0_f32))
            .into_any();
    }
    let graphic = match editor {
        Editor::VsCode => CODE,
        Editor::AndroidStudio => STUDIO,
        #[cfg(target_os = "macos")]
        Editor::Xcode => XCODE,
        Editor::Custom => unreachable!(),
    };
    svg(graphic)
        .style(|s| s.size(18, 18).flex_shrink(0_f32).set(TextColor, None))
        .into_any()
}

#[cfg(target_os = "macos")]
fn installed(editor: Editor) -> Option<&'static [u8]> {
    use std::sync::OnceLock;
    static CODE: OnceLock<Vec<u8>> = OnceLock::new();
    static STUDIO: OnceLock<Vec<u8>> = OnceLock::new();
    static XCODE: OnceLock<Vec<u8>> = OnceLock::new();
    let (id, cache) = match editor {
        Editor::VsCode => (c"com.microsoft.VSCode", &CODE),
        Editor::AndroidStudio => (c"com.google.android.studio", &STUDIO),
        Editor::Xcode => (c"com.apple.dt.Xcode", &XCODE),
        Editor::Custom => return None,
    };
    let bytes = cache.get_or_init(|| png(id));
    (!bytes.is_empty()).then_some(bytes)
}

pub(super) fn application(_path: &std::path::Path) -> floem::view::AnyView {
    #[cfg(target_os = "macos")]
    if let Ok(path) = std::ffi::CString::new(_path.as_os_str().as_encoded_bytes()) {
        let bytes = png(&path);
        if !bytes.is_empty() {
            return floem::views::img(move || bytes.clone())
                .style(|s| s.size(18, 18).flex_shrink(0_f32))
                .into_any();
        }
    }
    icon(Glyph::Open, 16., muted).into_any()
}

#[cfg(target_os = "macos")]
fn png(query: &std::ffi::CStr) -> Vec<u8> {
    unsafe extern "C" {
        fn umide_editor_icon(
            id: *const std::ffi::c_char,
            out: *mut u8,
            capacity: usize,
        ) -> usize;
    }
    let mut out = vec![0u8; 16 * 1024];
    // The native helper copies at most capacity bytes before returning.
    let count =
        unsafe { umide_editor_icon(query.as_ptr(), out.as_mut_ptr(), out.len()) };
    out.truncate(count);
    out
}
