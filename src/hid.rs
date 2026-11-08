//! Reads the Edge's touch panel and drives the gesture recognizer.
//!
//! The Edge's touch controller (USB 0x27c0:0x0859) shows up as three HID
//! interfaces: a 10-finger digitizer, a mouse-emulation interface and a
//! vendor one. macOS 27 treats it like a trackpad, so we open all three with
//! "seize" (exclusive access). macOS stops seeing the touches, and we turn the
//! digitizer's fingers into clicks, scrolls and zoom right where you touched.
//!
//! Needs Input Monitoring (to seize) and Accessibility (to send events).

use crate::gesture::{Frame, Pt, Recognizer};
use crate::output::Output;
use crate::settings::{self, log};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use core_foundation::base::TCFType;
use core_foundation::number::CFNumber;
use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop, CFRunLoopRunInMode};
use core_foundation::string::CFString;
use core_graphics::display::CGDisplay;

pub const TOUCH_VENDOR_ID: i64 = 0x27c0;
pub const TOUCH_PRODUCT_ID: i64 = 0x0859;

/// What the menu bar shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Starting,
    On,
    Off,
    NotFound,
    NeedsInputMonitoring,
    NeedsAccessibility,
    /// Edge Deck (or another app) still holds the panel.
    Waiting,
    Failed(String),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Starting => "Starting…".into(),
            Status::On => "Touch is on".into(),
            Status::Off => "Touch is off".into(),
            Status::NotFound => "Edge not found (check the USB cable)".into(),
            Status::NeedsInputMonitoring => "Needs Input Monitoring permission".into(),
            Status::NeedsAccessibility => "Needs Accessibility permission".into(),
            Status::Waiting => "Waiting for Edge Deck to let go…".into(),
            Status::Failed(e) => format!("Touch error {e}"),
        }
    }
}

static STATUS: Mutex<Status> = Mutex::new(Status::Starting);

pub fn status() -> Status {
    STATUS.lock().unwrap().clone()
}

fn set_status(s: Status) {
    let mut cur = STATUS.lock().unwrap();
    if *cur != s {
        log(&format!("status: {}", s.label()));
        *cur = s;
    }
}

/// The Edge's bounds in global points: the widest display at least 3x as wide
/// as it is tall.
pub fn edge_rect() -> Option<(f64, f64, f64, f64)> {
    CGDisplay::active_displays()
        .ok()?
        .into_iter()
        .map(|id| CGDisplay::new(id).bounds())
        .filter(|b| b.size.height > 0.0 && b.size.width / b.size.height >= 3.0)
        .max_by(|a, b| a.size.width.total_cmp(&b.size.width))
        .map(|b| (b.origin.x, b.origin.y, b.size.width, b.size.height))
}

/// Map a raw HID value onto 0..1 using the element's logical range.
fn normalize(v: i64, min: i64, max: i64) -> f64 {
    if max <= min {
        return 0.0;
    }
    ((v - min) as f64 / (max - min) as f64).clamp(0.0, 1.0)
}

/// One finger slot in the digitizer report (the Edge has 10).
#[derive(Default, Clone, Copy, Debug)]
struct Slot {
    tip: bool,
    id: u32,
    x: f64,
    y: f64,
}

/// Latest value of every slot, filled in by the HID callback.
#[derive(Default)]
struct Slots {
    slots: HashMap<usize, Slot>,
    dirty: bool,
}

impl Slots {
    /// Every finger that's down, 0..1 across the panel, keyed by contact id.
    fn fingers(&self) -> Vec<(u32, f64, f64)> {
        let mut by_id: HashMap<u32, (f64, f64)> = HashMap::new();
        for s in self.slots.values().filter(|s| s.tip) {
            by_id.insert(s.id, (s.x, s.y));
        }
        by_id.into_iter().map(|(id, (x, y))| (id, x, y)).collect()
    }
}

