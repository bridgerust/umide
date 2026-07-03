pub mod ai_assistant_view;
pub mod ai_sessions;
pub mod call_hierarchy_view;
pub mod data;
pub mod debug_view;
pub mod device_logs_stream;
pub mod device_logs_view;
pub mod document_symbol;
pub mod emulator_native_view;
pub mod emulator_stream;
pub mod emulator_view;
pub mod global_search_view;
pub mod implementation_view;
// The embedded iOS simulator stream is macOS-only (iOS Simulator + idb_companion
// are macOS-only); other OSes never reference it.
#[cfg(target_os = "macos")]
pub mod ios_stream;
pub mod kind;
pub mod plugin_view;
pub mod position;
pub mod problem_view;
pub mod references_view;
pub mod source_control_view;
pub mod style;
pub mod terminal_view;
mod video_view;
pub mod view;
