mod session;

pub use session::capture_png;
pub(crate) use session::{device_command, tap};

use std::{cell::Cell, rc::Rc, time::Instant};

use floem::{
    event::{Event, EventListener},
    ext_event::create_ext_action,
    peniko::kurbo::Size,
    prelude::*,
    reactive::{Memo, Scope},
    style::TextOverflow,
    views::{dyn_container, dyn_stack, video_frame},
};
#[cfg(target_os = "macos")]
use umide_emulator::IosSimulator;
use umide_emulator::{AndroidEmulator, DeviceInfo, DevicePlatform, DeviceState};

use crate::design::{
    Glyph, MONO, UiText, accent, button, canvas, faint, icon, ink, line, mint_wash,
    muted, panel as panel_color, surface,
};
use crate::i18n::{label, message, tr};

type DeviceId = (String, String);

fn platform(device: &DeviceInfo) -> &'static str {
    match device.platform {
        DevicePlatform::Android => "android",
        DevicePlatform::Ios => "ios",
    }
}

fn target(device: &DeviceInfo) -> Option<DeviceId> {
    if device.state != DeviceState::Running {
        return None;
    }
    let id = match device.platform {
        DevicePlatform::Android => device.serial.clone()?,
        DevicePlatform::Ios => device.id.clone(),
    };
    Some((platform(device).to_owned(), id))
}

fn inventory() -> (Vec<DeviceInfo>, String) {
    let mut errors = Vec::new();
    let mut devices = AndroidEmulator::list_devices().unwrap_or_else(|e| {
        errors.push(format!("Android SDK: {e}"));
        Vec::new()
    });
    for device in &mut devices {
        if let Some(serial) = AndroidEmulator::running_serial(&device.id) {
            device.serial = Some(serial);
            device.state = DeviceState::Running;
        }
    }
    #[cfg(target_os = "macos")]
    match IosSimulator::list_devices() {
        Ok(mut sims) => devices.append(&mut sims),
        Err(e) => errors.push(format!("iOS Simulator: {e}")),
    }
    devices.sort_by_key(|d| {
        (
            d.state != DeviceState::Running,
            d.name.to_lowercase(),
            d.id.clone(),
        )
    });
    let message = errors.join(" · ");
    (devices, message)
}

#[derive(Clone, Copy)]
struct Panel {
    scope: Scope,
    selected: RwSignal<Option<DeviceId>>,
    devices: RwSignal<Vec<DeviceInfo>>,
    message: RwSignal<String>,
    busy: RwSignal<bool>,
}

impl Panel {
    fn refresh(self, open: Option<DeviceInfo>) {
        if self.busy.get_untracked() {
            return;
        }
        self.busy.set(true);
        self.message.set(match &open {
            Some(device) => format!("Starting {}…", device.name),
            None => "Finding devices…".into(),
        });
        let after = create_ext_action(
            self.scope,
            move |(devices, message, target_to_open): (
                Vec<DeviceInfo>,
                String,
                Option<DeviceId>,
            )| {
                let next = target_to_open.or_else(|| {
                    self.selected.get_untracked().filter(|id| {
                        devices.iter().any(|d| target(d).as_ref() == Some(id))
                    })
                });
                if next != self.selected.get_untracked() {
                    self.selected.set(next);
                }
                self.devices.set(devices);
                self.message.set(message);
                self.busy.set(false);
            },
        );
        std::thread::spawn(move || {
            let boot = open.as_ref().map(|d| session::boot(platform(d), &d.id));
            let (devices, mut message) = inventory();
            let mut next = None;
            if let Some(boot) = boot {
                match boot {
                    Ok(()) => {
                        next = open
                            .as_ref()
                            .and_then(|d| {
                                devices.iter().find(|a| {
                                    a.id == d.id && a.platform == d.platform
                                })
                            })
                            .and_then(target);
                        if next.is_none() {
                            message =
                                "Device is starting. Refresh when it is ready."
                                    .into();
                        }
                    }
                    Err(e) => message = e,
                }
            }
            after((devices, message, next));
        });
    }
}

fn platform_label(kind: DevicePlatform) -> &'static str {
    match kind {
        DevicePlatform::Android => "Android emulator",
        DevicePlatform::Ios => "iOS simulator",
    }
}