type Ref = *mut c_void;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const std::ffi::c_char) -> Ref;
    fn IOServiceGetMatchingServices(main_port: u32, matching: Ref, iter: *mut u32) -> i32;
    fn IOIteratorNext(iter: u32) -> u32;
    fn IOObjectRelease(obj: u32) -> i32;
    fn IOHIDDeviceCreate(allocator: Ref, service: u32) -> Ref;
    fn IOHIDDeviceOpen(device: Ref, options: u32) -> i32;
    fn IOHIDDeviceGetProperty(device: Ref, key: Ref) -> Ref;
    fn IOHIDDeviceRegisterInputValueCallback(device: Ref, callback: extern "C" fn(*mut c_void, i32, Ref, Ref), context: *mut c_void);
    fn IOHIDDeviceScheduleWithRunLoop(device: Ref, run_loop: Ref, mode: Ref);
    fn IOHIDDeviceUnscheduleFromRunLoop(device: Ref, run_loop: Ref, mode: Ref);
    fn IOHIDDeviceClose(device: Ref, options: u32) -> i32;
    fn IOHIDValueGetElement(value: Ref) -> Ref;
    fn IOHIDValueGetIntegerValue(value: Ref) -> isize;
    fn IOHIDElementGetUsagePage(element: Ref) -> u32;
    fn IOHIDElementGetUsage(element: Ref) -> u32;
    fn IOHIDElementGetLogicalMin(element: Ref) -> isize;
    fn IOHIDElementGetLogicalMax(element: Ref) -> isize;
    fn IOHIDElementGetParent(element: Ref) -> Ref;
    fn IOHIDElementGetCookie(element: Ref) -> u32;
    fn IOHIDRequestAccess(request_type: u32) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDictionarySetValue(dict: Ref, key: Ref, value: Ref);
    fn CFRelease(obj: Ref);
}

const SEIZE_DEVICE: u32 = 0x01;
const REQUEST_LISTEN_EVENT: u32 = 1;
/// kIOReturnNotPermitted: Input Monitoring isn't granted.
const NOT_PERMITTED: i32 = 0xe00002e2u32 as i32;
/// kIOReturnExclusiveAccess: another app (Edge Deck) has seized it.
const EXCLUSIVE: i32 = 0xe00002c5u32 as i32;

const PAGE_GENERIC_DESKTOP: u32 = 0x01;
const USAGE_X: u32 = 0x30;
const USAGE_Y: u32 = 0x31;
const PAGE_BUTTON: u32 = 0x09;
const PAGE_DIGITIZER: u32 = 0x0D;
const USAGE_TIP_SWITCH: u32 = 0x42;
const USAGE_CONTACT_ID: u32 = 0x51;

/// Every HID interface of the Edge's touch controller.
unsafe fn touch_devices() -> Vec<Ref> {
    let matching = IOServiceMatching(c"IOHIDDevice".as_ptr());
    if matching.is_null() {
        return vec![];
    }
    let (vk, vv) = (CFString::new("VendorID"), CFNumber::from(TOUCH_VENDOR_ID));
    let (pk, pv) = (CFString::new("ProductID"), CFNumber::from(TOUCH_PRODUCT_ID));
    CFDictionarySetValue(matching, vk.as_concrete_TypeRef() as Ref, vv.as_concrete_TypeRef() as Ref);
    CFDictionarySetValue(matching, pk.as_concrete_TypeRef() as Ref, pv.as_concrete_TypeRef() as Ref);
    let mut iter = 0u32;
    if IOServiceGetMatchingServices(0, matching, &mut iter) != 0 {
        return vec![];
    }
    let mut out = vec![];
    loop {
        let service = IOIteratorNext(iter);
        if service == 0 {
            break;
        }
        let dev = IOHIDDeviceCreate(std::ptr::null_mut(), service);
        if !dev.is_null() {
            out.push(dev);
        }
        IOObjectRelease(service);
    }
    IOObjectRelease(iter);
    out
}

unsafe fn prop_i64(dev: Ref, key: &str) -> i64 {
    let k = CFString::new(key);
    let v = IOHIDDeviceGetProperty(dev, k.as_concrete_TypeRef() as Ref);
    if v.is_null() {
        return -1;
    }
    CFNumber::wrap_under_get_rule(v as _).to_i64().unwrap_or(-1)
}

/// How many touch interfaces are plugged in right now.
unsafe fn count_devices() -> usize {
    let d = touch_devices();
    release_all(&d);
    d.len()
}

unsafe fn release_all(devices: &[Ref]) {
    for d in devices {
        CFRelease(*d);
    }
}

/// HID value callback: store the value in its finger slot.
extern "C" fn on_value(ctx: *mut c_void, _result: i32, _sender: Ref, value: Ref) {
    let slots = unsafe { &mut *(ctx as *mut Slots) };
    unsafe {
        let el = IOHIDValueGetElement(value);
        let (page, usage) = (IOHIDElementGetUsagePage(el), IOHIDElementGetUsage(el));
        let v = IOHIDValueGetIntegerValue(value) as i64;
        // The parent collection is the finger slot.
        let key = IOHIDElementGetCookie(IOHIDElementGetParent(el)) as usize;
        let slot = slots.slots.entry(key).or_default();
        match (page, usage) {
            (PAGE_GENERIC_DESKTOP, USAGE_X) => slot.x = normalize(v, IOHIDElementGetLogicalMin(el) as i64, IOHIDElementGetLogicalMax(el) as i64),
            (PAGE_GENERIC_DESKTOP, USAGE_Y) => slot.y = normalize(v, IOHIDElementGetLogicalMin(el) as i64, IOHIDElementGetLogicalMax(el) as i64),
            (PAGE_DIGITIZER, USAGE_TIP_SWITCH) | (PAGE_BUTTON, 1) => slot.tip = v != 0,
            (PAGE_DIGITIZER, USAGE_CONTACT_ID) => slot.id = v as u32,
            _ => return,
        }
        slots.dirty = true;
    }
}

