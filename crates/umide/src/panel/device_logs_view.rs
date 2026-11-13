//! Device Logs panel — native device logs (Android logcat / iOS unified log)
//! streamed live inside UMIDE, so the everyday loop never leaves the editor for
//! Android Studio's Logcat or Xcode's console.
//!
//! The stream backends live in [`super::device_logs_stream`]: Android via
//! `start_logcat_stream` (adb logcat), iOS via `start_ios_log_stream` (simctl
//! log stream). Both deliver batches of [`LogLine`] into a signal on the same
//! contract, so this view treats the two platforms identically. The panel
//! follows the most recent Run target or the device shown in the Emulator panel.

use std::{cell::Cell, rc::Rc};

use floem::{
    View,
    kurbo::Point,
    reactive::{Effect, RwSignal, SignalGet, SignalUpdate, SignalWith},
    style::CursorStyle,
    views::{Decorators, Label, Scroll, Stack, dyn_stack},
};
use umide_emulator::{DevicePlatform, DeviceState};

use super::{
    device_logs_stream::{
        LogBatch, LogLine, LogSeverity, LogcatHandle, start_ios_log_stream,
        start_logcat_stream,
    },
    position::PanelPosition,
};
use crate::{config::color::UmideColor, window_tab::WindowTabData};

/// Cap the retained buffer so a chatty device can't grow the view unbounded.
/// The backend already batches (256 lines / 50 ms), so this is a display cap,
/// not a drop policy on the stream. (A future pass can swap the `dyn_stack` for
/// a windowed `virtual_stack` if we want to keep more scrollback cheaply.)
const MAX_LINES: usize = 1000;

/// The theme color a line is painted in, by severity.
fn severity_color(severity: LogSeverity) -> &'static str {
    match severity {
        LogSeverity::Error | LogSeverity::Fatal => UmideColor::LAPCE_ERROR,
        LogSeverity::Warn => UmideColor::LAPCE_WARN,
        LogSeverity::Info => UmideColor::PANEL_FOREGROUND,
        LogSeverity::Verbose | LogSeverity::Debug => {
            UmideColor::PANEL_FOREGROUND_DIM
        }
    }
}

