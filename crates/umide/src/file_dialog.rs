/// macOS-compatible file/folder dialog helpers.
///
/// On macOS, `floem::action::open_file` uses `rfd::FileDialog` (sync) which calls
/// `NSOpenPanel.runModal()`. This spins a nested AppKit event loop that conflicts with
/// floem/winit's event loop, causing the app to close immediately after the user picks a file.
///
/// The fix: use `rfd::AsyncFileDialog` which uses `beginSheetModalForWindow:completionHandler:`
/// — a callback-based approach that never runs a nested event loop. We drive the async future
/// on a background thread with a local tokio runtime so the main thread stays free.
///
/// On non-macOS platforms we delegate to floem's own `open_file` / `save_as`.
use std::path::PathBuf;

#[allow(unused_imports)]
use floem::{
    ext_event::create_ext_action,
    file::{FileDialogOptions, FileInfo},
    reactive::Scope,
};

/// Open a folder picker dialog. Replaces `floem::action::open_file` with
/// `FileDialogOptions::select_directories()` on macOS.
pub fn pick_folder(
    starting_directory: Option<PathBuf>,
    callback: impl Fn(Option<FileInfo>) + 'static,
) {
    #[cfg(target_os = "macos")]
    {
        let send = create_ext_action(Scope::new(), callback);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            let result = rt.block_on(async move {
                let mut dialog =
                    rfd::AsyncFileDialog::new().set_title("Choose a folder");
                if let Some(dir) = starting_directory {
                    dialog = dialog.set_directory(&dir);
                }
                dialog.pick_folder().await.map(|h| FileInfo {
                    path: vec![h.path().to_path_buf()],
                    format: None,
                })
            });
            send(result);
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut options = FileDialogOptions::new()
            .title("Choose a folder")
            .select_directories();
        if let Some(dir) = starting_directory {
            options = options.force_starting_directory(dir);
        }
        floem::action::open_file(options, callback);
    }
}

/// Open a single-file picker dialog. Replaces `floem::action::open_file` on macOS.
pub fn pick_file(callback: impl Fn(Option<FileInfo>) + 'static) {
    #[cfg(target_os = "macos")]
    {
        let send = create_ext_action(Scope::new(), callback);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            let result = rt.block_on(async {
                rfd::AsyncFileDialog::new()
                    .set_title("Choose a file")
                    .pick_file()
                    .await
                    .map(|h| FileInfo {
                        path: vec![h.path().to_path_buf()],
                        format: None,
                    })
            });
            send(result);
        });
    }
    #[cfg(not(target_os = "macos"))]
    floem::action::open_file(
        FileDialogOptions::new().title("Choose a file"),
        callback,
    );
}

/// Open a save-file dialog. Replaces `floem::action::save_as` on macOS.
pub fn save_file(
    default_name: Option<String>,
    callback: impl Fn(Option<FileInfo>) + 'static,
) {
    #[cfg(target_os = "macos")]
    {
        let send = create_ext_action(Scope::new(), callback);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            let result = rt.block_on(async move {
                let mut dialog = rfd::AsyncFileDialog::new().set_title("Save File");
                if let Some(name) = default_name {
                    dialog = dialog.set_file_name(&name);
                }
                dialog.save_file().await.map(|h| FileInfo {
                    path: vec![h.path().to_path_buf()],
                    format: None,
                })
            });
            send(result);
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut options = FileDialogOptions::new().title("Save File");
        if let Some(name) = default_name {
            options = options.default_name(name);
        }
        floem::action::save_as(options, callback);
    }
}
