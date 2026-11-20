mod session;

pub use session::capture_png;

use std::{cell::Cell, rc::Rc, time::Instant};

use floem::{
    event::{Event, EventListener},
    ext_event::create_ext_action,
    peniko::{Color, kurbo::Size},
    prelude::*,
    reactive::Scope,
    views::{dyn_container, dyn_stack, video_frame},
};
#[cfg(target_os = "macos")]
use umide_emulator::IosSimulator;
use umide_emulator::{AndroidEmulator, DeviceInfo, DevicePlatform, DeviceState};

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
    let message = if devices.is_empty() && errors.is_empty() {
        "Create a virtual device in Android Studio or Xcode, then refresh.".into()
    } else {
        errors.join(" · ")
    };
    (devices, message)
}

#[derive(Clone, Copy)]
struct Panel {
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
            Scope::current(),
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

pub fn device_panel(selected: RwSignal<Option<DeviceId>>) -> impl IntoView {
    let panel = Panel {
        selected,
        devices: RwSignal::new(Vec::new()),
        message: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
    };
    panel.refresh(None);
    let header = Stack::horizontal((
        Label::new("Devices")
            .style(|s| s.font_bold().font_size(16.).flex_grow(1_f32)),
        crate::button("Refresh", move || panel.refresh(None)),
    ))
    .style(|s| s.width_full().items_center());
    let list = Scroll::new(
        dyn_stack(
            move || panel.devices.get(),
            |d| (platform(d), d.id.clone(), d.state as u32, d.serial.clone()),
            move |device| {
                let view_id = target(&device);
                let badge = if device.platform == DevicePlatform::Android {
                    "Android"
                } else {
                    "iOS"
                };
                let action = if view_id.is_some() { "View" } else { "Start" };
                Button::new(
                    Stack::vertical((
                        Label::new(device.name.clone()).style(|s| {
                            s.font_size(13.)
                                .color(crate::INK)
                                .width_full()
                                .text_ellipsis()
                        }),
                        Label::new(format!("{badge} · {action}")).style(|s| {
                            s.font_size(11.).color(crate::MUTED).width_full()
                        }),
                    ))
                    .style(|s| s.width_full().items_start().row_gap(3.)),
                )
                .action(move || {
                    if panel.busy.get_untracked() {
                        return;
                    }
                    if let Some(id) = target(&device) {
                        if selected.get_untracked().as_ref() != Some(&id) {
                            selected.set(Some(id));
                        }
                    } else {
                        panel.refresh(Some(device.clone()));
                    }
                })
                .style(move |s| {
                    s.width_full()
                        .items_start()
                        .justify_start()
                        .background(crate::CANVAS)
                        .border(1.)
                        .border_color(crate::LINE)
                        .border_radius(7.)
                        .padding_vert(8.)
                        .padding_horiz(11.)
                        .hover(|s| s.background(crate::PANEL))
                        .apply_if(
                            view_id.is_some() && selected.get() == view_id,
                            |s| {
                                s.border_color(crate::ACCENT)
                                    .background(crate::PANEL)
                            },
                        )
                })
            },
        )
        .style(|s| s.width_full().flex_col().row_gap(5.)),
    )
    .style(move |s| {
        s.width_full()
            .min_height(0.)
            .apply_if(selected.get().is_some(), |s| s.height(135.))
            .apply_if(selected.get().is_none(), |s| {
                s.flex_grow(1_f32).flex_basis(0.)
            })
    });
    let preview = dyn_container(
        move || selected.get(),
        move |id| match id {
            Some((platform, id)) => preview_view(platform, id, selected).into_any(),
            None => Empty::new().into_any(),
        },
    )
    .style(move |s| {
        s.width_full()
            .min_height(0.)
            .apply_if(selected.get().is_some(), |s| {
                s.flex_grow(1_f32).flex_basis(0.)
            })
    });
    Stack::vertical((
        header,
        Label::derived(move || panel.message.get()).style(move |s| {
            s.font_size(12.)
                .width_full()
                .apply_if(panel.message.get().is_empty(), |s| s.hide())
        }),
        list,
        preview,
    ))
    .style(|s| {
        s.size_full()
            .min_height(0.)
            .min_width(0.)
            .row_gap(9.)
            .padding(12.)
    })
}

fn preview_view(
    platform: String,
    id: String,
    selected: RwSignal<Option<DeviceId>>,
) -> impl IntoView {
    let session::Preview {
        frame,
        status,
        control_status,
        gestures,
    } = session::start(platform, id);
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
        .style(|s| {
            s.background(Color::from_rgb8(17, 20, 25))
                .width_full()
                .flex_grow(1_f32)
                .flex_basis(0.)
                .min_height(0.)
        });
    Stack::vertical((
        image,
        Label::derived(move || {
            let state = status.get();
            if state.is_empty() {
                control_status.get()
            } else {
                state
            }
        })
        .style(|s| s.font_size(11.).width_full()),
        crate::button("Close preview", move || selected.set(None)),
    ))
    .style(|s| s.size_full().min_height(0.))
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
