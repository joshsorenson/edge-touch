//! Turns touch frames into mouse, scroll and zoom actions.
//!
//! Pure logic: no macOS calls, so every gesture is unit tested. `hid.rs` feeds
//! it frames (every finger that's down, in global screen points) and timer
//! ticks; `output.rs` turns the actions into real macOS events.
//!
//! Gestures:
//! - Tap: click (tap twice quickly for a double-click).
//! - One finger drag: scroll (or click-and-drag, if "one finger scrolls" is off).
//! - Hold, then drag: click-and-drag (move windows, select text).
//! - Hold, then lift: right-click.
//! - Two fingers: drag to scroll, pinch to zoom, tap to right-click.
//! - Three fingers: swipe right for Back, left for Forward.

/// A point in global screen points (CoreGraphics coordinates, y down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    pub fn new(x: f64, y: f64) -> Self {
        Pt { x, y }
    }
    fn dist(self, o: Pt) -> f64 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
}

/// Every finger that's down right now. `t` is in seconds.
#[derive(Clone, Debug)]
pub struct Frame {
    pub t: f64,
    pub touches: Vec<(u32, Pt)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Began,
    Changed,
    Ended,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Remember where the cursor is, before the touch moves it.
    SaveCursor,
    /// Put the cursor back where `SaveCursor` found it.
    RestoreCursor,
    MoveTo(Pt),
    /// Left button down/up. The number is the click count (2 = double-click).
    Down(Pt, i64),
    Drag(Pt),
    Up(Pt, i64),
    RightClick(Pt),
    /// Content moves with the finger by (dx, dy) points.
    Scroll { at: Pt, dx: f64, dy: f64, phase: Phase },
    /// The glide after a flick.
    Momentum { at: Pt, dx: f64, dy: f64, phase: Phase },
    /// `amount` is the change in scale since the last event (0.1 = 10% bigger).
    Zoom { at: Pt, amount: f64, phase: Phase },
    Back,
    Forward,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// One-finger drag scrolls (true) or click-drags (false).
    pub drag_scrolls: bool,
    /// Keep scrolling for a moment after a flick.
    pub momentum: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { drag_scrolls: true, momentum: true }
    }
}

/// How far a finger moves (points) before a touch counts as a drag, not a tap.
const SLOP: f64 = 14.0;
/// How long (seconds) a still finger is held before it becomes a hold.
const HOLD: f64 = 0.4;
/// Two fingers down and up within this time, without moving, is a right-click.
const TWO_TAP: f64 = 0.35;
/// Change in finger spread (points) that starts a pinch.
const PINCH_START: f64 = 24.0;
/// Two-finger movement (points) that starts a scroll.
const SCROLL_START: f64 = 8.0;
/// Three-finger sideways travel (points) for Back / Forward.
const SWIPE: f64 = 120.0;
/// A second tap within this time and distance makes a double-click.
const DOUBLE_TIME: f64 = 0.4;
const DOUBLE_DIST: f64 = 24.0;
/// Momentum: speed left after one second (fraction), and the speed (points per
/// second) where it stops.
const DECAY_PER_SEC: f64 = 0.06;
const MIN_SPEED: f64 = 40.0;
/// Only the last bit of a drag counts toward flick speed.
const VELOCITY_WINDOW: f64 = 0.08;

#[derive(Clone, Copy, Debug, PartialEq)]
enum TwoMode {
    Undecided,
    Scroll,
    Pinch,
}

#[derive(Clone, Debug)]
enum State {
    Idle,
    /// One finger down, not yet a tap, scroll, drag or hold.
    Pending { id: u32, start: Pt, t0: f64, held: bool },
    Scrolling { id: u32, at: Pt, last: Pt, samples: Vec<(f64, Pt)> },
    Dragging { id: u32, last: Pt },
    Two { t0: f64, at: Pt, start_c: Pt, start_d: f64, last_c: Pt, last_d: f64, mode: TwoMode, samples: Vec<(f64, Pt)> },
    Three { start: Pt, fired: bool },
    /// Gliding after a flick.
    Coast { at: Pt, vx: f64, vy: f64, t: f64, began: bool },
    /// A gesture ended but fingers are still down: ignore them until none are.
    Wait,
}

pub struct Recognizer {
    state: State,
    saved: bool,
    last_click: Option<(f64, Pt, i64)>,
    pub opts: Options,
}

impl Default for Recognizer {
    fn default() -> Self {
        Recognizer { state: State::Idle, saved: false, last_click: None, opts: Options::default() }
    }
}

