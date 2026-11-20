use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

use crate::state::{ConfigView, Frontend, SessionInfo, Status};

const BANNER: &str = "banner";
const BANNER_WIDTH: f64 = 460.0;
const BANNER_HEIGHT: f64 = 44.0;

pub fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Keeps a bar on top of everything saying who is controlling the computer,
/// with a button to end it.
fn show_banner(app: &AppHandle, info: &SessionInfo) {
    if let Some(window) = app.get_webview_window(BANNER) {
        let _ = window.show();
        return;
    }

    let url = WebviewUrl::App("index.html".into());
    let window = WebviewWindowBuilder::new(app, BANNER, url)
        .title(format!("{} is controlling this computer", info.name()))
        .inner_size(BANNER_WIDTH, BANNER_HEIGHT)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .shadow(true)
        .build();
    let Ok(window) = window else { return };

    if let Ok(Some(monitor)) = window.primary_monitor() {
        let scale = monitor.scale_factor();
        let size = monitor.size();
        let position = monitor.position();
        let x = position.x + ((size.width as f64 - BANNER_WIDTH * scale) / 2.0) as i32;
        let y = position.y + (8.0 * scale) as i32;
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
}

fn hide_banner(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(BANNER) {
        let _ = window.close();
    }
}

/// The app's own windows.
pub struct TauriFrontend {
    pub app: AppHandle,
}

impl Frontend for TauriFrontend {
    fn status(&self, status: &Status) {
        let _ = self.app.emit("status", status);
    }

    fn session(&self, session: Option<&SessionInfo>) {
        let _ = self.app.emit("session", session);
        match session {
            Some(info) if info.accepted => show_banner(&self.app, info),
            _ => hide_banner(&self.app),
        }
    }

    fn ask(&self, _session: &SessionInfo) -> Result<(), String> {
        show_main(&self.app);
        Ok(())
    }

    fn config(&self, config: &ConfigView) {
        let _ = self.app.emit("config", config);
    }
}
