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
mod tour;

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
    use serde_json::{json, Value};
    use tao::dpi::{LogicalPosition, LogicalSize};
    use tao::event::{Event, StartCause, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
    use tao::window::{Window, WindowBuilder};
    use wry::{WebView, WebViewBuilder};
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

    /// The menu: status, the on/off switch, Settings and About.
    struct Items {
        status: MenuItem,
        enabled: CheckMenuItem,
        settings: MenuItem,
        tour: MenuItem,
        about: MenuItem,
        quit: MenuItem,
    }

    fn build(s: &Settings) -> (Menu, Items) {
        let items = Items {
            status: MenuItem::new("Edge Touch: starting…", false, None),
            enabled: CheckMenuItem::new("Use the Edge as a touchscreen", true, s.enabled, None),
            settings: MenuItem::new("Settings…", true, None),
            tour: MenuItem::new("Try the Gestures…", true, None),
            about: MenuItem::new("About Edge Touch", true, None),
            quit: MenuItem::new("Quit Edge Touch", true, None),
        };
        let menu = Menu::new();
        let sep = || PredefinedMenuItem::separator();
        let _ = menu.append_items(&[&items.status, &sep(), &items.enabled, &sep(), &items.settings, &items.tour, &items.about, &sep(), &items.quit]);
        (menu, items)
    }

    /// Messages from the settings page (see src/ui/settings.html).
    enum UserEvent {
        /// From the settings window.
        Ipc(String),
        /// From the gesture tour.
        Tour(String),
    }

    /// The settings window and its web view.
    struct SettingsWindow {
        window: Window,
        view: WebView,
    }

    /// Everything the settings page shows, as JSON for its `render()`.
    fn state_json() -> String {
        let st = hid::status();
        json!({
            "settings": settings::get(),
            "status": st.label(),
            "on": st == hid::Status::On,
            "login": agent_path().exists(),
            "accessibility": crate::output::accessibility_ok(false),
            "version": env!("CARGO_PKG_VERSION"),
        })
        .to_string()
    }

    fn open_window(target: &EventLoopWindowTarget<UserEvent>, proxy: &EventLoopProxy<UserEvent>) -> Option<SettingsWindow> {
        let window = WindowBuilder::new()
            .with_title("Edge Touch")
            .with_inner_size(LogicalSize::new(560.0, 600.0))
            .with_min_inner_size(LogicalSize::new(460.0, 420.0))
            .build(target)
            .map_err(|e| settings::log(&format!("settings window: {e}")))
            .ok()?;
        let proxy = proxy.clone();
        let view = WebViewBuilder::new()
            .with_html(include_str!("ui/settings.html"))
            .with_ipc_handler(move |req| {
                let _ = proxy.send_event(UserEvent::Ipc(req.body().clone()));
            })
            .build(&window)
            .map_err(|e| settings::log(&format!("settings view: {e}")))
            .ok()?;
        window.set_focus();
        Some(SettingsWindow { window, view })
    }

    /// The gesture tour, shown on the Edge itself so you practice where
    /// you'll touch. Borderless and above Edge Deck's window.
    fn open_tour(target: &EventLoopWindowTarget<UserEvent>, proxy: &EventLoopProxy<UserEvent>) -> Option<SettingsWindow> {
        let mut builder = WindowBuilder::new().with_title("Try Edge Touch").with_decorations(false).with_always_on_top(true);
        builder = match hid::edge_rect() {
            Some((x, y, w, h)) => {
                let (tw, th) = ((w - 120.0).max(800.0), (h - 60.0).max(400.0));
                builder
                    .with_inner_size(LogicalSize::new(tw, th))
                    .with_position(LogicalPosition::new(x + (w - tw) / 2.0, y + (h - th) / 2.0))
            }
            None => builder.with_inner_size(LogicalSize::new(1600.0, 640.0)),
        };
        let window = builder.build(target).map_err(|e| settings::log(&format!("tour window: {e}"))).ok()?;
        let proxy = proxy.clone();
        let view = WebViewBuilder::new()
            .with_html(include_str!("ui/tour.html"))
            .with_ipc_handler(move |req| {
                let _ = proxy.send_event(UserEvent::Tour(req.body().clone()));
            })
            .build(&window)
            .map_err(|e| settings::log(&format!("tour view: {e}")))
            .ok()?;
        window.set_focus();
        crate::tour::set_active(true);
        Some(SettingsWindow { window, view })
    }

    fn tour_status() -> String {
        let st = hid::status();
        json!({ "on": st == hid::Status::On, "label": st.label() }).to_string()
    }

    /// Handle one message from the settings page.
    fn on_ipc(body: &str, win: &SettingsWindow) {
        let Ok(msg) = serde_json::from_str::<Value>(body) else { return };
        match msg["type"].as_str() {
            Some("set") => {
                let (Some(key), Some(on)) = (msg["key"].as_str(), msg["value"].as_bool()) else { return };
                if key == "login" {
                    set_login(on);
                } else {
                    settings::update(|s| {
                        let mut v = serde_json::to_value(*s).unwrap_or_default();
                        if v.get(key).is_some() {
                            v[key] = Value::Bool(on);
                            if let Ok(next) = serde_json::from_value(v) {
                                *s = next;
                            }
                        }
                    });
                }
            }
            Some("open") => match msg["what"].as_str() {
                Some("accessibility") => open_settings("Privacy_Accessibility"),
                Some("input") => open_settings("Privacy_ListenEvent"),
                Some("website") => drop(std::process::Command::new("open").arg("https://joshsorenson.com").spawn()),
                Some("log") => drop(std::process::Command::new("open").arg("-R").arg(settings::dir().join("edge-touch.log")).spawn()),
                _ => {}
            },
            _ => {}
        }
        let _ = win.view.evaluate_script(&format!("render({})", state_json()));
    }

    pub fn run() {
        settings::log("starting");
        let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
        let proxy = event_loop.create_proxy();

        hid::start();

        let mut tray: Option<(TrayIcon, Items)> = None;
        let mut win: Option<SettingsWindow> = None;
        let mut tour: Option<SettingsWindow> = None;
        let mut open_settings_tab: Option<&str> = None;
        let mut tour_started = false;
        let mut shown: Option<String> = None;
        let mut last_push = Instant::now();
        let menu_events = MenuEvent::receiver();

        event_loop.run(move |event, target, flow| {
            *flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(250));

            match &event {
                Event::NewEvents(StartCause::Init) => {
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
                Event::UserEvent(UserEvent::Ipc(body)) => {
                    if let Some(w) = &win {
                        on_ipc(body, w);
                    }
                }
                Event::UserEvent(UserEvent::Tour(body)) => {
                    let msg: Value = serde_json::from_str(body).unwrap_or_default();
                    match msg["type"].as_str() {
                        Some("step") => settings::log(&format!("tour: {}", msg["id"].as_str().unwrap_or("?"))),
                        Some("done") | Some("skip") | Some("settings") => {
                            settings::update(|s| s.onboarded = true);
                            crate::tour::set_active(false);
                            tour = None;
                            if msg["type"] == "settings" {
                                open_settings_tab = Some("gestures");
                            }
                        }
                        _ => {}
                    }
                    if let Some(t) = &tour {
                        let _ = t.view.evaluate_script(&format!("setStatus({})", tour_status()));
                    }
                }
                Event::WindowEvent { event: WindowEvent::CloseRequested, window_id, .. } => {
                    if tour.as_ref().map(|t| t.window.id()) == Some(*window_id) {
                        crate::tour::set_active(false);
                        tour = None;
                    } else {
                        win = None;
                    }
                }
                _ => {}
            }
            let Some((icon_handle, items)) = tray.as_ref() else { return };

            // Open the settings window on a tab (or bring it forward).
            let mut show = |tab: &str| {
                if win.is_none() {
                    win = open_window(target, &proxy);
                }
                if let Some(w) = &win {
                    w.window.set_focus();
                    let _ = w.view.evaluate_script(&format!("show('{tab}'); render({})", state_json()));
                }
            };

            if let Some(tab) = open_settings_tab.take() {
                show(tab);
            }

            // First launch: the tour. Later: from the menu.
            let mut start_tour = !tour_started && !settings::get().onboarded;
            tour_started = true;

            while let Ok(ev) = menu_events.try_recv() {
                let id = ev.id();
                if id == items.quit.id() {
                    claim::release();
                    settings::log("quit");
                    *flow = ControlFlow::Exit;
                    return;
                } else if id == items.enabled.id() {
                    settings::update(|s| s.enabled = items.enabled.is_checked());
                } else if id == items.settings.id() {
                    show("general");
                } else if id == items.about.id() {
                    show("about");
                } else if id == items.tour.id() {
                    start_tour = true;
                }
            }
            if start_tour {
                match &tour {
                    Some(t) => t.window.set_focus(),
                    None => tour = open_tour(target, &proxy),
                }
            }
            // Gestures the touch thread recognized, for the tour to check off.
            if let Some(t) = &tour {
                for name in crate::tour::drain() {
                    let _ = t.view.evaluate_script(&format!("gesture('{name}')"));
                }
            }

            // Keep the menu and the settings page in step with the driver.
            let st = hid::status();
            let s = settings::get();
            if items.enabled.is_checked() != s.enabled {
                items.enabled.set_checked(s.enabled);
            }
            let key = format!("{st:?}{s:?}");
            if shown.as_ref() != Some(&key) {
                items.status.set_text(format!("Edge Touch: {}", st.label()));
                let _ = icon_handle.set_icon_with_as_template(Some(icon(st == hid::Status::On)), true);
                shown = Some(key);
                last_push = Instant::now() - Duration::from_secs(10);
            }
            if last_push.elapsed() > Duration::from_secs(2) {
                if let Some(w) = &win {
                    let _ = w.view.evaluate_script(&format!("render({})", state_json()));
                }
                if let Some(t) = &tour {
                    let _ = t.view.evaluate_script(&format!("setStatus({})", tour_status()));
                }
                last_push = Instant::now();
            }
        });
    }
}
