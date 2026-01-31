use std::sync::Arc;
// use wgpu;
use floem::{
    prelude::*,
    peniko::{Blob, Color, ImageFormat, Image},
    views::{canvas, container},
};
use floem::reactive::create_memo;
use umide_emulator::decoder::DecodedFrame;

pub fn video_view(
    frame_signal: impl SignalGet<Option<Arc<DecodedFrame>>> + 'static,
    on_click: impl Fn(f64, f64) + 'static,
) -> impl View {
    let img_memo = create_memo(move |_| {
        frame_signal.get().map(|frame| {
            match &frame.frame {
                umide_emulator::decoder::GpuFrame::Software(data, _stride, _) => {
                    let blob = Blob::new(data.clone());
                    Image::new(
                        blob,
                        ImageFormat::Rgba8,
                        frame.width,
                        frame.height
                    )
                }
                umide_emulator::decoder::GpuFrame::Hardware(_) => {
                    // TODO: Implement hardware surface rendering
                     let blob = Blob::new(Arc::new(vec![0; (frame.width * frame.height * 4) as usize]));
                     Image::new(
                        blob,
                        ImageFormat::Rgba8,
                        frame.width,
                        frame.height
                    )
                }
            }
        })
    });

    container(
        canvas(move |cx, size| {
            let rect = size.to_rect();

            if let Some(_image) = img_memo.get() {
                // cx.draw_img(image.into(), rect);
            } else {
                cx.fill(&rect, Color::BLACK, 0.0);
            }
        })
            .on_click_stop(move |event| {
                if let floem::event::Event::PointerDown(pointer_event) = event {
                    on_click(pointer_event.pos.x, pointer_event.pos.y);
                }
            })
    )
        .style(|s| s.width_full().height_full())
}