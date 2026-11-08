# Edge Touch

A menu bar app that makes the Corsair XENEON Edge a real touchscreen on macOS. Touch any app on the Edge directly, and the cursor goes back to your main screen when you're done.

macOS 27 treats the Edge's touch panel like a trackpad. Edge Touch takes the panel over and turns your fingers into clicks, scrolls and zoom at the spot you touched.

## Install

Download the latest `Edge-Touch-<version>.zip` from Releases, unzip, and drag **Edge Touch** to Applications. Open it and allow **Accessibility** and **Input Monitoring** when macOS asks. Works on Apple silicon and Intel, macOS 13 or later. Signed and notarized.

## Gestures

| Gesture | Does |
| --- | --- |
| Tap | Click (tap twice for a double-click) |
| One finger drag | Scroll, with a glide after a flick |
| Hold, then drag | Click-and-drag (move windows, select text) |
| Hold, then lift | Right-click |
| Tap, then touch and drag | Zoom (down: in, up: out) |
| Swipe in from the left / right side | Back / Forward |
| Swipe up from the bottom | Mission Control |
| Swipe down from the top | App windows |

Pinch, two-finger scroll and three-finger swipes are built in too, but the Edge never sends them to a Mac (see below).

## Why only one finger

The panel is 5+ point multi-touch, but its firmware decides during USB enumeration whether the computer is Windows. macOS reads USB string descriptors length-first, the panel takes that as "not Windows", and it only ever sends one pointer through its mouse interface. The multi-finger digitizer interface stays silent, even after the standard "switch to multi-touch" request Windows sends. This happens before any app runs, so no Mac software can change it. See [hathach/tinyusb#4346](https://github.com/hathach/tinyusb/issues/4346) and [Myseri/xeneon-edge-multitouch-macos](https://github.com/Myseri/xeneon-edge-multitouch-macos).

Ways around it: a small USB adapter (RP2040/RP2350 running TinyUSB) between the panel and the Mac that enumerates it the Windows way, or a firmware fix from Corsair.

## First launch

A setup window walks through the two permissions one at a time (Accessibility, then Input Monitoring), checks each one off as soon as it's on, confirms the Edge and its touch panel are connected, then hands off to the gesture tour. Open it again from the menu bar: **Set Up Permissions…**.

## Gesture tour

On first launch, a tour opens on the Edge and has you try each one-finger gesture: tap, double-tap, scroll, flick, hold for right-click, hold and drag, tap-then-drag zoom and an edge swipe. Each step checks itself off when you do it. Run it again from the menu bar: **Try the Gestures…**. While it's open, edge swipes don't trigger Back or Mission Control.

## Menu and settings

The menu bar icon has the on/off switch (off hands the panel back to Edge Deck), Settings and About. Settings has General (cursor return, open at login, permissions, log), Gestures (every toggle) and About tabs.

## Permissions

macOS asks once for each:

- **Accessibility:** to send clicks and scrolls.
- **Input Monitoring:** to read the touch panel.

## Edge Deck

Only one app can hold the touch panel. While Edge Touch is on, it refreshes `~/Library/Application Support/EdgeTouch/claim` every second, and Edge Deck lets go of the panel. Turn Edge Touch off (or quit it) and Edge Deck takes the panel back within a few seconds.

## Build

```bash
cargo test
scripts/bundle.sh
```

`scripts/release.sh` builds the download: universal, Developer ID signed, notarized, zipped and published as a GitHub release (setup steps are at the top of the script). `scripts/make_icon.py` redraws the app icon.

`bundle.sh` builds `dist/Edge Touch.app`, signs it with your Apple Development identity (so permissions survive rebuilds), installs it in /Applications and opens it.

Log: `~/Library/Application Support/EdgeTouch/edge-touch.log`.

## Code

- `src/hid.rs`: reads the panel (USB 0x27c0:0x0859, 10-finger digitizer) with exclusive access.
- `src/gesture.rs`: fingers to actions. Pure and unit tested.
- `src/output.rs`: actions to macOS events.
- `src/claim.rs`: the handoff with Edge Deck.
- `src/ax.rs`: asks Accessibility what's under a finger (scroll or drag).
- `src/settings.rs`: settings, saved as JSON.
- `src/ui/settings.html`: the settings window (a web view).
- `src/ui/setup.html`: the permissions setup.
- `src/ui/tour.html` + `src/tour.rs`: the gesture tour, and how the touch thread tells it which gestures it saw.

Diagnostic: create `~/Library/Application Support/EdgeTouch/listen` and restart the app. For 45 seconds it watches every interface without taking the panel over and logs which values arrive.
