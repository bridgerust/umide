use floem::{
    prelude::*,
    reactive::{create_memo, create_effect, create_rw_signal},
    peniko::{Blob, Color, ImageFormat, Image},
    views::{canvas, container, label, stack, Decorators},
    kurbo::Rect,
};
use floem_renderer::Img;
use umide_emulator::decoder::DecodedFrame;
use std::sync::Arc;

/// Calculate the destination rectangle that preserves aspect ratio
fn calculate_aspect_fit(image_width: u32, image_height: u32, container_width: f64, container_height: f64) -> Rect {
    if image_width == 0 || image_height == 0 || container_width <= 0.0 || container_height <= 0.0 {
        return Rect::ZERO;
    }
    
    let image_aspect = image_width as f64 / image_height as f64;
    let container_aspect = container_width / container_height;
    
    let (draw_width, draw_height) = if image_aspect > container_aspect {
        let w = container_width;
        let h = container_width / image_aspect;
        (w, h)
    } else {
        let h = container_height;
        let w = container_height * image_aspect;
        (w, h)
    };
    
    let x = (container_width - draw_width) / 2.0;
    let y = (container_height - draw_height) / 2.0;
    
    Rect::new(x, y, x + draw_width, y + draw_height)
}

/// Map click coordinates from canvas space to device coordinates
fn map_to_device_coords(
    click_x: f64, click_y: f64,
    image_rect: Rect,
    device_width: u32, device_height: u32
) -> Option<(i32, i32)> {
    if image_rect.width() <= 0.0 || image_rect.height() <= 0.0 {
        return None;
    }
    
    if click_x < image_rect.x0 || click_x > image_rect.x1 ||
       click_y < image_rect.y0 || click_y > image_rect.y1 {
        return None;
    }
    
    let norm_x = (click_x - image_rect.x0) / image_rect.width();
    let norm_y = (click_y - image_rect.y0) / image_rect.height();
    
    let device_x = (norm_x * device_width as f64) as i32;
    let device_y = (norm_y * device_height as f64) as i32;
    
    Some((device_x, device_y))
}

pub fn emulator_host_view(
    frame_signal: impl SignalGet<Option<Arc<DecodedFrame>>> + Copy + 'static,
    on_click: impl Fn(f64, f64) + 'static,
) -> impl View {
    // Track frame counter for cache invalidation
    let frame_counter = create_rw_signal(0u64);
    
    // Store the last image rect for touch mapping
    let last_image_rect = create_rw_signal(Rect::ZERO);
    let device_dims = create_rw_signal((0u32, 0u32));
    
    // Effect to trigger repaint when frame signal changes
    create_effect(move |_| {
        if frame_signal.get().is_some() {
            frame_counter.update(|v| *v = v.wrapping_add(1));
        }
    });

    // Create a memo that extracts RGBA data and creates a peniko::Image
    let image_memo = create_memo(move |_| {
        frame_signal.get().and_then(|frame| {
            let rgba_data = frame.to_rgba()?;
            let blob = Blob::new(Arc::new(rgba_data));
            device_dims.set((frame.width, frame.height));
            Some(Image::new(blob, ImageFormat::Rgba8, frame.width, frame.height))
        })
    });

    let has_frame = create_memo(move |_| frame_signal.get().is_some());
    
    stack((
        canvas(move |cx, size| {
            let counter = frame_counter.get();
            
            let full_rect = size.to_rect();
            cx.fill(&full_rect, Color::from_rgb8(20, 20, 25), 0.0);
            
            if let Some(image) = image_memo.get() {
                let img_rect = calculate_aspect_fit(
                    image.width, image.height,
                    size.width, size.height
                );
                
                if img_rect.width() > 0.0 && img_rect.height() > 0.0 {
                    last_image_rect.set(img_rect);
                    
                    let hash_bytes = counter.to_ne_bytes();
                    let img = Img {
                        img: image,
                        hash: &hash_bytes,
                    };
                    cx.draw_img(img, img_rect);
                }
            }
        })
        .on_event_stop(floem::event::EventListener::PointerDown, move |e| {
            if let floem::event::Event::PointerDown(pe) = e {
                let img_rect = last_image_rect.get();
                let (dev_w, dev_h) = device_dims.get();
                
                if dev_w > 0 && dev_h > 0 {
                    if let Some((device_x, device_y)) = map_to_device_coords(
                        pe.pos.x, pe.pos.y, img_rect, dev_w, dev_h
                    ) {
                        on_click(device_x as f64, device_y as f64);
                    }
                }
            }
        })
        .style(|s| s.width_full().height_full().min_width(200.0).min_height(400.0)),
        
        container(
            label(move || "Starting emulator...".to_string())
                .style(|s| s.color(Color::from_rgb8(150, 150, 150)))
        )
        .style(move |s| {
            s.absolute()
                .width_full()
                .height_full()
                .items_center()
                .justify_center()
                .apply_if(has_frame.get(), |s| s.hide())
        }),
    ))
    .style(|s| s.width_full().height_full().min_width(200.0).min_height(400.0))
}
