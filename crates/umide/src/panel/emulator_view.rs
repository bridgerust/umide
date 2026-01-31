use std::{rc::Rc, sync::Arc};
use tracing::{info, error};
use floem::{
    View, prelude::{SignalGet, SignalUpdate, SignalWith}, reactive::{create_rw_signal, create_effect}, 
    views::{Decorators, label, scroll, stack, dyn_stack, container, h_stack}, 
};

use crate::{
    app::clickable_icon,
    panel::{position::PanelPosition, view::PanelBuilder},
    window_tab::WindowTabData,
    config::{icon::LapceIcons, color::LapceColor},
};
use umide_emulator::{list_all_devices, launch_device, stop_device, DeviceInfo, DevicePlatform, DeviceState};

pub fn emulator_panel(
    window_tab_data: Rc<WindowTabData>,
    position: PanelPosition,
) -> impl View {
    let config = window_tab_data.common.config;
    let devices = create_rw_signal(Vec::<DeviceInfo>::new());
    let active_platform = create_rw_signal(DevicePlatform::Android);
    let running_device = create_rw_signal(None::<DeviceInfo>);

    // Effect to fetch devices
    create_effect(move |_| {
        let dev_list = list_all_devices();
        // Update running_device if any device is Running
        if let Some(running) = dev_list.iter().find(|d| d.state == DeviceState::Running) {
            running_device.set(Some(running.clone()));
        }
        devices.set(dev_list);
    });

    let emulator_frame = window_tab_data.panel.emulator_frame;
    {
        create_effect(move |_| {
            if running_device.get().is_some() {
                let (tx, rx) = std::sync::mpsc::channel::<umide_emulator::decoder::DecodedFrame>();
                let frame_signal = floem::ext_event::create_signal_from_channel(rx);
                
                create_effect(move |_| {
                    frame_signal.with(|frame: &Option<umide_emulator::decoder::DecodedFrame>| {
                        if let Some(frame) = frame {
                            // println!("UI: Received Frame {}x{}", frame.width, frame.height);
                            emulator_frame.set(Some(Arc::new(frame.clone())));
                        }
                    });
                });

                std::thread::spawn(move || {
                    // VERIFICATION HARNESS (Option A)
                    // Check for test file
                    let test_file = "/tmp/test.h264";
                    
                    if std::path::Path::new(test_file).exists() {
                        info!("Found test file: {}", test_file);
                        if let Ok(mut source) = umide_emulator::video::h264_source::H264FileSource::new(test_file) {
                            #[cfg(target_os = "macos")]
                            {
                                use umide_emulator::decoder::VideoDecoder;
                                
                                info!("Initializing Hardware Decoder...");
                                // Wrap in block to catch errors
                                match umide_emulator::video::macos_hardware::VideoToolboxDecoder::new() {
                                    Ok(mut decoder) => {
                                        loop {
                                            if let Some(nalu) = source.next_nalu() {
                                                if let Ok(decoded_frames) = decoder.decode_frame(nalu) {
                                                    for frame in decoded_frames {
                                                        info!("Got Hardware Frame: {}x{}", frame.width, frame.height);
                                                        if tx.send(frame).is_err() { return; }
                                                    }
                                                } else {
                                                    info!("Decode failed");
                                                }
                                                std::thread::sleep(std::time::Duration::from_millis(16)); // ~60fps
                                            } else {
                                                info!("EOF, restarting loop");
                                                source = umide_emulator::video::h264_source::H264FileSource::new(test_file).unwrap();
                                            }   
                                        }
                                    }
                                    Err(e) => error!("Failed to create decoder: {:?}", e),
                                }
                            }
                            #[cfg(not(target_os = "macos"))]
                            {
                                info!("H264 verification only supported on macOS for now");
                            }
                        }
                    }

                    // FALLBACK: Noise loop (Updated for visibility)
                    info!("Running Noise Loop (No test file found at {})", test_file);
                    let mut i: u8 = 0;
                    loop {
                        let mut data = vec![0u8; 200 * 400 * 4];
                        // Draw BLUE gradients
                        for y in 0..400 {
                            for x in 0..200 {
                                let idx = (y * 200 + x) * 4;
                                data[idx] = 0; // R
                                data[idx + 1] = 0; // G
                                data[idx + 2] = (x as u8).wrapping_add(i); // B
                                data[idx + 3] = 255;
                            }
                        }
                        let frame = umide_emulator::decoder::DecodedFrame {
                            width: 200,
                            height: 400,
                            frame: umide_emulator::decoder::GpuFrame::Software(Arc::new(data), 200 * 4, 400),
                        };
                        if tx.send(frame).is_err() { break; }
                        i = i.wrapping_add(5);
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                });
            } else {
                emulator_frame.set(None);
            }
        });
    }

    let device_item = {
        let running_device = running_device.clone();
        move |device: DeviceInfo| {
            let device_cloned_start = device.clone();
            let device_cloned_stop = device.clone();
            let device_cloned_running = device.clone();
            let name = device.name.clone();
            let is_running = device.state == DeviceState::Running;
            let is_starting = device.state == DeviceState::Starting;
            let is_disconnected = device.state == DeviceState::Disconnected;

            stack((
                label(move || name.clone())
                    .style(|s| s.flex_grow(1.0).padding_horiz(5.0)),
                clickable_icon(
                    || LapceIcons::START,
                    move || {
                        let _ = launch_device(&device_cloned_start);
                        // For testing: simulate transitioning to running
                        running_device.set(Some(device_cloned_running.clone()));
                    },
                    || false,
                    move || !is_disconnected,
                    || "Start",
                    config,
                ),
                clickable_icon(
                    || LapceIcons::DEBUG_STOP,
                    move || {
                        let _ = stop_device(&device_cloned_stop);
                        running_device.set(None);
                    },
                    || false,
                    move || !is_running,
                    || "Stop",
                    config,
                ),
                label(move || {
                    if is_starting {
                        "Starting..."
                    } else {
                        ""
                    }
                })
                .style(|s| s.padding_horiz(5.0).font_size(10.0)),
            ))
            .style(|s| {
                s.width_full()
                    .flex_row()
                    .items_center()
                    .padding_vert(5.0)
                    .border_bottom(1.0)
            })
        }
    };

    let tab_button = move |platform: DevicePlatform, label_str: &'static str| {
        let platform_cloned = platform.clone();
        let is_active = move || active_platform.get() == platform_cloned;
        container(
            label(move || label_str)
                .style(move |s| {
                    s.padding_vert(6.0)
                        .flex_grow(1.0)
                        .items_center()
                        .justify_center()
                })
        )
        .on_click_stop(move |_| {
            active_platform.set(platform.clone());
        })
        .style(move |s| {
            let config = config.get();
            s.flex_grow(1.0)
                .items_center()
                .justify_center()
                .cursor(floem::style::CursorStyle::Pointer)
                .apply_if(is_active(), |s| {
                    s.border_bottom(2.0)
                        .border_color(config.color(LapceColor::LAPCE_TAB_ACTIVE_UNDERLINE))
                })
                .hover(|s| {
                    s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
                })
        })
    };

    PanelBuilder::new(config, position)
        .add(
            "Emulators",
            stack((
                h_stack((
                    tab_button(DevicePlatform::Android, "Android"),
                    tab_button(DevicePlatform::Ios, "iOS"),
                ))
                .style(move |s| {
                    s.width_full()
                        .border_bottom(1.0)
                        .border_color(config.get().color(LapceColor::LAPCE_BORDER))
                        .apply_if(running_device.get().is_some(), |s| s.hide())
                }),
                scroll(
                    stack((
                        dyn_stack(
                            move || {
                                if running_device.get().is_some() {
                                    return Vec::new();
                                }
                                let platform = active_platform.get();
                                devices.get().into_iter().filter(|d| d.platform == platform).collect::<Vec<_>>()
                            },
                            |d| format!("{}-{}", d.id, d.state as u32),
                            move |d| device_item(d)
                        ).style(|s| s.flex_col().width_full()),
                        stack((
                            {
                                let running_device = running_device.clone();
                                crate::panel::emulator_host_view::emulator_host_view(window_tab_data.panel.emulator_frame, move |x, y| {
                                    if let Some(device) = running_device.get_untracked() {
                                        println!("Sending touch to {}: ({}, {})", device.name, x, y);
                                    }
                                })
                            },
                            clickable_icon(
                                || LapceIcons::CLOSE,
                                move || {
                                    running_device.set(None);
                                },
                                || false,
                                || false,
                                || "Back to list",
                                config,
                            ).style(|s| s.absolute().margin_left(200.0).margin_top(10.0))
                        ))
                        .style(move |s| {
                            s.flex_col()
                                .width_full()
                                .height(500.0)
                                .apply_if(running_device.get().is_none(), |s| s.hide())
                        })
                    ))
                    .style(|s| s.flex_col().width_full().padding(10.0))
                )
                .style(|s| s.flex_grow(1.0).width_full()),
            ))
            .style(|s| s.flex_col().size_full()),
            window_tab_data.panel.section_open(crate::panel::data::PanelSection::Process),
        )
        .build()
}
