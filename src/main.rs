//! Edge Touch: a menu bar app that makes the Corsair XENEON Edge a real
//! touchscreen on macOS. Tap, scroll, pinch and swipe on any app on the Edge,
//! and the cursor goes back to your main screen when you're done.
//!
//! - `hid.rs`: reads the touch panel (exclusive access) on its own thread.
//! - `gesture.rs`: turns fingers into clicks, scrolls, zoom (pure, tested).
//! - `output.rs`: sends those as real macOS events.
//! - `claim.rs`: tells Edge Deck to let go of the panel while we use it.
//! - `settings.rs`: the menu's toggles, saved as JSON.

#[cfg(target_os = "macos")]
mod ax;
mod claim;
mod gesture;
#[cfg(target_os = "macos")]
mod hid;
#[cfg(target_os = "macos")]
mod output;
mod settings;

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("Edge Touch only runs on macOS.");
}

#[cfg(target_os = "macos")]
fn main() {
    app::run();
}

#[cfg(target_os = "macos")]
mod app {
    use crate::settings::{self, Settings};
    use crate::{claim, hid};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};
    use tao::event::{Event, StartCause};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    const LABEL: &str = "com.thesqd.edgetouch";

    /// A finger-touch glyph for the menu bar: a ring with a dot. Drawn in code
    /// as a template image, so macOS tints it for light and dark menu bars.
    fn icon(on: bool) -> Icon {
        let size = 36u32;
        let c = size as f64 / 2.0;
        let mut rgba = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                let d = ((x as f64 + 0.5 - c).powi(2) + (y as f64 + 0.5 - c).powi(2)).sqrt();
                let ring = (2.0 - (d - 13.5).abs()).clamp(0.0, 1.0);
                let dot = if on { (6.5 - d).clamp(0.0, 1.0) } else { 0.0 };
                rgba.extend_from_slice(&[0, 0, 0, (ring.max(dot) * 255.0) as u8]);
            }
        }
        Icon::from_rgba(rgba, size, size).expect("icon")
    }

    fn agent_path() -> PathBuf {
        dirs::home_dir().unwrap_or_default().join("Library/LaunchAgents").join(format!("{LABEL}.plist"))
    }

    /// Start at login with a LaunchAgent that opens this app.
    fn set_login(on: bool) {
        let path = agent_path();
        if !on {
            let _ = std::fs::remove_file(path);
            return;
        }
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{exe}</string></array>
  <key>RunAtLoad</key><true/>
  <key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
"#
        );
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, plist);
    }

    fn open_settings(pane: &str) {
        let _ = std::process::Command::new("open").arg(format!("x-apple.systempreferences:com.apple.preference.security?{pane}")).spawn();
    }

    struct Items {
        status: MenuItem,
        enabled: CheckMenuItem,
        drag_scrolls: CheckMenuItem,
        momentum: CheckMenuItem,
        flip: CheckMenuItem,
        return_cursor: CheckMenuItem,
        pinch_keys: CheckMenuItem,
        login: CheckMenuItem,
        ax: MenuItem,
        input: MenuItem,
        quit: MenuItem,
    }

    fn build(s: &Settings) -> (Menu, Items) {
        let items = Items {
            status: MenuItem::new("Edge Touch: starting…", false, None),
            enabled: CheckMenuItem::new("Use the Edge as a touchscreen", true, s.enabled, None),
            drag_scrolls: CheckMenuItem::new("One finger scrolls in pages and lists", true, s.drag_scrolls, None),
            momentum: CheckMenuItem::new("Glide after a flick", true, s.momentum, None),
            flip: CheckMenuItem::new("Flip scroll direction", true, s.flip_scroll, None),
            return_cursor: CheckMenuItem::new("Return cursor to main screen", true, s.return_cursor, None),
            pinch_keys: CheckMenuItem::new("Pinch presses Cmd + / Cmd −", true, s.pinch_keys, None),
            login: CheckMenuItem::new("Open at login", true, agent_path().exists(), None),
            ax: MenuItem::new("Open Accessibility settings…", true, None),
            input: MenuItem::new("Open Input Monitoring settings…", true, None),
            quit: MenuItem::new("Quit Edge Touch", true, None),
        };
        let menu = Menu::new();
        let sep = || PredefinedMenuItem::separator();
        let _ = menu.append_items(&[
            &items.status,
            &sep(),
            &items.enabled,
            &sep(),
            &items.drag_scrolls,
            &items.momentum,
            &items.flip,
            &items.return_cursor,
            &items.pinch_keys,
            &sep(),
            &items.login,
            &items.ax,
            &items.input,
            &sep(),
            &items.quit,
        ]);
        (menu, items)
    }

    pub fn run() {
        settings::log("starting");
        let mut event_loop = EventLoopBuilder::new().build();
        event_loop.set_activation_policy(ActivationPolicy::Accessory);

        hid::start();

        let mut tray: Option<(TrayIcon, Items)> = None;
        let mut shown: Option<hid::Status> = None;
        let menu_events = MenuEvent::receiver();

        event_loop.run(move |event, _, flow| {
            *flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(250));

            if let Event::NewEvents(StartCause::Init) = event {
                let (menu, items) = build(&settings::get());
                let icon = TrayIconBuilder::new()
                    .with_menu(Box::new(menu))
                    .with_icon(icon(true))
                    .with_icon_as_template(true)
                    .with_tooltip("Edge Touch")
                    .build()
                    .expect("menu bar icon");
                tray = Some((icon, items));
            }
            let Some((icon_handle, items)) = tray.as_ref() else { return };

            while let Ok(ev) = menu_events.try_recv() {
                let id = ev.id();
                if id == items.quit.id() {
                    claim::release();
                    settings::log("quit");
                    *flow = ControlFlow::Exit;
                    return;
                } else if id == items.enabled.id() {
                    settings::update(|s| s.enabled = items.enabled.is_checked());
                } else if id == items.drag_scrolls.id() {
                    settings::update(|s| s.drag_scrolls = items.drag_scrolls.is_checked());
                } else if id == items.momentum.id() {
                    settings::update(|s| s.momentum = items.momentum.is_checked());
                } else if id == items.flip.id() {
                    settings::update(|s| s.flip_scroll = items.flip.is_checked());
                } else if id == items.return_cursor.id() {
                    settings::update(|s| s.return_cursor = items.return_cursor.is_checked());
                } else if id == items.pinch_keys.id() {
                    settings::update(|s| s.pinch_keys = items.pinch_keys.is_checked());
                } else if id == items.login.id() {
                    set_login(items.login.is_checked());
                } else if id == items.ax.id() {
                    open_settings("Privacy_Accessibility");
                } else if id == items.input.id() {
                    open_settings("Privacy_ListenEvent");
                }
            }

            let st = hid::status();
            if shown.as_ref() != Some(&st) {
                items.status.set_text(format!("Edge Touch: {}", st.label()));
                let _ = icon_handle.set_icon_with_as_template(Some(icon(st == hid::Status::On)), true);
                shown = Some(st);
            }
        });
    }
}