fn device_icon(kind: DevicePlatform, live: bool) -> impl IntoView {
    let glyph = match kind {
        DevicePlatform::Android => Glyph::Android,
        DevicePlatform::Ios => Glyph::Apple,
    };
    icon(glyph, 16., move || if live { accent() } else { muted() })
        .container()
        .style(move |s| {
            s.size(30., 30.)
                .items_center()
                .justify_center()
                .border_radius(8.)
                .background(if live { mint_wash() } else { surface() })
                .flex_shrink(0_f32)
        })
}

fn device_row(panel: Panel, device: DeviceInfo) -> impl IntoView {
    let running = target(&device).is_some();
    let starting = device.state == DeviceState::Starting;
    let name = Label::new(device.name.clone())
        .style(|s| s.width_full().font_size(13.).color(ink()).text_ellipsis());
    let detail = label(platform_label(device.platform))
        .style(|s| s.color(muted()).font_size(11.));
    let action = if running {
        "Open"
    } else if starting {
        "Starting"
    } else {
        "Start"
    };
    let action_color: fn() -> Color = if running { accent } else { muted };
    let trailing = Stack::horizontal((
        label(action).style(move |s| s.font_size(11.).color(action_color())),
        icon(
            if running { Glyph::Chevron } else { Glyph::Play },
            11.,
            action_color,
        ),
    ))
    .style(|s| s.items_center().gap(3.).flex_shrink(0_f32));

    Button::new(
        Stack::horizontal((
            device_icon(device.platform, running),
            Stack::vertical((name, detail)).style(|s| {
                s.min_width(0.)
                    .flex_grow(1_f32)
                    .flex_basis(0.)
                    .items_start()
                    .row_gap(2.)
            }),
            trailing,
        ))
        .style(|s| s.width_full().items_center().gap(9.)),
    )
    .action(move || {
        if panel.busy.get_untracked() || starting {
            return;
        }
        if let Some(id) = target(&device) {
            panel.selected.set(Some(id));
        } else {
            panel.refresh(Some(device.clone()));
        }
    })
    .style(move |s| {
        s.width_full()
            .padding_vert(8.)
            .padding_horiz(7.)
            .border(0.)
            .border_radius(7.)
            .background(panel_color())
            .hover(|s| s.background(surface()))
            .focus_visible(|s| s.outline(2.).outline_color(accent()))
            .set_disabled(panel.busy.get() || starting)
    })
}

fn device_list(panel: Panel) -> impl IntoView {
    let query = RwSignal::new(String::new());
    let filtered = move || {
        let query = query.get().trim().to_lowercase();
        panel.devices.with(|list| {
            list.iter()
                .filter(|d| {
                    d.name.to_lowercase().contains(&query)
                        || tr(platform_label(d.platform))
                            .to_lowercase()
                            .contains(&query)
                })
                .cloned()
                .collect::<Vec<_>>()
        })
    };
    let search = Stack::horizontal((
        icon(Glyph::Search, 14., muted),
        dyn_container(
            || tr("Filter devices…"),
            move |placeholder| {
                text_input(query)
                    .placeholder(placeholder)
                    .on_key_down(
                        Key::Named(NamedKey::Escape),
                        |m| m.is_empty(),
                        move |_| query.set(String::new()),
                    )
                    .style(|s| {
                        s.background(canvas())
                            .color(ink())
                            .class(floem::views::PlaceholderTextClass, |s| {
                                s.color(faint())
                            })
                            .border(0.)
                            .height(29.)
                            .min_width(0.)
                            .flex_grow(1_f32)
                            .padding_horiz(2.)
                            .font_size(12.)
                    })
            },
        )
        .style(|s| s.min_width(0.).flex_grow(1_f32)),
    ))
    .style(move |s| {
        s.width_full()
            .items_center()
            .gap(6.)
            .padding_horiz(9.)
            .border(1.)
            .border_color(line())
            .background(canvas())
            .border_radius(7.)
            .apply_if(
                panel.devices.with(|v| v.len() < 7) && query.with(String::is_empty),
                |s| s.hide(),
            )
    });

    let group = move |running: bool| {
        let devices = move || {
            filtered()
                .into_iter()
                .filter(|d| target(d).is_some() == running)
                .collect::<Vec<_>>()
        };
        let header = Stack::horizontal((
            label(if running { "Running" } else { "Available" })
                .style(|s| s.ui_bold().font_size(11.).color(muted())),
            Label::derived(move || devices().len().to_string())
                .style(|s| s.font_size(11.).color(faint())),
        ))
        .style(|s| {
            s.width_full()
                .items_center()
                .gap(6.)
                .padding_left(6.)
                .padding_top(9.)
                .padding_bottom(4.)
        });
        let items = dyn_stack(
            devices,
            |d| {
                (
                    d.id.clone(),
                    d.platform as u8,
                    d.name.clone(),
                    d.state as u8,
                    d.serial.clone(),
                )
            },
            move |device| device_row(panel, device),
        );
        Stack::vertical((header, items.style(|s| s.width_full().flex_col()))).style(
            move |s| s.width_full().apply_if(devices().is_empty(), |s| s.hide()),
        )
    };

    let empty = Stack::vertical((
        icon(Glyph::Device, 23., muted).container().style(|s| {
            s.size(46., 46.)
                .items_center()
                .justify_center()
                .border(1.)
                .border_color(line())
                .border_radius(12.)
                .background(surface())
        }),
        Label::derived(move || {
            if query.with(String::is_empty) {
                tr("No devices found")
            } else {
                tr("No matching devices")
            }
        })
        .style(|s| s.font_size(14.).ui_bold().color(ink())),
        Label::derived(move || {
            if !query.with(String::is_empty) {
                tr("Try a different device name or platform.")
            } else if cfg!(target_os = "macos") {
                tr("Create a device in Android Studio or Xcode and refresh.")
            } else {
                tr("Create a device in Android Studio and refresh.")
            }
        })
        .style(|s| {
            s.font_size(12.)
                .color(muted())
                .width_full()
                .text_overflow(TextOverflow::Wrap)
        }),
    ))
    .style(move |s| {
        s.width_full()
            .gap(9.)
            .padding(12.)
            .apply_if(!filtered().is_empty() || panel.busy.get(), |s| s.hide())
    });

    let rows = Stack::vertical((group(true), group(false), empty))
        .style(|s| s.width_full());
    Stack::vertical((
        search,
        Scroll::new(rows).style(|s| s.flex_grow(1_f32).min_height(0.).width_full()),
    ))
    .style(|s| s.width_full().min_height(0.).flex_grow(1_f32).gap(5.))
}

