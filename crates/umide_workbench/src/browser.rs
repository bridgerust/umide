//! Interactive WebKit child view hosted in the Floem window on macOS.
use std::{
    ffi::{CString, c_char, c_void},
    ptr::NonNull,
};

use floem::{
    RawWindowHandle, View, ViewId, context::PaintCx, prelude::*, window::WindowIdExt,
};

unsafe extern "C" {
    fn umide_browser_new(parent: *mut c_void, address: *const c_char)
    -> *mut c_void;
    fn umide_browser_layout(
        view: *mut c_void,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    );
    fn umide_browser_reload(view: *mut c_void);
    fn umide_browser_remove(view: *mut c_void);
}

struct Browser {
    id: ViewId,
    url: CString,
    child: Option<NonNull<c_void>>,
}

impl View for Browser {
    fn id(&self) -> ViewId {
        self.id
    }

    fn update(
        &mut self,
        _cx: &mut floem::context::UpdateCx,
        _state: Box<dyn std::any::Any>,
    ) {
        if let Some(child) = self.child {
            unsafe {
                umide_browser_reload(child.as_ptr());
            }
        }
    }

    fn paint(&mut self, _cx: &mut PaintCx) {
        let Some(layout) = self.id.get_layout() else {
            return;
        };
        if self.child.is_none() {
            let Some(RawWindowHandle::AppKit(handle)) = self
                .id
                .window_id()
                .and_then(|window| window.raw_window_handle())
            else {
                return;
            };
            // Floem paints on AppKit's main thread; the NSView remains valid for this call.
            self.child = NonNull::new(unsafe {
                umide_browser_new(handle.ns_view.as_ptr(), self.url.as_ptr())
            });
        }
        if let Some(child) = self.child {
            let origin = self.id.get_window_origin();
            unsafe {
                umide_browser_layout(
                    child.as_ptr(),
                    origin.x,
                    origin.y,
                    layout.size.width.into(),
                    layout.size.height.into(),
                );
            }
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            // Balances the retained reference from umide_browser_new on the UI thread.
            unsafe { umide_browser_remove(child.as_ptr()) };
        }
    }
}

pub fn viewport(url: String, refresh: RwSignal<u64>) -> impl IntoView {
    let id = ViewId::new();
    floem::reactive::UpdaterEffect::new(
        move || refresh.get(),
        move |_| id.update_state(()),
    );
    Browser {
        id,
        // Only backend-generated loopback addresses reach the viewport.
        url: CString::new(url).expect("server URL has no nul byte"),
        child: None,
    }
}