/// Run the touch driver on its own thread, forever.
pub fn start() {
    std::thread::spawn(|| unsafe { run() });
}

unsafe fn run() {
    // Shows the Input Monitoring prompt the first time.
    IOHIDRequestAccess(REQUEST_LISTEN_EVENT);
    let slots: *mut Slots = Box::into_raw(Box::default());
    let run_loop = CFRunLoop::get_current().as_concrete_TypeRef() as Ref;
    let mode = kCFRunLoopDefaultMode as Ref;
    let mut out = Output::new();
    let mut gestures = Recognizer::default();
    let clock = Instant::now();
    let mut asked_ax = false;

    loop {
        let s = settings::get();
        if !s.enabled {
            crate::claim::release();
            set_status(Status::Off);
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        if !crate::output::accessibility_ok(!asked_ax) {
            asked_ax = true;
            set_status(Status::NeedsAccessibility);
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        crate::claim::heartbeat();

        let devices = touch_devices();
        if devices.is_empty() {
            set_status(Status::NotFound);
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }
        // Read fingers from the digitizer only. The mouse-emulation interface
        // reports the same touch again, which would look like a second finger.
        let has_digitizer = devices.iter().any(|&d| prop_i64(d, "PrimaryUsagePage") == PAGE_DIGITIZER as i64);
        for &dev in &devices {
            if !has_digitizer || prop_i64(dev, "PrimaryUsagePage") == PAGE_DIGITIZER as i64 {
                IOHIDDeviceRegisterInputValueCallback(dev, on_value, slots as *mut c_void);
            }
            IOHIDDeviceScheduleWithRunLoop(dev, run_loop, mode);
        }

        // Seize every interface. Edge Deck lets go within a second or two of
        // seeing our claim, so keep trying.
        let mut seized = vec![false; devices.len()];
        let started = Instant::now();
        while !seized.iter().all(|s| *s) && settings::get().enabled {
            let mut codes = vec![];
            for (i, &dev) in devices.iter().enumerate() {
                if !seized[i] {
                    let rc = IOHIDDeviceOpen(dev, SEIZE_DEVICE);
                    seized[i] = rc == 0;
                    codes.push(rc);
                }
            }
            if seized.iter().all(|s| *s) {
                break;
            }
            set_status(if codes.contains(&NOT_PERMITTED) {
                Status::NeedsInputMonitoring
            } else if codes.contains(&EXCLUSIVE) {
                Status::Waiting
            } else {
                Status::Failed(format!("{:#x}", codes.iter().find(|c| **c != 0).unwrap_or(&0)))
            });
            crate::claim::heartbeat();
            std::thread::sleep(Duration::from_millis(500));
            if started.elapsed() > Duration::from_secs(10) && count_devices() != devices.len() {
                break;
            }
        }

        if seized.iter().all(|s| *s) {
            set_status(Status::On);
            let mut edge = edge_rect();
            let mut ticks = 0u32;
            loop {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.008, 0);
                let s = settings::get();
                gestures.opts.drag_scrolls = s.drag_scrolls;
                gestures.opts.momentum = s.momentum;
                let t = clock.elapsed().as_secs_f64();
                if (*slots).dirty {
                    (*slots).dirty = false;
                    if let Some((ex, ey, ew, eh)) = edge {
                        let touches = (*slots).fingers().into_iter().map(|(id, x, y)| (id, Pt::new(ex + x * ew, ey + y * eh))).collect();
                        out.run(&gestures.frame(&Frame { t, touches }), &s);
                    }
                }
                out.run(&gestures.tick(t), &s);

                ticks += 1;
                if !s.enabled {
                    break;
                }
                if ticks % 125 == 0 {
                    crate::claim::heartbeat();
                    edge = edge_rect();
                }
                if ticks % 375 == 0 && count_devices() != devices.len() {
                    log("touch panel unplugged; looking for it again");
                    break;
                }
            }
            out.run(&gestures.cancel(), &settings::get());
        }

        for (i, &dev) in devices.iter().enumerate() {
            if seized[i] {
                IOHIDDeviceClose(dev, 0);
            }
            IOHIDDeviceUnscheduleFromRunLoop(dev, run_loop, mode);
        }
        release_all(&devices);
        *slots = Slots::default();
    }
}
