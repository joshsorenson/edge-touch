//! Menu bar settings, saved as JSON in
//! ~/Library/Application Support/EdgeTouch/settings.json.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Touch is on (Edge Touch holds the panel). Off hands it back.
    pub enabled: bool,
    /// One-finger drag scrolls. Off: it click-drags.
    pub drag_scrolls: bool,
    /// Keep scrolling for a moment after a flick.
    pub momentum: bool,
    /// Reverse the scroll direction if it feels backwards.
    pub flip_scroll: bool,
    /// After each touch, put the cursor back on the main screen.
    pub return_cursor: bool,
    /// Pinch presses Cmd + / Cmd - instead of sending a zoom gesture.
    pub pinch_keys: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { enabled: true, drag_scrolls: true, momentum: true, flip_scroll: false, return_cursor: true, pinch_keys: false }
    }
}

static CURRENT: Mutex<Option<Settings>> = Mutex::new(None);

pub fn dir() -> PathBuf {
    std::env::var("EDGE_TOUCH_DIR").map(PathBuf::from).unwrap_or_else(|_| {
        dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("EdgeTouch")
    })
}

fn path() -> PathBuf {
    dir().join("settings.json")
}

pub fn get() -> Settings {
    let mut cur = CURRENT.lock().unwrap();
    *cur.get_or_insert_with(|| {
        std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    })
}

pub fn update(f: impl FnOnce(&mut Settings)) -> Settings {
    let mut s = get();
    f(&mut s);
    *CURRENT.lock().unwrap() = Some(s);
    let _ = std::fs::create_dir_all(dir());
    if let Ok(json) = serde_json::to_string_pretty(&s) {
        let _ = std::fs::write(path(), json);
    }
    s
}

/// One line in ~/Library/Application Support/EdgeTouch/edge-touch.log.
pub fn log(msg: &str) {
    use std::io::Write;
    eprintln!("Edge Touch: {msg}");
    let _ = std::fs::create_dir_all(dir());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir().join("edge-touch.log")) {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "{secs} {msg}");
    }
}
