//! Turns gesture actions into real macOS events (clicks, drags, scrolls,
//! zoom, Back/Forward), the same events a mouse or trackpad would send.
//! Needs Accessibility permission, or macOS drops them.

use crate::gesture::{Action, Phase, Pt};
use crate::settings::Settings;
use std::ffi::c_void;

type Ref = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceCreate(state: i32) -> Ref;
    fn CGEventSourceSetLocalEventsSuppressionInterval(src: Ref, seconds: f64);
    fn CGEventCreate(src: Ref) -> Ref;
    fn CGEventCreateMouseEvent(src: Ref, t: u32, at: CGPoint, button: u32) -> Ref;
    fn CGEventCreateScrollWheelEvent2(src: Ref, units: u32, count: u32, w1: i32, w2: i32, w3: i32) -> Ref;
    fn CGEventCreateKeyboardEvent(src: Ref, key: u16, down: bool) -> Ref;
    fn CGEventSetType(e: Ref, t: u32);
    fn CGEventSetLocation(e: Ref, at: CGPoint);
    fn CGEventGetLocation(e: Ref) -> CGPoint;
    fn CGEventSetFlags(e: Ref, flags: u64);
    fn CGEventSetIntegerValueField(e: Ref, field: u32, v: i64);
    fn CGEventSetDoubleValueField(e: Ref, field: u32, v: f64);
    fn CGEventPost(tap: u32, e: Ref);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(obj: Ref);
}

// Event types.
const LEFT_DOWN: u32 = 1;
const LEFT_UP: u32 = 2;
const RIGHT_DOWN: u32 = 3;
const RIGHT_UP: u32 = 4;
const MOVED: u32 = 5;
const LEFT_DRAGGED: u32 = 6;
/// NSEventTypeGesture: the wrapper type trackpad gestures arrive as.
const GESTURE: u32 = 29;

// Event fields.
const CLICK_STATE: u32 = 1;
const SCROLL_IS_CONTINUOUS: u32 = 88;
const SCROLL_PHASE: u32 = 99;
const SCROLL_MOMENTUM_PHASE: u32 = 123;
const SCROLL_POINT_DELTA_1: u32 = 96;
const SCROLL_POINT_DELTA_2: u32 = 97;
const SCROLL_FIXED_DELTA_1: u32 = 93;
const SCROLL_FIXED_DELTA_2: u32 = 94;
// Undocumented gesture fields (as used by trackpad zoom events).
const GESTURE_HID_TYPE: u32 = 110;
const GESTURE_ZOOM_VALUE: u32 = 113;
const GESTURE_PHASE: u32 = 132;
const HID_TYPE_ZOOM: i64 = 8;

const HID_TAP: u32 = 0;
const PIXEL_UNITS: u32 = 0;
const CMD: u64 = 1 << 20;
const CTRL: u64 = 1 << 18;
/// Fn + numeric keypad: what a real arrow key press carries.
const ARROW_FLAGS: u64 = (1 << 23) | (1 << 21);
const KEY_UP: u16 = 0x7E;
const KEY_DOWN: u16 = 0x7D;

// Key codes.
const KEY_LEFT_BRACKET: u16 = 0x21;
const KEY_RIGHT_BRACKET: u16 = 0x1E;
const KEY_EQUAL: u16 = 0x18;
const KEY_MINUS: u16 = 0x1B;

pub struct Output {
    src: Ref,
    saved: Option<Pt>,
    /// Sub-point scroll left over from the last event, so slow scrolls still move.
    carry: (f64, f64),
    /// Pinch progress toward the next Cmd +/- press.
    pinch: f64,
}

// The event source is only used from the touch thread.
unsafe impl Send for Output {}

fn cg(p: Pt) -> CGPoint {
    CGPoint { x: p.x, y: p.y }
}

fn scroll_phase(p: Phase) -> i64 {
    match p {
        Phase::Began => 1,
        Phase::Changed => 2,
        Phase::Ended => 4,
    }
}

fn momentum_phase(p: Phase) -> i64 {
    match p {
        Phase::Began => 1,
        Phase::Changed => 2,
        Phase::Ended => 3,
    }
}

impl Output {
    pub fn new() -> Self {
        unsafe {
            // HID system state: our events look like they came from a device.
            let src = CGEventSourceCreate(1);
            if !src.is_null() {
                // No pause in real mouse input after we move the cursor.
                CGEventSourceSetLocalEventsSuppressionInterval(src, 0.0);
            }
            Output { src, saved: None, carry: (0.0, 0.0), pinch: 0.0 }
        }
    }

    unsafe fn post(&self, e: Ref) {
        if !e.is_null() {
            CGEventPost(HID_TAP, e);
            CFRelease(e);
        }
    }

    unsafe fn mouse(&self, t: u32, p: Pt, button: u32, clicks: i64) {
        let e = CGEventCreateMouseEvent(self.src, t, cg(p), button);
        if !e.is_null() && clicks > 0 {
            CGEventSetIntegerValueField(e, CLICK_STATE, clicks);
        }
        self.post(e);
    }

    unsafe fn key(&self, code: u16, flags: u64) {
        for down in [true, false] {
            let e = CGEventCreateKeyboardEvent(self.src, code, down);
            if !e.is_null() {
                CGEventSetFlags(e, flags);
            }
            self.post(e);
        }
    }

