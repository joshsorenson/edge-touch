//! Asks macOS (through Accessibility) what's under a finger, so a one-finger
//! drag can scroll inside scrollable things (pages, lists) and drag
//! everything else (window title bars, sliders, icons).

use std::ffi::c_void;

use core_foundation::base::{CFType, TCFType};
use core_foundation::string::CFString;

type Ref = *mut c_void;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> Ref;
    fn AXUIElementCopyElementAtPosition(app: Ref, x: f32, y: f32, out: *mut Ref) -> i32;
    fn AXUIElementCopyAttributeValue(el: Ref, attr: Ref, out: *mut Ref) -> i32;
    fn AXUIElementSetMessagingTimeout(el: Ref, seconds: f32) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(obj: Ref);
}

/// Roles that scroll when you drag them on a touchscreen.
const SCROLLS: [&str; 4] = ["AXScrollArea", "AXWebArea", "AXTable", "AXOutline"];
/// Roles that a drag should grab, even inside a scroll area.
const GRABS: [&str; 4] = ["AXSlider", "AXSplitter", "AXScrollBar", "AXValueIndicator"];

unsafe fn role(el: Ref) -> Option<String> {
    let attr = CFString::new("AXRole");
    let mut out: Ref = std::ptr::null_mut();
    if AXUIElementCopyAttributeValue(el, attr.as_concrete_TypeRef() as Ref, &mut out) != 0 || out.is_null() {
        return None;
    }
    let v = CFType::wrap_under_create_rule(out as _);
    v.downcast::<CFString>().map(|s| s.to_string())
}

unsafe fn parent(el: Ref) -> Ref {
    let attr = CFString::new("AXParent");
    let mut out: Ref = std::ptr::null_mut();
    if AXUIElementCopyAttributeValue(el, attr.as_concrete_TypeRef() as Ref, &mut out) != 0 {
        return std::ptr::null_mut();
    }
    out
}

/// True if dragging at (x, y) should scroll, false if it should drag. If the
/// app doesn't answer (or answers slowly), scroll.
pub fn scrolls_at(x: f64, y: f64) -> bool {
    unsafe {
        let sys = AXUIElementCreateSystemWide();
        if sys.is_null() {
            return true;
        }
        AXUIElementSetMessagingTimeout(sys, 0.15);
        let mut el: Ref = std::ptr::null_mut();
        let rc = AXUIElementCopyElementAtPosition(sys, x as f32, y as f32, &mut el);
        CFRelease(sys);
        if rc != 0 || el.is_null() {
            return true;
        }
        // Walk up from the element under the finger to its window.
        let mut answer = false;
        for _ in 0..16 {
            match role(el).as_deref() {
                Some(r) if GRABS.contains(&r) => break,
                Some(r) if SCROLLS.contains(&r) => {
                    answer = true;
                    break;
                }
                Some("AXWindow") | Some("AXApplication") | None => break,
                _ => {}
            }
            let up = parent(el);
            CFRelease(el);
            el = up;
            if el.is_null() {
                break;
            }
        }
        if !el.is_null() {
            CFRelease(el);
        }
        answer
    }
}