pub fn device_panel(selected: RwSignal<Option<DeviceId>>) -> impl IntoView {
    let panel = Panel {
        scope: Scope::current(),
        selected,
        devices: RwSignal::new(Vec::new()),
        message: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
    };
    panel.refresh(None);
    let header = Stack::horizontal((
        icon(Glyph::Device, 17., muted),
        label("Devices")
            .style(|s| s.ui_bold().font_size(16.).color(ink()).flex_grow(1_f32)),
        Button::new(
            Stack::horizontal((
                icon(Glyph::Refresh, 12., muted),
                label("Refresh").style(|s| s.color(muted()).font_size(11.)),
            ))
            .style(|s| s.items_center().gap(5.)),
        )
        .action(move || panel.refresh(None))
        .style(move |s| {
            s.border(0.)
                .border_radius(5.)
                .background(panel_color())
                .padding_horiz(6.)
                .padding_vert(5.)
                .hover(|s| s.background(surface()))
                .focus_visible(|s| s.outline(2.).outline_color(accent()))
                .set_disabled(panel.busy.get())
        }),
    ))
    .style(|s| s.width_full().items_center().gap(8.));
    let notice = Label::derived(move || panel.message.with(|text| message(text)))
        .style(move |s| {
            s.font_size(12.)
                .color(muted())
                .width_full()
                .text_overflow(TextOverflow::Wrap)
                .apply_if(panel.message.with(String::is_empty), |s| s.hide())
        });
    let content = dyn_container(
        move || selected.get(),
        move |open| match open {
            Some((platform, id)) => preview_view(panel, platform, id).into_any(),
            None => device_list(panel).into_any(),
        },
    )
    .style(|s| {
        s.width_full()
            .min_height(0.)
            .flex_grow(1_f32)
            .flex_basis(0.)
    });
    Stack::vertical((header, notice, content)).style(|s| {
        s.width_full()
            .min_height(0.)
            .flex_grow(1_f32)
            .flex_basis(0.)
            .padding(16.)
            .gap(12.)
    })
}

