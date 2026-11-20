use std::path::PathBuf;

use floem::{ext_event::create_ext_action, reactive::Scope};

pub(crate) fn choose(
    scope: Scope,
    chosen: impl FnOnce(Option<PathBuf>) + 'static,
) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|error| {
            format!("Could not open the application picker: {error}")
        })?;
    let dialog =
        rfd::AsyncFileDialog::new().set_title("Choose an editor application");
    #[cfg(target_os = "macos")]
    let dialog = dialog
        .set_directory("/Applications")
        .add_filter("Applications", &["app"]);
    #[cfg(target_os = "windows")]
    let dialog = dialog.add_filter("Applications", &["exe"]);

    let deliver = create_ext_action(scope, chosen);
    // Let the input event finish before rfd presents its native sheet on AppKit.
    std::thread::Builder::new()
        .name("editor-picker".into())
        .spawn(move || {
            let file = runtime.block_on(dialog.pick_file());
            deliver(file.map(|file| file.path().to_owned()));
        })
        .map(|_| ())
        .map_err(|error| format!("Could not open the application picker: {error}"))
}