    /// One continuous (trackpad-style) scroll event.
    unsafe fn scroll(&mut self, at: Pt, dx: f64, dy: f64, phase: i64, momentum: i64, flip: bool) {
        let s = if flip { -1.0 } else { 1.0 };
        let (fx, fy) = (dx * s + self.carry.0, dy * s + self.carry.1);
        let (ix, iy) = (fx.trunc(), fy.trunc());
        self.carry = (fx - ix, fy - iy);
        let e = CGEventCreateScrollWheelEvent2(self.src, PIXEL_UNITS, 2, iy as i32, ix as i32, 0);
        if e.is_null() {
            return;
        }
        CGEventSetLocation(e, cg(at));
        CGEventSetIntegerValueField(e, SCROLL_IS_CONTINUOUS, 1);
        CGEventSetIntegerValueField(e, SCROLL_POINT_DELTA_1, iy as i64);
        CGEventSetIntegerValueField(e, SCROLL_POINT_DELTA_2, ix as i64);
        CGEventSetDoubleValueField(e, SCROLL_FIXED_DELTA_1, fy);
        CGEventSetDoubleValueField(e, SCROLL_FIXED_DELTA_2, fx);
        CGEventSetIntegerValueField(e, SCROLL_PHASE, phase);
        CGEventSetIntegerValueField(e, SCROLL_MOMENTUM_PHASE, momentum);
        self.post(e);
        if phase == 4 || momentum == 3 {
            self.carry = (0.0, 0.0);
        }
    }

    /// Pinch as a trackpad zoom gesture (apps zoom the way they do on a
    /// trackpad), or as Cmd +/- key presses if that setting is on.
    unsafe fn zoom(&mut self, at: Pt, amount: f64, phase: Phase, keys: bool) {
        if keys {
            self.pinch += amount;
            if phase == Phase::Ended {
                self.pinch = 0.0;
            }
            while self.pinch.abs() >= 0.15 {
                self.key(if self.pinch > 0.0 { KEY_EQUAL } else { KEY_MINUS }, CMD);
                self.pinch -= 0.15 * self.pinch.signum();
            }
            return;
        }
        let e = CGEventCreate(self.src);
        if e.is_null() {
            return;
        }
        CGEventSetType(e, GESTURE);
        CGEventSetLocation(e, cg(at));
        CGEventSetIntegerValueField(e, GESTURE_HID_TYPE, HID_TYPE_ZOOM);
        CGEventSetDoubleValueField(e, GESTURE_ZOOM_VALUE, amount);
        CGEventSetIntegerValueField(e, GESTURE_PHASE, scroll_phase(phase));
        self.post(e);
    }

    pub fn run(&mut self, actions: &[Action], s: &Settings) {
        for a in actions {
            unsafe { self.one(a, s) }
        }
    }

    unsafe fn one(&mut self, a: &Action, s: &Settings) {
        match *a {
            Action::SaveCursor => {
                let e = CGEventCreate(std::ptr::null_mut());
                if !e.is_null() {
                    let p = CGEventGetLocation(e);
                    CFRelease(e);
                    self.saved = Some(Pt::new(p.x, p.y));
                }
            }
            Action::RestoreCursor => {
                if let Some(p) = self.saved.take() {
                    if s.return_cursor {
                        // A queued move, not a warp: it lands after the click
                        // and scroll events already sent, so they reach the
                        // Edge before the cursor leaves. (A warp jumps the
                        // queue and the click went to the main screen.)
                        std::thread::sleep(std::time::Duration::from_millis(30));
                        self.mouse(MOVED, p, 0, 0);
                    }
                }
            }
            Action::MoveTo(p) => self.mouse(MOVED, p, 0, 0),
            Action::Down(p, n) => self.mouse(LEFT_DOWN, p, 0, n),
            Action::Up(p, n) => self.mouse(LEFT_UP, p, 0, n),
            Action::Drag(p) => self.mouse(LEFT_DRAGGED, p, 0, 1),
            Action::RightClick(p) => {
                self.mouse(RIGHT_DOWN, p, 1, 1);
                self.mouse(RIGHT_UP, p, 1, 1);
            }
            Action::Scroll { at, dx, dy, phase } => self.scroll(at, dx, dy, scroll_phase(phase), 0, s.flip_scroll),
            Action::Momentum { at, dx, dy, phase } => self.scroll(at, dx, dy, 0, momentum_phase(phase), s.flip_scroll),
            Action::Zoom { at, amount, phase } => self.zoom(at, amount, phase, s.pinch_keys),
            Action::Back => self.key(KEY_LEFT_BRACKET, CMD),
            Action::Forward => self.key(KEY_RIGHT_BRACKET, CMD),
            // Control + Up / Down arrow (arrows carry the Fn and keypad flags).
            Action::MissionControl => self.key(KEY_UP, CTRL | ARROW_FLAGS),
            Action::AppWindows => self.key(KEY_DOWN, CTRL | ARROW_FLAGS),
        }
    }
}

/// Whether macOS lets us send events (Accessibility). `prompt` shows the
/// system dialog that sends you to Settings.
pub fn accessibility_ok(prompt: bool) -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrustedWithOptions(options: Ref) -> bool;
    }
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let val = if prompt { CFBoolean::true_value() } else { CFBoolean::false_value() };
    let opts = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), val.as_CFType())]);
    unsafe { AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef() as Ref) }
}
