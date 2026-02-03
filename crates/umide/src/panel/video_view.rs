use std::sync::Arc;
// use wgpu;
use floem::{
    prelude::*,
    peniko::{Blob, Color},
    views::{canvas},
};
use floem::reactive::Memo;
use umide_emulator::decoder::DecodedFrame;

pub fn video_view(
    frame_signal: impl SignalGet<Option<Arc<DecodedFrame>>> + 'static,
    on_click: impl Fn(f64, f64) + 'static,
) -> impl View {
    let img_memo = Memo::new(move |_| {
        frame_signal.get().map(|frame| {
            match &frame.frame {
                umide_emulator::decoder::GpuFrame::Software(data, _stride, _) => {
                    let _blob = Blob::new(data.clone());
                    // Image::new(
                    //     blob,
                    //   ::Rgba8,
                    //     frame.width,
                    //     frame.height
                    // )
                }
                umide_emulator::decoder::GpuFrame::Hardware(_) => {
                    // TODO: Implement hardware surface rendering
                     let _blob = Blob::new(Arc::new(vec![0; (frame.width * frame.height * 4) as usize]));
                    //  Image::new(
                    //     blob,
                    //     ImageFormat::Rgba8,
                    //     frame.width,
                    //     frame.height
                    // )
                }
            }
        })
    });

    Container::new(
        canvas(move |cx, size| {
            let rect = size.to_rect();

            if let Some(_image) = img_memo.get() {
                // cx.draw_img(image.into(), rect);
            } else {
                cx.fill(&rect, Color::BLACK, 0.0);
            }
        })
            .on_click_stop(move |event| {
                if let floem::event::Event::Pointer(PointerEvent::Down(PointerButtonEvent { state, .. })) = event {
                    on_click(state.logical_point().x, state.logical_point().y);
                }
            })
    )
        .style(|s| s.width_full().height_full())
}