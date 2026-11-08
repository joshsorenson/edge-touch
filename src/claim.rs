//! Sharing the touch panel with Edge Deck.
//!
//! Only one app can hold the Edge's touch panel. While Edge Touch wants it,
//! it touches a "claim" file every second. Edge Deck checks the file: if it
//! changed in the last few seconds, Edge Deck lets go of the panel. When Edge
//! Touch is turned off or quits, the file goes stale (or is removed) and Edge
//! Deck takes the panel back.
//!
//! The file is ~/Library/Application Support/EdgeTouch/claim.

use std::path::PathBuf;

pub fn path() -> PathBuf {
    crate::settings::dir().join("claim")
}

/// Say "Edge Touch wants the panel" (call about once a second).
pub fn heartbeat() {
    let _ = std::fs::create_dir_all(crate::settings::dir());
    let _ = std::fs::write(path(), std::process::id().to_string());
}

/// Hand the panel back right away.
pub fn release() {
    let _ = std::fs::remove_file(path());
}
