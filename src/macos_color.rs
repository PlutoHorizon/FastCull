//! Tag the actual wgpu Metal layers as sRGB so ColorSync can match the display.
//! An untagged CAMetalLayer is unmanaged on wide-gamut screens.
use objc2::{
    class, msg_send,
    runtime::{AnyObject, Bool},
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ffi::c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: *const c_void;
    fn CGColorSpaceCreateWithName(name: *const c_void) -> *const c_void;
    fn CGColorSpaceRelease(space: *const c_void);
}
pub fn configure(cc: &eframe::CreationContext<'_>) {
    if objc2::MainThreadMarker::new().is_none() {
        return;
    }
    let Ok(handle) = cc.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: CreationContext borrows a live AppKit NSView on its UI thread.
    // CALayer owns its child layers during this synchronous traversal. Only
    // actual CAMetalLayer instances receive setColorspace:. The layer retains
    // the CGColorSpace; release our create-rule reference after tagging it.
    unsafe {
        let view = &*handle.ns_view.as_ptr().cast::<AnyObject>();
        let layer: *mut AnyObject = msg_send![view, layer];
        let space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        if space.is_null() {
            log::warn!("graphics failed to create sRGB color space");
            return;
        }
        let count = layer.as_ref().map_or(0, |layer| tag(layer, space, 0));
        CGColorSpaceRelease(space);
        if count > 0 {
            log::info!("graphics colorspace=sRGB metal_layers={count}");
        } else {
            log::warn!("graphics Metal color-space tagging found no layer");
        }
    }
}
unsafe fn tag(layer: &AnyObject, space: *const c_void, depth: usize) -> usize {
    if depth > 8 {
        return 0;
    }
    unsafe {
        let is_metal: Bool = msg_send![layer,isKindOfClass:class!(CAMetalLayer)];
        let mut tagged = 0;
        if is_metal.as_bool() {
            let (): () = msg_send![layer,setColorspace:space];
            tagged = 1;
        }
        let children: *mut AnyObject = msg_send![layer, sublayers];
        if let Some(children) = children.as_ref() {
            let count: usize = msg_send![children, count];
            for i in 0..count.min(64) {
                let child: *mut AnyObject = msg_send![children,objectAtIndex:i];
                if let Some(child) = child.as_ref() {
                    tagged += tag(child, space, depth + 1);
                }
            }
        }
        tagged
    }
}