fn centroid(ts: &[(u32, Pt)]) -> Pt {
    let n = ts.len().max(1) as f64;
    Pt::new(ts.iter().map(|t| t.1.x).sum::<f64>() / n, ts.iter().map(|t| t.1.y).sum::<f64>() / n)
}

fn spread(ts: &[(u32, Pt)]) -> f64 {
    if ts.len() < 2 { 0.0 } else { ts[0].1.dist(ts[1].1) }
}

/// Speed (points/second) over the last few samples, or zero if the finger had
/// stopped before lifting.
fn velocity(samples: &[(f64, Pt)], now: f64) -> (f64, f64) {
    let recent: Vec<_> = samples.iter().filter(|(t, _)| now - t <= VELOCITY_WINDOW).collect();
    match (recent.first(), recent.last()) {
        (Some(a), Some(b)) if b.0 - a.0 > 0.01 => ((b.1.x - a.1.x) / (b.0 - a.0), (b.1.y - a.1.y) / (b.0 - a.0)),
        _ => (0.0, 0.0),
    }
}

impl Recognizer {
    /// A new set of fingers arrived.
    pub fn frame(&mut self, f: &Frame) -> Vec<Action> {
        let mut out = vec![];
        let mut ts = f.touches.clone();
        ts.sort_by_key(|t| t.0);
        self.step(f.t, &ts, &mut out);
        out
    }

    /// Time passed with no new frame (holds and momentum need this).
    pub fn tick(&mut self, t: f64) -> Vec<Action> {
        let mut out = vec![];
        match &mut self.state {
            State::Pending { t0, held, .. } if !*held && t - *t0 >= HOLD => *held = true,
            State::Coast { at, vx, vy, t: last, began } => {
                let dt = (t - *last).max(0.0);
                if dt < 0.004 {
                    return out;
                }
                *last = t;
                let k = DECAY_PER_SEC.powf(dt);
                *vx *= k;
                *vy *= k;
                let at = *at;
                if vx.hypot(*vy) < MIN_SPEED {
                    if *began {
                        out.push(Action::Momentum { at, dx: 0.0, dy: 0.0, phase: Phase::Ended });
                    }
                    self.finish(&mut out);
                } else {
                    let phase = if *began { Phase::Changed } else { Phase::Began };
                    *began = true;
                    out.push(Action::Momentum { at, dx: *vx * dt, dy: *vy * dt, phase });
                }
            }
            _ => {}
        }
        out
    }

    /// Stop whatever is in progress (touch paused or the Edge unplugged).
    pub fn cancel(&mut self) -> Vec<Action> {
        let mut out = vec![];
        match self.state.clone() {
            State::Dragging { last, .. } => out.push(Action::Up(last, 1)),
            State::Scrolling { at, .. } => out.push(Action::Scroll { at, dx: 0.0, dy: 0.0, phase: Phase::Ended }),
            State::Two { at, mode: TwoMode::Scroll, .. } => out.push(Action::Scroll { at, dx: 0.0, dy: 0.0, phase: Phase::Ended }),
            State::Two { at, mode: TwoMode::Pinch, .. } => out.push(Action::Zoom { at, amount: 0.0, phase: Phase::Ended }),
            State::Coast { at, began: true, .. } => out.push(Action::Momentum { at, dx: 0.0, dy: 0.0, phase: Phase::Ended }),
            _ => {}
        }
        self.finish(&mut out);
        out
    }

    /// Back to idle, giving the cursor back.
    fn finish(&mut self, out: &mut Vec<Action>) {
        self.state = State::Idle;
        if self.saved {
            out.push(Action::RestoreCursor);
            self.saved = false;
        }
    }

    fn save(&mut self, out: &mut Vec<Action>) {
        if !self.saved {
            out.push(Action::SaveCursor);
            self.saved = true;
        }
    }

    fn click(&mut self, t: f64, p: Pt, out: &mut Vec<Action>) {
        let n = match self.last_click {
            Some((lt, lp, n)) if t - lt <= DOUBLE_TIME && lp.dist(p) <= DOUBLE_DIST => n + 1,
            _ => 1,
        };
        self.last_click = Some((t, p, n));
        out.push(Action::Down(p, n));
        out.push(Action::Up(p, n));
    }

    /// Start a gesture from nothing.
    fn begin(&mut self, t: f64, ts: &[(u32, Pt)], out: &mut Vec<Action>) {
        match ts.len() {
            0 => {}
            1 => {
                self.save(out);
                out.push(Action::MoveTo(ts[0].1));
                self.state = State::Pending { id: ts[0].0, start: ts[0].1, t0: t, held: false };
            }
            2 => self.begin_two(t, ts, out),
            _ => {
                self.save(out);
                self.state = State::Three { start: centroid(ts), fired: false };
            }
        }
    }