fn preview_view(panel: Panel, platform: String, id: String) -> impl IntoView {
    let selection = (platform.clone(), id.clone());
    let kind = if platform == "android" {
        DevicePlatform::Android
    } else {
        DevicePlatform::Ios
    };
    let name = Label::derived(move || {
        panel.devices.with(|devices| {
            devices
                .iter()
                .find(|d| target(d).as_ref() == Some(&selection))
                .map(|d| d.name.clone())
                .unwrap_or_else(|| selection.1.clone())
        })
    })
    .style(|s| {
        s.ui_bold()
            .font_size(13.)
            .color(ink())
            .width_full()
            .text_ellipsis()
    });
    let selected = Stack::horizontal((
        device_icon(kind, true),
        Stack::vertical((
            name,
            label(platform_label(kind)).style(|s| s.color(muted()).font_size(11.)),
        ))
        .style(|s| s.flex_grow(1_f32).min_width(0.).flex_basis(0.).row_gap(2.)),
        button("Change", move || panel.selected.set(None))
            .style(|s| s.font_size(11.).padding_horiz(8.).padding_vert(4.)),
    ))
    .style(|s| {
        s.width_full()
            .items_center()
            .gap(9.)
            .padding(9.)
            .background(surface())
            .border_radius(8.)
    });

    let session::Preview {
        frame,
        status,
        control_status,
        gestures,
    } = session::start(platform, id);
    let stage_size = RwSignal::new(Size::ZERO);
    let screen = frame.clone();
    let dimensions =
        Memo::new(move |_| screen.get().map(|f| (f.image.width, f.image.height)));
    let size = Rc::new(Cell::new(Size::ZERO));
    let resize = size.clone();
    let press = RwSignal::new(None);
    let render = frame.clone();
    let position = move |e: &Event| {
        let frame = frame.get_untracked()?;
        let (w, h) = frame.input_size?;
        let p = e.point()?;
        let bounds = size.get();
        map_point(p.x, p.y, bounds.width, bounds.height, w, h)
    };
    let position_up = position.clone();
    let image = video_frame(move || render.get().map(|f| f.image))
        .on_resize(move |rect| resize.set(rect.size()))
        .on_event_stop(EventListener::PointerDown, move |e| {
            press.set(position(e).map(|p| (p, Instant::now())))
        })
        .on_event_stop(EventListener::PointerUp, move |e| {
            let previous = press.get_untracked();
            press.set(None);
            if let (Some((start, at)), Some(end)) = (previous, position_up(e)) {
                let _ = gestures.try_send(session::Gesture {
                    start,
                    end,
                    duration: at.elapsed(),
                });
            }
        })
        .style(move |s| {
            let available = stage_size.get();
            let (w, h) = dimensions.get().unwrap_or((1, 1));
            let scale = ((available.width - 14.).max(0.) / w.max(1) as f64)
                .min((available.height - 14.).max(0.) / h.max(1) as f64);
            s.size((scale * w as f64).floor(), (scale * h as f64).floor())
                .background(canvas())
        });
    let screen = image.container().style(move |s| {
        s.border(1.)
            .border_color(line())
            .padding(2.)
            .border_radius(5.)
            .background(canvas())
            .apply_if(dimensions.get().is_none(), |s| s.hide())
    });
    let placeholder = Stack::vertical((
        icon(Glyph::Device, 30., faint),
        label("Waiting for screen…").style(|s| s.color(muted()).font_size(12.)),
    ))
    .style(move |s| {
        s.items_center()
            .gap(8.)
            .apply_if(dimensions.get().is_some(), |s| s.hide())
    });
    let stage = Stack::vertical((screen, placeholder))
        .on_resize(move |r| {
            if stage_size.get_untracked() != r.size() {
                stage_size.set(r.size())
            }
        })
        .style(|s| {
            s.width_full()
                .flex_grow(1_f32)
                .flex_basis(0.)
                .min_height(0.)
                .items_center()
                .justify_center()
                .border(1.)
                .border_color(line())
                .border_radius(11.)
                .background(canvas())
        });

    let stream_status = status.clone();
    let live = Memo::new(move |_| {
        dimensions.get().is_some() && stream_status.get().is_empty()
    });
    let stream = Stack::horizontal((
        Empty::new().style(move |s| {
            s.size(6., 6.).border_radius(3.).background(if live.get() {
                accent()
            } else {
                faint()
            })
        }),
        Label::derived(move || {
            if live.get() {
                tr("Live preview")
            } else {
                tr("Screen preview")
            }
        })
        .style(|s| s.font_size(12.).ui_bold().color(muted()).flex_grow(1_f32)),
        Label::derived(move || {
            dimensions
                .get()
                .map(|(w, h)| format!("{w} × {h}"))
                .unwrap_or_default()
        })
        .style(|s| {
            s.color(faint())
                .font_size(11.)
                .font_family(MONO.to_string())
        }),
    ))
    .style(|s| s.width_full().gap(6.).items_center());
    let notice = Memo::new(move |_| {
        let state = status.get();
        if !state.is_empty() {
            return if state == "Connecting…" {
                String::new()
            } else {
                message(&state)
            };
        }
        let controls = control_status.get();
        if controls.is_empty() && live.get() {
            tr("Click to tap · Drag to swipe").to_owned()
        } else {
            message(&controls)
        }
    });
    let notice = Label::derived(move || notice.get()).style(move |s| {
        s.width_full()
            .font_size(11.)
            .color(muted())
            .text_overflow(TextOverflow::Wrap)
            .apply_if(notice.with(String::is_empty), |s| s.hide())
    });
    Stack::vertical((selected, stream, stage, notice))
        .style(|s| s.size_full().min_height(0.).gap(10.))
}

