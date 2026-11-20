//! Independently sized terminal beneath the current workspace view.
use floem::{
    event::{Event, EventListener},
    prelude::*,
    style::CursorStyle,
    ui_events::pointer::{PointerButton, PointerEvent},
};

use crate::{
    design::*,
    state::Workbench,
    workbench_terminal::{self, Terminals},
};

#[derive(Clone, Copy)]
pub struct Dock {
    pub visible: RwSignal<bool>,
    requested: RwSignal<Option<f64>>,
    available: RwSignal<f64>,
}

impl Dock {
    pub fn new() -> Self {
        Self {
            visible: RwSignal::new(false),
            requested: RwSignal::new(None),
            available: RwSignal::new(720.),
        }
    }

    pub fn window_resized(self, height: f64) {
        if self.available.get_untracked() != height {
            self.available.set(height);
        }
    }

    fn height(self) -> f64 {
        let available = self.available.get();
        let preferred = self
            .requested
            .get()
            .unwrap_or_else(|| (available * 0.30).clamp(150., 285.));
        // Leave room for visible messages and the composer at the minimum window size.
        preferred.clamp(130., (available - 320.).max(130.))
    }

    fn resize(self, height: f64) {
        self.requested.set(Some(
            height.clamp(130., (self.available.get_untracked() - 320.).max(130.)),
        ));
    }

    fn divider(self) -> impl IntoView {
        let start = RwSignal::new(None::<(f64, f64)>);
        let grip = Empty::new().style(move |s| {
            s.width_full()
                .height(6)
                .flex_shrink(0_f32)
                .border_top(1)
                .border_color(line())
                .background(if start.get().is_some() {
                    mint_wash()
                } else {
                    panel()
                })
                .cursor(CursorStyle::RowResize)
                .focusable(true)
                .hover(|s| s.background(mint_wash()))
                .focus_visible(|s| s.background(accent()))
        });
        let id = grip.id();
        grip.on_event_stop(EventListener::PointerDown, move |event| {
            if let Event::Pointer(PointerEvent::Down(pointer)) = event
                && pointer.button == Some(PointerButton::Primary)
            {
                if pointer.state.count == 2 {
                    self.requested.set(None);
                    return;
                }
                if let Some(pid) = pointer.pointer.pointer_id {
                    id.set_pointer_capture(pid);
                }
                start.set(Some((
                    id.get_window_origin().y + pointer.state.logical_point().y,
                    self.height(),
                )));
            }
        })
        .on_event_cont(EventListener::PointerMove, move |event| {
            if let Some((origin, height)) = start.get_untracked()
                && let Some(point) = event.point()
            {
                self.resize(height + origin - (id.get_window_origin().y + point.y));
            }
        })
        .on_event_cont(EventListener::PointerUp, move |event| {
            if let Event::Pointer(PointerEvent::Up(pointer)) = event
                && let Some(pid) = pointer.pointer.pointer_id
            {
                id.release_pointer_capture(pid);
            }
            start.set(None);
        })
        .on_event_cont(EventListener::LostPointerCapture, move |_| start.set(None))
        .on_key_down(
            Key::Named(NamedKey::ArrowUp),
            |m| m.is_empty(),
            move |_| self.resize(self.height() + 24.),
        )
        .on_key_down(
            Key::Named(NamedKey::ArrowDown),
            |m| m.is_empty(),
            move |_| self.resize(self.height() - 24.),
        )
        .tooltip(|| "Resize terminal · double-click to reset")
    }

    pub fn view(
        self,
        wb: Workbench,
        terminals: Terminals,
        hide: impl Fn() + Copy + 'static,
    ) -> impl IntoView {
        dyn_container(
            move || self.visible.get(),
            move |open| {
                if !open {
                    return Empty::new().into_any();
                }
                Stack::vertical((
                    self.divider(),
                    workbench_terminal::panel(wb, terminals, hide).style(|s| {
                        s.width_full().min_height(0).flex_grow(1_f32).flex_basis(0)
                    }),
                ))
                .style(|s| s.size_full().min_size(0, 0))
                .into_any()
            },
        )
        .style(move |s| {
            s.width_full()
                .height(self.height())
                .flex_shrink(0_f32)
                .apply_if(!self.visible.get(), |s| s.hide())
        })
    }
}