pub fn device_logs_panel(
    window_tab_data: Rc<WindowTabData>,
    _position: PanelPosition,
) -> impl View {
    let config = window_tab_data.common.config;
    let active_device = window_tab_data.panel.active_device;
    let run_device = window_tab_data.panel.run_log_device;

    // Retained lines as `(id, line)` so `dyn_stack` has a stable key even as the
    // buffer drains from the front; `next_id` is the monotonic id source.
    let lines: RwSignal<Vec<(u64, LogLine)>> = RwSignal::new(Vec::new());
    let next_id = RwSignal::new(0u64);
    // The backend delivers one batch per flush here.
    let batch: RwSignal<Option<LogBatch>> = RwSignal::new(None);
    let generation = Rc::new(Cell::new(0_u64));
    // The live stream child; dropping the handle kills it (adb/simctl exits).
    let handle: RwSignal<Option<LogcatHandle>> = RwSignal::new(None);
    // What the header shows: the streamed device, or why there's nothing.
    let header =
        RwSignal::new(String::from("No running device — start one in Emulator"));

    // Switch streams when Run selects a target or the viewed device changes.
    let stream_generation = generation.clone();
    Effect::new(move |_| {
        let pinned = run_device.get();
        let is_pinned = pinned.is_some();
        let device = pinned.or_else(|| active_device.get());
        let current_generation = stream_generation.get().wrapping_add(1);
        stream_generation.set(current_generation);
        handle.set(None);
        batch.set(None);
        lines.set(Vec::new());
        next_id.set(0);

        match device {
            Some(device) if device.state == DeviceState::Running => {
                let started = match device.platform {
                    DevicePlatform::Android => {
                        device.serial.as_deref().and_then(|serial| {
                            start_logcat_stream(serial, batch, current_generation)
                        })
                    }
                    DevicePlatform::Ios => {
                        start_ios_log_stream(&device.id, batch, current_generation)
                    }
                };
                let platform = crate::project::platform_label(device.platform);
                let source = if is_pinned { " · Run target" } else { "" };
                header.set(if started.is_some() {
                    format!("{} · {platform}{source}", device.name)
                } else {
                    format!(
                        "{} · {platform} — couldn't start log stream",
                        device.name
                    )
                });
                handle.set(started);
            }
            _ => {
                header
                    .set(String::from("No running device — start one in Emulator"));
            }
        }
    });

    // Append each delivered batch, keeping the buffer capped at `MAX_LINES`.
    Effect::new(move |_| {
        if let Some((source, new_lines)) = batch.get() {
            if source != generation.get() || new_lines.is_empty() {
                return;
            }
            lines.update(|buf| {
                for line in new_lines {
                    let id = next_id.get_untracked();
                    next_id.set(id + 1);
                    buf.push((id, line));
                }
                if buf.len() > MAX_LINES {
                    buf.drain(0..buf.len() - MAX_LINES);
                }
            });
        }
    });

    let header_bar = Stack::new((
        Label::derived(move || {
            let count = lines.with(|buf| buf.len());
            format!("{}  ·  {count} lines", header.get())
        })
        .style(move |s| {
            s.flex_grow(1.0)
                .min_width(0.0)
                .font_size(11.0)
                .text_ellipsis()
                .color(config.get().color(UmideColor::PANEL_FOREGROUND_DIM))
        }),
        Label::new("Follow Emulator")
            .on_click_stop(move |_| run_device.set(None))
            .style(move |s| {
                s.apply_if(run_device.get().is_none(), |s| s.hide())
                    .padding_horiz(8.0)
                    .font_size(11.0)
                    .cursor(CursorStyle::Pointer)
                    .color(config.get().color(UmideColor::PANEL_FOREGROUND))
            }),
        Label::new("Clear")
            .on_click_stop(move |_| lines.set(Vec::new()))
            .style(move |s| {
                s.padding_horiz(8.0)
                    .padding_vert(2.0)
                    .font_size(11.0)
                    .border_radius(6.0)
                    .cursor(CursorStyle::Pointer)
                    .color(config.get().color(UmideColor::PANEL_FOREGROUND))
                    .hover(|s| {
                        s.background(
                            config.get().color(UmideColor::PANEL_CURRENT_BACKGROUND),
                        )
                    })
            }),
    ))
    .style(move |s| {
        s.width_full()
            .items_center()
            .padding_horiz(8.0)
            .padding_vert(4.0)
            .border_bottom(1.0)
            .border_color(config.get().color(UmideColor::LAPCE_BORDER))
    });

    let log_list = Scroll::new(
        dyn_stack(
            move || lines.get(),
            |(id, _)| *id,
            move |(_, line)| {
                let severity = line.severity;
                Label::new(line.text).style(move |s| {
                    s.width_full()
                        .min_width(0.0)
                        .padding_horiz(8.0)
                        .font_size(12.0)
                        .font_family(config.get().editor.font_family.clone())
                        .color(config.get().color(severity_color(severity)))
                })
            },
        )
        .style(|s| s.flex_col().width_full().min_width(0.0)),
    )
    .style(|s| s.flex_grow(1.0).width_full().min_height(0.0))
    // Tail the log: scroll to the very bottom as batches arrive. The y must be
    // LARGE BUT FINITE: the scroll clamp adds the viewport height to it, and
    // `f64::MAX + h == inf` poisons the viewport rect — the clamp never
    // converges, which both breaks the tail AND re-schedules layout every
    // frame (a self-sustaining ~100% CPU repaint storm; shipped in v0.3.2 as
    // "the app freezes"). 1e9 px is beyond any real log buffer and clamps to
    // the true bottom in one step.
    .scroll_to(move || {
        lines.with(|buf| buf.len());
        Some(Point::new(0.0, 1e9))
    });

    Stack::new((header_bar, log_list))
        .style(|s| s.flex_col().size_pct(100.0, 100.0).min_width(0.0))
        .debug_name("Device Logs Panel")
}