fn map_point(
    x: f64,
    y: f64,
    vw: f64,
    vh: f64,
    w: u32,
    h: u32,
) -> Option<(i32, i32)> {
    if w == 0 || h == 0 || vw <= 0. || vh <= 0. {
        return None;
    }
    let scale = (vw / w as f64).min(vh / h as f64);
    let x = (x - (vw - scale * w as f64) * 0.5) / scale;
    let y = (y - (vh - scale * h as f64) * 0.5) / scale;
    if x < 0. || y < 0. || x >= w as f64 || y >= h as f64 {
        return None;
    }
    Some((x as i32, y as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taps_respect_letterboxing_and_orientation() {
        assert_eq!(map_point(150., 200., 300., 400., 100, 200), Some((50, 100)));
        assert_eq!(map_point(40., 200., 300., 400., 100, 200), None);
        assert_eq!(map_point(150., 40., 300., 400., 200, 100), None);
        assert_eq!(map_point(299., 274., 300., 400., 200, 100), Some((199, 99)));
    }

    /// Opt-in integration check for a disposable local simulator or emulator.
    #[test]
    #[ignore = "requires UMIDE_TEST_PLATFORM and UMIDE_TEST_DEVICE"]
    fn selected_device_emits_a_real_preview() {
        use std::{thread, time::Duration};
        let platform = std::env::var("UMIDE_TEST_PLATFORM").unwrap();
        let id = std::env::var("UMIDE_TEST_DEVICE").unwrap();
        let (devices, errors) = inventory();
        assert!(
            devices
                .iter()
                .any(|d| target(d) == Some((platform.clone(), id.clone()))),
            "{errors}"
        );
        let image =
            image::load_from_memory(&capture_png(&platform, &id).unwrap()).unwrap();
        assert!(image.width() > 0 && image.height() > 0);
        let preview = session::start(platform.clone(), id);
        let start = Instant::now();
        let frame = loop {
            if let Some(frame) = preview.frame.get_untracked() {
                break frame;
            }
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "{}",
                preview.status.get_untracked()
            );
            thread::sleep(Duration::from_millis(100));
        };
        assert_eq!(
            frame.image.data.len(),
            frame.image.width as usize * frame.image.height as usize * 4
        );
        if platform == "android"
            || std::env::var_os("UMIDE_TEST_IOS_INPUT").is_some()
        {
            let (w, h) = frame.input_size.unwrap();
            let w = w as i32;
            let h = h as i32;
            if platform == "ios" {
                let start = Instant::now();
                while !preview.control_status.get_untracked().is_empty() {
                    assert!(
                        start.elapsed() < Duration::from_secs(25),
                        "{}",
                        preview.control_status.get_untracked()
                    );
                    thread::sleep(Duration::from_millis(100));
                }
            }
            let swipe_x = w * 7 / 8;
            preview
                .gestures
                .try_send(session::Gesture {
                    start: (swipe_x, 2),
                    end: (swipe_x, h * 3 / 4),
                    duration: Duration::from_millis(300),
                })
                .unwrap();
            let start = Instant::now();
            loop {
                thread::sleep(Duration::from_millis(150));
                if preview
                    .frame
                    .get_untracked()
                    .is_some_and(|f| f.image.data != frame.image.data)
                {
                    break;
                }
                assert!(
                    start.elapsed() < Duration::from_secs(12),
                    "No frame changed after swipe"
                );
            }
        }
    }
}
