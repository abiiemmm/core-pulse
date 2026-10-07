//! Native window/tray lifecycle. No generic shell or frontend tray permissions.
use crate::{AppState, begin_shutdown};
use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager, Wry, menu::{Menu, MenuItem}, tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent}};

pub struct Tray {
    icon: Option<TrayIcon>,
    open: Option<MenuItem<Wry>>,
    quit: Option<MenuItem<Wry>>,
}

#[derive(Serialize)]
pub struct Status {
    pub tray_available: bool,
    pub window_visible: bool,
    pub window_minimized: bool,
}

#[derive(Debug, PartialEq)]
pub enum CloseAction { Hide, Shutdown, Wait, Allow }

pub fn close_action(closing: u8, close_to_tray: bool, tray_available: bool) -> CloseAction {
    match closing {
        0 if close_to_tray && tray_available => CloseAction::Hide,
        0 => CloseAction::Shutdown,
        2 => CloseAction::Allow,
        _ => CloseAction::Wait,
    }
}

// Unknown window state is treated as background; explicit pause always wins.
pub fn sampling_enabled(requested: bool, background: bool, presence: Option<(bool, bool)>) -> bool {
    requested && (background || presence.is_some_and(|(visible, minimized)| visible && !minimized))
}

fn labels(language: &str) -> (&'static str, &'static str) {
    match language {
        "en" => ("Open Core Pulse", "Quit Core Pulse"),
        "es" => ("Abrir Core Pulse", "Salir de Core Pulse"),
        _ => ("Buka Core Pulse", "Keluar dari Core Pulse"),
    }
}

impl Tray {
    pub fn install(app: &tauri::AppHandle, language: &str) -> Self {
        match Self::build(app, language) {
            Ok(tray) => tray,
            // Tray is optional. A failed tray must never leave a hidden, unreachable app.
            Err(_) => Self { icon: None, open: None, quit: None },
        }
    }

    fn build(app: &tauri::AppHandle, language: &str) -> tauri::Result<Self> {
        let (open_label, quit_label) = labels(language);
        let open = MenuItem::with_id(app, "core-pulse-open", open_label, true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "core-pulse-quit", quit_label, true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&open, &quit])?;
        let Some(icon) = app.default_window_icon() else { return Ok(Self { icon: None, open: None, quit: None }); };
        let icon = TrayIconBuilder::with_id("core-pulse-tray")
            .icon(icon.clone()).tooltip("Core Pulse").menu(&menu)
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event| match event.id.as_ref() {
                "core-pulse-open" => { if let Err(error) = show_main(app) { report(app, &error); } },
                "core-pulse-quit" => quit_application(app),
                _ => {},
            })
            .on_tray_icon_event(|tray, event| {
                if matches!(event, TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }) {
                    if let Err(error) = show_main(tray.app_handle()) { report(tray.app_handle(), &error); }
                }
            })
            .build(app)?;
        Ok(Self { icon: Some(icon), open: Some(open), quit: Some(quit) })
    }

    pub fn available(&self) -> bool { self.icon.is_some() }
}

pub fn show_main(app: &tauri::AppHandle) -> Result<(), String> {
    if app.try_state::<AppState>().is_some_and(|state| state.inner.closing.load(Ordering::Acquire) != 0) { return Ok(()); }
    let window = app.get_webview_window("main").ok_or("Jendela utama tidak tersedia")?;
    window.show().and_then(|_| window.unminimize()).and_then(|_| window.set_focus())
        .map_err(|_| "Jendela utama tidak dapat dibuka kembali".into())
}

pub fn quit_application(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<AppState>() { begin_shutdown(app, state.inner.clone(), 0); }
}

pub fn report(app: &tauri::AppHandle, message: &str) {
    let _ = app.emit("app:error", serde_json::json!({"message":message,"status":"error","timestamp":chrono::Utc::now().to_rfc3339()}));
}

pub fn status(app: &tauri::AppHandle) -> Result<Status, String> {
    let window = app.get_webview_window("main").ok_or("Jendela utama tidak tersedia")?;
    Ok(Status {
        tray_available: app.try_state::<Tray>().is_some_and(|tray| tray.available()),
        window_visible: window.is_visible().map_err(|_| "Status jendela tidak tersedia")?,
        window_minimized: window.is_minimized().map_err(|_| "Status jendela tidak tersedia")?,
    })
}

pub fn refresh_labels(app: &tauri::AppHandle) {
    let handle = app.clone();
    // Acquire settings only on the UI thread, after the settings worker releases locks.
    // Menu setters can dispatch to that thread; never call them while holding a state lock.
    let _ = app.run_on_main_thread(move || {
        let Some(state) = handle.try_state::<AppState>() else { return; };
        let language = match state.inner.settings.lock() { Ok(settings) => settings.language.clone(), Err(_) => return };
        let Some(tray) = handle.try_state::<Tray>() else { return; };
        let (open_label, quit_label) = labels(&language);
        let result = (|| -> tauri::Result<()> {
            if let Some(open) = &tray.open { open.set_text(open_label)?; }
            if let Some(quit) = &tray.quit { quit.set_text(quit_label)?; }
            Ok(())
        })();
        if result.is_err() { report(&handle, "Bahasa menu system tray belum diperbarui"); }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_keeps_default_shutdown_and_never_hides_without_a_tray() {
        assert_eq!(close_action(0, false, true), CloseAction::Shutdown);
        assert_eq!(close_action(0, true, false), CloseAction::Shutdown);
        assert_eq!(close_action(0, true, true), CloseAction::Hide);
        // Once shutdown starts, another close cannot convert it into a hidden app.
        assert_eq!(close_action(1, true, true), CloseAction::Wait);
        assert_eq!(close_action(2, true, true), CloseAction::Allow);
    }

    #[test]
    fn hidden_minimized_and_unknown_windows_follow_background_opt_in_and_pause() {
        for presence in [Some((false, false)), Some((true, true)), None] {
            assert!(!sampling_enabled(true, false, presence));
            assert!(sampling_enabled(true, true, presence));
            assert!(!sampling_enabled(false, true, presence));
        }
        assert!(sampling_enabled(true, false, Some((true, false))));
        assert!(!sampling_enabled(false, false, Some((true, false))));
    }
}