    fn begin_two(&mut self, t: f64, ts: &[(u32, Pt)], out: &mut Vec<Action>) {
        self.save(out);
        let (c, d) = (centroid(ts), spread(ts));
        out.push(Action::MoveTo(c));
        self.state = State::Two { t0: t, at: c, start_c: c, start_d: d, last_c: c, last_d: d, mode: TwoMode::Undecided, samples: vec![(t, c)] };
    }

    /// Fingers lifted mid-gesture: glide if it was a flick, else wait or finish.
    fn release(&mut self, t: f64, at: Pt, samples: &[(f64, Pt)], fingers_left: usize, out: &mut Vec<Action>) {
        let (vx, vy) = velocity(samples, t);
        if fingers_left == 0 && self.opts.momentum && vx.hypot(vy) >= MIN_SPEED * 4.0 {
            self.state = State::Coast { at, vx, vy, t, began: false };
        } else if fingers_left == 0 {
            self.finish(out);
        } else {
            self.state = State::Wait;
        }
    }

    fn step(&mut self, t: f64, ts: &[(u32, Pt)], out: &mut Vec<Action>) {
        let n = ts.len();
        match self.state.clone() {
            State::Idle => self.begin(t, ts, out),

            State::Coast { at, began, .. } => {
                if n == 0 {
                    return;
                }
                if began {
                    out.push(Action::Momentum { at, dx: 0.0, dy: 0.0, phase: Phase::Ended });
                }
                self.state = State::Idle;
                self.begin(t, ts, out);
            }

            State::Wait | State::Three { fired: true, .. } => {
                if n == 0 {
                    self.finish(out);
                }
            }

            State::Three { start, fired: false } => {
                if n == 0 {
                    self.finish(out);
                    return;
                }
                if n < 3 {
                    return;
                }
                let c = centroid(ts);
                let (dx, dy) = (c.x - start.x, c.y - start.y);
                if dx.abs() >= SWIPE && dx.abs() > dy.abs() {
                    out.push(if dx > 0.0 { Action::Back } else { Action::Forward });
                    self.state = State::Three { start, fired: true };
                }
            }

            State::Pending { id, start, t0, held } => {
                let held = held || t - t0 >= HOLD;
                if n == 0 {
                    if held {
                        out.push(Action::RightClick(start));
                    } else {
                        self.click(t, start, out);
                    }
                    self.finish(out);
                } else if n >= 2 {
                    self.state = State::Idle;
                    if n == 2 { self.begin_two(t, ts, out) } else { self.begin(t, ts, out) }
                } else {
                    let p = ts.iter().find(|x| x.0 == id).map(|x| x.1).unwrap_or(ts[0].1);
                    if p.dist(start) <= SLOP {
                        self.state = State::Pending { id, start, t0, held };
                    } else if self.opts.drag_scrolls && !held {
                        out.push(Action::Scroll { at: start, dx: p.x - start.x, dy: p.y - start.y, phase: Phase::Began });
                        self.state = State::Scrolling { id, at: start, last: p, samples: vec![(t0, start), (t, p)] };
                    } else {
                        out.push(Action::Down(start, 1));
                        out.push(Action::Drag(p));
                        self.state = State::Dragging { id, last: p };
                    }
                }
            }

            State::Scrolling { id, at, last, mut samples } => match ts.iter().find(|x| x.0 == id) {
                Some(&(_, p)) if n == 1 => {
                    out.push(Action::Scroll { at, dx: p.x - last.x, dy: p.y - last.y, phase: Phase::Changed });
                    samples.push((t, p));
                    samples.retain(|(st, _)| t - st <= VELOCITY_WINDOW * 2.0);
                    self.state = State::Scrolling { id, at, last: p, samples };
                }
                _ => {
                    out.push(Action::Scroll { at, dx: 0.0, dy: 0.0, phase: Phase::Ended });
                    self.release(t, at, &samples, n, out);
                }
            },

            State::Dragging { id, last } => match ts.iter().find(|x| x.0 == id) {
                Some(&(_, p)) => {
                    if p != last {
                        out.push(Action::Drag(p));
                    }
                    self.state = State::Dragging { id, last: p };
                }
                None => {
                    out.push(Action::Up(last, 1));
                    if n == 0 { self.finish(out) } else { self.state = State::Wait }
                }
            },

            State::Two { t0, at, start_c, start_d, last_c, last_d, mode, mut samples } => {
                if n == 2 {
                    let (c, d) = (centroid(ts), spread(ts));
                    let mut mode = mode;
                    if mode == TwoMode::Undecided {
                        if (d - start_d).abs() >= PINCH_START {
                            mode = TwoMode::Pinch;
                            out.push(Action::Zoom { at, amount: 0.0, phase: Phase::Began });
                        } else if c.dist(start_c) >= SCROLL_START {
                            mode = TwoMode::Scroll;
                            out.push(Action::Scroll { at, dx: c.x - start_c.x, dy: c.y - start_c.y, phase: Phase::Began });
                        }
                    } else if mode == TwoMode::Scroll {
                        out.push(Action::Scroll { at, dx: c.x - last_c.x, dy: c.y - last_c.y, phase: Phase::Changed });
                    }
                    if mode == TwoMode::Pinch && last_d > 0.0 && d != last_d {
                        out.push(Action::Zoom { at, amount: d / last_d - 1.0, phase: Phase::Changed });
                    }
                    samples.push((t, c));
                    samples.retain(|(st, _)| t - st <= VELOCITY_WINDOW * 2.0);
                    // Only track spread once pinching, so the first zoom step
                    // includes the movement that started it.
                    let ld = if mode == TwoMode::Pinch { d } else { last_d };
                    self.state = State::Two { t0, at, start_c, start_d, last_c: c, last_d: ld, mode, samples };
                    return;
                }
                match mode {
                    TwoMode::Undecided if n < 2 && t - t0 <= TWO_TAP => out.push(Action::RightClick(at)),
                    TwoMode::Scroll => out.push(Action::Scroll { at, dx: 0.0, dy: 0.0, phase: Phase::Ended }),
                    TwoMode::Pinch => out.push(Action::Zoom { at, amount: 0.0, phase: Phase::Ended }),
                    _ => {}
                }
                if n >= 3 {
                    self.state = State::Three { start: centroid(ts), fired: false };
                } else if mode == TwoMode::Scroll {
                    self.release(t, at, &samples, n, out);
                } else if n == 0 {
                    self.finish(out);
                } else {
                    self.state = State::Wait;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(t: f64, pts: &[(u32, f64, f64)]) -> Frame {
        Frame { t, touches: pts.iter().map(|&(id, x, y)| (id, Pt::new(x, y))).collect() }
    }

    #[test]
    fn tap_clicks_and_returns_the_cursor() {
        let mut r = Recognizer::default();
        let a = r.frame(&f(0.0, &[(1, 100.0, 50.0)]));
        assert_eq!(a, vec![Action::SaveCursor, Action::MoveTo(Pt::new(100.0, 50.0))]);
        let a = r.frame(&f(0.1, &[]));
        let p = Pt::new(100.0, 50.0);
        assert_eq!(a, vec![Action::Down(p, 1), Action::Up(p, 1), Action::RestoreCursor]);
    }

    #[test]
    fn two_quick_taps_double_click() {
        let mut r = Recognizer::default();
        r.frame(&f(0.0, &[(1, 100.0, 50.0)]));
        r.frame(&f(0.08, &[]));
        r.frame(&f(0.2, &[(2, 104.0, 52.0)]));
        let a = r.frame(&f(0.28, &[]));
        assert!(a.contains(&Action::Down(Pt::new(104.0, 52.0), 2)));
    }

    #[test]
    fn hold_then_lift_right_clicks() {
        let mut r = Recognizer::default();
        r.frame(&f(0.0, &[(1, 10.0, 10.0)]));
        r.tick(0.6);
        let a = r.frame(&f(0.7, &[]));
        assert_eq!(a, vec![Action::RightClick(Pt::new(10.0, 10.0)), Action::RestoreCursor]);
    }

    #[test]
    fn hold_then_drag_click_drags() {
        let mut r = Recognizer::default();
        r.frame(&f(0.0, &[(1, 10.0, 10.0)]));
        r.tick(0.6);
        let a = r.frame(&f(0.7, &[(1, 60.0, 10.0)]));
        assert_eq!(a, vec![Action::Down(Pt::new(10.0, 10.0), 1), Action::Drag(Pt::new(60.0, 10.0))]);
        let a = r.frame(&f(0.8, &[]));
        assert_eq!(a, vec![Action::Up(Pt::new(60.0, 10.0), 1), Action::RestoreCursor]);
    }

    #[test]
    fn one_finger_drag_scrolls_with_the_finger() {
        let mut r = Recognizer::default();
        r.frame(&f(0.0, &[(1, 100.0, 100.0)]));
        let a = r.frame(&f(0.05, &[(1, 100.0, 130.0)]));
        assert_eq!(a, vec![Action::Scroll { at: Pt::new(100.0, 100.0), dx: 0.0, dy: 30.0, phase: Phase::Began }]);
        let a = r.frame(&f(0.1, &[(1, 100.0, 140.0)]));
        assert_eq!(a, vec![Action::Scroll { at: Pt::new(100.0, 100.0), dx: 0.0, dy: 10.0, phase: Phase::Changed }]);
    }

    #[test]
    fn slow_release_ends_scroll_without_momentum() {
        let mut r = Recognizer::default();
        r.frame(&f(0.0, &[(1, 100.0, 100.0)]));
        r.frame(&f(0.05, &[(1, 100.0, 130.0)]));
        r.frame(&f(0.5, &[(1, 100.0, 131.0)]));
        let a = r.frame(&f(0.9, &[]));
        assert_eq!(a.last(), Some(&Action::RestoreCursor));
    }

    #[test]
    fn flick_glides_then_stops() {
        let mut r = Recognizer::default();
        r.frame(&f(0.0, &[(1, 100.0, 100.0)]));
        r.frame(&f(0.02, &[(1, 100.0, 120.0)]));
        r.frame(&f(0.04, &[(1, 100.0, 160.0)]));
        r.frame(&f(0.06, &[(1, 100.0, 200.0)]));
        let a = r.frame(&f(0.07, &[]));
        assert!(!a.contains(&Action::RestoreCursor));
        let first = r.tick(0.08);
        assert!(matches!(first[0], Action::Momentum { phase: Phase::Began, dy, .. } if dy > 0.0));
        let mut t = 0.08;
        let mut ended = false;
        while t < 5.0 && !ended {
            t += 0.008;
            ended = r.tick(t).contains(&Action::RestoreCursor);
        }
        assert!(ended);
    }

    #[test]
    fn two_finger_tap_right_clicks() {
        let mut r = Recognizer::default();
        r.frame(&f(0.0, &[(1, 100.0, 100.0), (2, 200.0, 100.0)]));
        let a = r.frame(&f(0.15, &[]));
        assert_eq!(a, vec![Action::RightClick(Pt::new(150.0, 100.0)), Action::RestoreCursor]);
    }

    #[test]
    fn pinch_zooms() {
        let mut r = Recognizer::default();
        r.frame(&f(0.0, &[(1, 100.0, 100.0), (2, 200.0, 100.0)]));
        let a = r.frame(&f(0.05, &[(1, 80.0, 100.0), (2, 220.0, 100.0)]));
        assert_eq!(a[0], Action::Zoom { at: Pt::new(150.0, 100.0), amount: 0.0, phase: Phase::Began });
        assert!(matches!(a[1], Action::Zoom { phase: Phase::Changed, amount, .. } if (amount - 0.4).abs() < 1e-9));
        let a = r.frame(&f(0.5, &[]));
        assert_eq!(a, vec![Action::Zoom { at: Pt::new(150.0, 100.0), amount: 0.0, phase: Phase::Ended }, Action::RestoreCursor]);
    }

    #[test]
    fn three_finger_swipe_right_goes_back() {
        let mut r = Recognizer::default();
        r.frame(&f(0.0, &[(1, 100.0, 100.0), (2, 150.0, 100.0), (3, 200.0, 100.0)]));
        let a = r.frame(&f(0.1, &[(1, 250.0, 100.0), (2, 300.0, 100.0), (3, 350.0, 100.0)]));
        assert_eq!(a, vec![Action::Back]);
        assert!(r.frame(&f(0.2, &[(1, 450.0, 100.0), (2, 500.0, 100.0), (3, 550.0, 100.0)])).is_empty());
    }

    #[test]
    fn drag_mode_click_drags_without_holding() {
        let mut r = Recognizer::default();
        r.opts.drag_scrolls = false;
        r.frame(&f(0.0, &[(1, 10.0, 10.0)]));
        let a = r.frame(&f(0.05, &[(1, 40.0, 10.0)]));
        assert_eq!(a, vec![Action::Down(Pt::new(10.0, 10.0), 1), Action::Drag(Pt::new(40.0, 10.0))]);
    }

    #[test]
    fn cancel_releases_a_drag() {
        let mut r = Recognizer::default();
        r.opts.drag_scrolls = false;
        r.frame(&f(0.0, &[(1, 10.0, 10.0)]));
        r.frame(&f(0.05, &[(1, 40.0, 10.0)]));
        assert_eq!(r.cancel(), vec![Action::Up(Pt::new(40.0, 10.0), 1), Action::RestoreCursor]);
    }
}
