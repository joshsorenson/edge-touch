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
    /// From the mouse-emulation interface, not the digitizer.
    mouse: bool,
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
    /// The digitizer has sent fingers. Until then (it may stay in mouse mode),
    /// fall back to the single touch from the mouse-emulation interface.
    digitizer_seen: bool,
    /// Values logged so far (the first few go to the log for debugging).
    logged: u32,
}

impl Slots {
    /// Every finger that's down, 0..1 across the panel, keyed by contact id.
    fn fingers(&self) -> Vec<(u32, f64, f64)> {
        let mut by_id: HashMap<u32, (f64, f64)> = HashMap::new();
        let want_mouse = !self.digitizer_seen;
        for s in self.slots.values().filter(|s| s.tip && s.mouse == want_mouse) {
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
    fn IOHIDDeviceGetReport(device: Ref, report_type: u32, report_id: isize, report: *mut u8, len: *mut isize) -> i32;
    fn IOHIDDeviceSetReport(device: Ref, report_type: u32, report_id: isize, report: *const u8, len: isize) -> i32;
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

/// Feature report 0x21 holds the digitizer's Device Mode (see the Edge's
/// report descriptor: Device Configuration collection).
const DEVICE_MODE_REPORT: isize = 0x21;
const REPORT_FEATURE: u32 = 2;

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
        // The parent collection is the finger slot. Its usage tells the
        // digitizer's fingers (Digitizer page) from the mouse pointer.
        let parent = IOHIDElementGetParent(el);
        let mouse = !parent.is_null() && IOHIDElementGetUsagePage(parent) != PAGE_DIGITIZER;
        if slots.logged < 12 && !mouse {
            slots.logged += 1;
            log(&format!("finger value page={page:#x} usage={usage:#x} v={v}"));
        }
        let key = if parent.is_null() { 0 } else { IOHIDElementGetCookie(parent) as usize };
        let slot = slots.slots.entry(key).or_default();
        slot.mouse = mouse;
        if !mouse && page == PAGE_DIGITIZER && !slots.digitizer_seen {
            slots.digitizer_seen = true;
            log("multi-finger reports arriving");
        }
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

static TRACED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Log what the first gestures did (finger count plus every action but moves),
/// to debug touches that don't do what they should.
fn trace(f: &Frame, acts: &[crate::gesture::Action]) {
    use crate::gesture::Action;
    use std::sync::atomic::Ordering;
    let shown: Vec<_> = acts.iter().filter(|a| !matches!(a, Action::Drag(_) | Action::Momentum { .. } | Action::Scroll { phase: crate::gesture::Phase::Changed, .. })).collect();
    if shown.is_empty() || TRACED.fetch_add(1, Ordering::Relaxed) > 150 {
        return;
    }
    log(&format!("t={:.3} fingers={} -> {:?}", f.t, f.touches.len(), shown));
}

/// Diagnostic: per-interface counts of (page, usage) values seen while
/// listening without exclusive access.
static SEEN: Mutex<Option<HashMap<(i64, u32, u32), u32>>> = Mutex::new(None);

extern "C" fn on_listen(ctx: *mut c_void, _result: i32, _sender: Ref, value: Ref) {
    unsafe {
        let el = IOHIDValueGetElement(value);
        let key = (ctx as i64, IOHIDElementGetUsagePage(el), IOHIDElementGetUsage(el));
        *SEEN.lock().unwrap().get_or_insert_with(HashMap::new).entry(key).or_default() += 1;
    }
}

/// If the "listen" flag file exists: watch every interface for 45 seconds
/// WITHOUT taking it over (macOS keeps it too), logging which values arrive.
/// Halfway through, ask the digitizer for multi-finger mode again. Shows
/// whether the panel sends multi-finger data when we don't seize it.
unsafe fn listen_diagnostic(run_loop: Ref) {
    let flag = settings::dir().join("listen");
    if !flag.exists() {
        return;
    }
    let _ = std::fs::remove_file(&flag);
    log("listen: start (touch with two fingers now)");
    set_status(Status::Failed("diagnostic: listening".into()));
    let devices = touch_devices();
    for &dev in &devices {
        let page = prop_i64(dev, "PrimaryUsagePage");
        IOHIDDeviceRegisterInputValueCallback(dev, on_listen, page as usize as *mut c_void);
        IOHIDDeviceScheduleWithRunLoop(dev, run_loop, kCFRunLoopDefaultMode as Ref);
        let rc = IOHIDDeviceOpen(dev, 0);
        log(&format!("listen: open {page:#x} (shared) -> {rc:#x}"));
    }
    let started = Instant::now();
    let mut asked = false;
    let mut next_report = 5u64;
    while started.elapsed() < Duration::from_secs(45) {
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, 0);
        if !asked && started.elapsed() > Duration::from_secs(22) {
            asked = true;
            for &dev in devices.iter().filter(|&&d| prop_i64(d, "PrimaryUsagePage") == PAGE_DIGITIZER as i64) {
                let report = [DEVICE_MODE_REPORT as u8, 2u8, 0u8];
                let rc = IOHIDDeviceSetReport(dev, REPORT_FEATURE, DEVICE_MODE_REPORT, report.as_ptr(), report.len() as isize);
                log(&format!("listen: asked for multi-finger mode -> {rc:#x}"));
            }
        }
        if started.elapsed().as_secs() >= next_report {
            next_report += 5;
            let seen = SEEN.lock().unwrap().take().unwrap_or_default();
            let mut rows: Vec<_> = seen.into_iter().collect();
            rows.sort();
            let txt: Vec<String> = rows.iter().map(|((iface, p, u), n)| format!("{iface:#x}:{p:#x}/{u:#x}={n}")).collect();
            log(&format!("listen: {}s {}", started.elapsed().as_secs(), if txt.is_empty() { "nothing".into() } else { txt.join(" ") }));
        }
    }
    for &dev in &devices {
        IOHIDDeviceClose(dev, 0);
        IOHIDDeviceUnscheduleFromRunLoop(dev, run_loop, kCFRunLoopDefaultMode as Ref);
    }
    release_all(&devices);
    log("listen: done");
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
    listen_diagnostic(run_loop);

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
            IOHIDDeviceRegisterInputValueCallback(dev, on_value, slots as *mut c_void);
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
            // Ask the digitizer for multi-finger reports (HID "Device Mode" =
            // 2). Many touch panels stay in mouse mode until the computer
            // asks, which gives one finger through the mouse interface only.
            if has_digitizer {
                for &dev in devices.iter().filter(|&&d| prop_i64(d, "PrimaryUsagePage") == PAGE_DIGITIZER as i64) {
                    // Windows reads "Contact Count Maximum" (feature 0x0A)
                    // before switching modes, and some panels wait for that.
                    let mut max = [0u8; 4];
                    max[0] = 0x0A;
                    let mut mlen: isize = max.len() as isize;
                    let mrc = IOHIDDeviceGetReport(dev, REPORT_FEATURE, 0x0A, max.as_mut_ptr(), &mut mlen);
                    log(&format!("contact count max: {mrc:#x} {:?}", &max[..mlen.max(0) as usize]));
                    let report = [DEVICE_MODE_REPORT as u8, 2u8, 0u8];
                    let rc = IOHIDDeviceSetReport(dev, REPORT_FEATURE, DEVICE_MODE_REPORT, report.as_ptr(), report.len() as isize);
                    // Same request without the report id byte, in case the
                    // controller wants it that way.
                    let bare = [2u8, 0u8];
                    let rc2 = IOHIDDeviceSetReport(dev, REPORT_FEATURE, DEVICE_MODE_REPORT, bare.as_ptr(), bare.len() as isize);
                    log(&format!("device mode (no id byte): {rc2:#x}"));
                    let mut back = [0u8; 8];
                    let mut len: isize = back.len() as isize;
                    back[0] = DEVICE_MODE_REPORT as u8;
                    let get = IOHIDDeviceGetReport(dev, REPORT_FEATURE, DEVICE_MODE_REPORT, back.as_mut_ptr(), &mut len);
                    log(&format!("device mode -> multi-touch: set {rc:#x}, read back {get:#x} {:?}", &back[..len.max(0) as usize]));
                }
            }
            set_status(Status::On);
            let mut edge = edge_rect();
            let mut ticks = 0u32;
            let mut fingers_before = 0usize;
            let mut here_scrolls = true;
            loop {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.008, 0);
                let s = settings::get();
                gestures.opts.drag_scrolls = s.drag_scrolls && here_scrolls;
                gestures.opts.momentum = s.momentum;
                gestures.opts.zoom_drag = s.zoom_drag;
                gestures.opts.edge_swipes = s.edge_swipes;
                gestures.opts.bounds = edge;
                let t = clock.elapsed().as_secs_f64();
                if (*slots).dirty {
                    (*slots).dirty = false;
                    if let Some((ex, ey, ew, eh)) = edge {
                        let touches = (*slots).fingers().into_iter().map(|(id, x, y)| (id, Pt::new(ex + x * ew, ey + y * eh))).collect();
                        let frame: Frame = Frame { t, touches };
                        // First finger down: ask what's under it, so a drag
                        // scrolls pages and lists but moves everything else.
                        if fingers_before == 0 && frame.touches.len() == 1 && s.drag_scrolls {
                            let p = frame.touches[0].1;
                            here_scrolls = crate::ax::scrolls_at(p.x, p.y);
                            gestures.opts.drag_scrolls = here_scrolls;
                        }
                        fingers_before = frame.touches.len();
                        let acts = gestures.frame(&frame);
                        trace(&frame, &acts);
                        out.run(&acts, &s);
                    }
                }
                let acts = gestures.tick(t);
                trace(&Frame { t, touches: vec![] }, &acts);
                out.run(&acts, &s);

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
