use umide_core::directory::Directory;
use tracing::level_filters::LevelFilter;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{filter::Targets, reload::Handle};

use crate::tracing::*;

#[inline(always)]
pub(super) fn logging() -> (Handle<Targets>, Vec<WorkerGuard>) {
    use tracing_subscriber::{filter, fmt, prelude::*, reload};

    let registry = tracing_subscriber::registry();
    let mut guards = Vec::new();

    // 1. Rolling File Appender (Original)
    let log_file_writer = match Directory::logs_directory()
        .and_then(|dir| {
            tracing_appender::rolling::Builder::new()
                .max_log_files(10)
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix("umide")
                .filename_suffix("log")
                .build(dir)
                .ok()
        })
        .map(tracing_appender::non_blocking)
    {
        Some((writer, guard)) => {
            guards.push(guard);
            Some(writer)
        },
        None => None,
    };

    // 2. Debug File Appender (User Request)
    let (debug_writer, debug_guard) = tracing_appender::non_blocking(
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open("/tmp/umide_debug.log")
            .unwrap_or_else(|_| std::fs::File::create("/dev/null").unwrap())
    );
    guards.push(debug_guard);

    // Filters
    let log_file_filter_targets = filter::Targets::new()
        .with_target("umide_app", LevelFilter::DEBUG)
        .with_target("umide_proxy", LevelFilter::DEBUG)
        .with_target("umide_core", LevelFilter::DEBUG)
        .with_target("umide_emulator", LevelFilter::TRACE) // Added emulator trace
        .with_default(LevelFilter::from_level(TraceLevel::INFO));
    
    let (log_file_filter, reload_handle) =
        reload::Subscriber::new(log_file_filter_targets);

    let console_filter_targets = std::env::var("LAPCE_LOG")
        .unwrap_or_default()
        .parse::<filter::Targets>()
        .unwrap_or_default();

    // Layers
    let debug_layer = tracing_subscriber::fmt::subscriber()
        .with_ansi(false)
        .with_writer(debug_writer)
        .with_filter(filter::Targets::new().with_default(LevelFilter::DEBUG));

    let rolling_layer = log_file_writer.map(|writer|
        tracing_subscriber::fmt::subscriber()
        .with_ansi(false)
        .with_writer(writer)
        .with_filter(log_file_filter));

    let console_layer = fmt::Subscriber::default()
        .with_line_number(true)
        .with_target(true)
        .with_thread_names(true)
        .with_filter(console_filter_targets);

    registry
        .with(rolling_layer)
        .with(debug_layer)
        .with(console_layer)
        .init();

    (reload_handle, guards)
}

pub(super) fn panic_hook() {
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let thread = thread.name().unwrap_or("main");
        let backtrace = backtrace::Backtrace::new();

        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s
        } else {
            "<unknown>"
        };

        match info.location() {
            Some(loc) => {
                trace!(
                    target: "lapce_app::panic_hook",
                    TraceLevel::ERROR,
                    "thread {thread} panicked at {} | file://./{}:{}:{}\n{:?}",
                    payload,
                    loc.file(), loc.line(), loc.column(),
                    backtrace,
                );
            }
            None => {
                trace!(
                    target: "lapce_app::panic_hook",
                    TraceLevel::ERROR,
                    "thread {thread} panicked at {}\n{:?}",
                    payload,
                    backtrace,
                );
            }
        }

        #[cfg(windows)]
        error_modal("Error", &info.to_string());
    }))
}

#[cfg(windows)]
pub(super) fn error_modal(title: &str, msg: &str) -> i32 {
    use std::{ffi::OsStr, iter::once, mem, os::windows::prelude::OsStrExt};

    use windows::Win32::UI::WindowsAndMessaging::{
        MB_ICONERROR, MB_SYSTEMMODAL, MessageBoxW,
    };

    let result: i32;

    let title = OsStr::new(title)
        .encode_wide()
        .chain(once(0u16))
        .collect::<Vec<u16>>();
    let msg = OsStr::new(msg)
        .encode_wide()
        .chain(once(0u16))
        .collect::<Vec<u16>>();
    unsafe {
        result = MessageBoxW(
            mem::zeroed(),
            msg.as_ptr(),
            title.as_ptr(),
            MB_ICONERROR | MB_SYSTEMMODAL,
        );
    }

    result
}
