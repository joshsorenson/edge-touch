//! The gesture tour (onboarding). While it's open, the touch thread reports
//! each gesture it recognizes here, by name, so the tour page can check off
//! the steps it can't see for itself (glide, zoom, edge swipes). System
//! shortcuts (Back, Mission Control) are held back while it's open, so trying
//! a swipe doesn't throw you out of the tour.

use crate::gesture::{Action, Phase};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static SEEN: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

pub fn set_active(on: bool) {
    ACTIVE.store(on, Ordering::Relaxed);
    SEEN.lock().unwrap().clear();
}

pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Gesture names in one batch of actions.
pub fn names(acts: &[Action]) -> Vec<&'static str> {
    let mut out = vec![];
    let mut down = false;
    for a in acts {
        match a {
            Action::Down(..) => down = true,
            Action::Up(_, n) if down => out.push(if *n >= 2 { "double" } else { "tap" }),
            Action::Drag(_) if down => out.push("drag"),
            Action::RightClick(_) => out.push("rightclick"),
            Action::Scroll { phase: Phase::Began, .. } => out.push("scroll"),
            Action::Momentum { phase: Phase::Began, .. } => out.push("glide"),
            Action::Zoom { phase: Phase::Began, .. } => out.push("zoom"),
            Action::Back => out.push("back"),
            Action::Forward => out.push("forward"),
            Action::MissionControl => out.push("mission"),
            Action::AppWindows => out.push("appwindows"),
            _ => {}
        }
    }
    out
}

/// Called by the touch thread with every batch of actions. Returns the
/// actions to actually send (system shortcuts removed while the tour is open).
pub fn observe(acts: Vec<Action>) -> Vec<Action> {
    if !active() {
        return acts;
    }
    let found = names(&acts);
    if !found.is_empty() {
        SEEN.lock().unwrap().extend(found);
    }
    acts.into_iter()
        .filter(|a| !matches!(a, Action::Back | Action::Forward | Action::MissionControl | Action::AppWindows))
        .collect()
}

/// Gestures recognized since the last call.
pub fn drain() -> Vec<&'static str> {
    std::mem::take(&mut *SEEN.lock().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gesture::Pt;

    #[test]
    fn names_taps_drags_and_swipes() {
        let p = Pt::new(1.0, 1.0);
        assert_eq!(names(&[Action::Down(p, 1), Action::Up(p, 1), Action::RestoreCursor]), vec!["tap"]);
        assert_eq!(names(&[Action::Down(p, 2), Action::Up(p, 2)]), vec!["double"]);
        assert_eq!(names(&[Action::Down(p, 1), Action::Drag(p)]), vec!["drag"]);
        assert_eq!(names(&[Action::Back]), vec!["back"]);
        assert!(names(&[Action::Up(p, 1)]).is_empty());
    }

    #[test]
    fn tour_holds_back_system_shortcuts() {
        set_active(true);
        let kept = observe(vec![Action::MissionControl, Action::RestoreCursor]);
        assert_eq!(kept, vec![Action::RestoreCursor]);
        assert_eq!(drain(), vec!["mission"]);
        set_active(false);
        assert_eq!(observe(vec![Action::Back]), vec![Action::Back]);
    }
}
