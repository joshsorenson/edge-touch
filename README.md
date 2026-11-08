# Edge Touch

A menu bar app that makes the Corsair XENEON Edge a real touchscreen on macOS. Touch any app on the Edge directly, and the cursor goes back to your main screen when you're done.

macOS 27 treats the Edge's touch panel like a trackpad. Edge Touch takes the panel over and turns your fingers into clicks, scrolls and zoom at the spot you touched.

## Gestures

| Gesture | Does |
| --- | --- |
| Tap | Click (tap twice for a double-click) |
| One finger drag | Scroll, with a glide after a flick |
| Hold, then drag | Click-and-drag (move windows, select text) |
| Hold, then lift | Right-click |
| Two finger drag | Scroll |
| Pinch | Zoom |
| Two finger tap | Right-click |
| Three finger swipe | Right: Back. Left: Forward |

## Menu

- Use the Edge as a touchscreen: on/off. Off hands the panel back to Edge Deck.
- One finger scrolls: turn off to make one-finger drag a click-drag.
- Glide after a flick, Flip scroll direction, Return cursor to main screen.
- Pinch presses Cmd + / Cmd −: for apps that ignore the zoom gesture.
- Open at login.

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

`bundle.sh` builds `dist/Edge Touch.app`, signs it with your Apple Development identity (so permissions survive rebuilds), installs it in /Applications and opens it.

Log: `~/Library/Application Support/EdgeTouch/edge-touch.log`.

## Code

- `src/hid.rs`: reads the panel (USB 0x27c0:0x0859, 10-finger digitizer) with exclusive access.
- `src/gesture.rs`: fingers to actions. Pure and unit tested.
- `src/output.rs`: actions to macOS events.
- `src/claim.rs`: the handoff with Edge Deck.
- `src/settings.rs`: menu toggles, saved as JSON.
