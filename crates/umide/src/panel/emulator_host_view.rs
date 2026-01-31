use floem::{
    prelude::*,
    reactive::{create_memo, create_effect, create_rw_signal},
    peniko::{Blob, Color, ImageFormat, Image},
    views::{canvas, container, label, stack, Decorators},
};
use floem_renderer::Img;
use umide_emulator::decoder::DecodedFrame;
use std::sync::Arc;

pub fn emulator_host_view(
    frame_signal: impl SignalGet<Option<Arc<DecodedFrame>>> + Copy + 'static,
    on_click: impl Fn(f64, f64) + 'static,
) -> impl View {
    // Track frame updates for repaint trigger
    let repaint_trigger = create_rw_signal(0u64);
    
    // Effect to trigger repaint when frame signal changes
    create_effect(move |_| {
        let _frame = frame_signal.get();
        // Increment repaint trigger to force canvas invalidation
        repaint_trigger.update(|v| *v = v.wrapping_add(1));
    });

    // Create a memo that extracts RGBA data and creates a peniko::Image directly
    // This avoids PNG encoding overhead (~5-10ms per frame)
    let image_memo = create_memo(move |_| {
        let frame_opt = frame_signal.get();
        if frame_opt.is_some() {
            println!("DEBUG: Frame signal has data");
        }
        frame_opt.and_then(|frame| {
            println!("DEBUG: Processing frame {}x{}", frame.width, frame.height);
            let rgba_data = frame.to_rgba();
            if rgba_data.is_none() {
                println!("DEBUG: to_rgba() returned None!");
                return None;
            }
            let rgba = rgba_data.unwrap();
            println!("DEBUG: RGBA data len = {}", rgba.len());
            let blob = Blob::new(Arc::new(rgba));
            Some(Image::new(blob, ImageFormat::Rgba8, frame.width, frame.height))
        })
    });

    // Use a frame counter as a simple hash for cache invalidation
    let frame_counter = std::sync::atomic::AtomicU64::new(0);
    
    // Check if we have any frames
    let has_frame = create_memo(move |_| {
        frame_signal.get().is_some()
    });
    
    stack((
        // Canvas for rendering frames
        canvas(move |cx, size| {
            // Read repaint trigger to subscribe to updates
            let _trigger = repaint_trigger.get();
            
            let rect = size.to_rect();
            println!("DEBUG: Canvas paint, size = {:?}, trigger = {}", size, _trigger);
            
            if let Some(image) = image_memo.get() {
                println!("DEBUG: Drawing image {}x{}", image.width, image.height);
                // Increment frame counter for cache hash
                let counter = frame_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let hash_bytes = counter.to_ne_bytes();
                let img = Img {
                    img: image,
                    hash: &hash_bytes,
                };
                cx.draw_img(img, rect);
            } else {
                // Draw a dark gray background with a gradient to show the canvas is working
                cx.fill(&rect, Color::from_rgb8(30, 30, 40), 0.0);
            }
        })
        .on_click_stop(move |e| {
            if let floem::event::Event::PointerDown(pe) = e {
                on_click(pe.pos.x, pe.pos.y);
            }
        })
        .style(|s| s.width_full().height_full().min_width(200.0).min_height(400.0)),
        
        // Loading indicator when no frames yet
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
