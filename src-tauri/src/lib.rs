mod agent;
mod config;
mod h264;
mod platform;
mod protocol;
mod session;
mod state;
mod ui;
mod ws;

use std::sync::Arc;

use serde::Deserialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WindowEvent};

use config::Config;
use state::{SessionInfo, State, Status};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    server: String,
    fingerprint: String,
    token: String,
    unattended: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    status: Status,
    session: Option<SessionInfo>,
    server: String,
    fingerprint: String,
    has_token: bool,
    unattended: bool,
    version: String,
}

#[tauri::command]
fn get_snapshot(app: AppHandle) -> Snapshot {
    let state = state::get(&app);
    let config = state.config();
    Snapshot {
        status: state.status(),
        session: state.session(),
        server: config.server,
        fingerprint: config.fingerprint,
        has_token: !config.token.is_empty(),
        unattended: config.unattended,
        version: agent::version().to_string(),
    }
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let state = state::get(&app);
    let mut config = state.config();
    config.server = settings.server.trim().to_string();
    config.fingerprint = settings.fingerprint.trim().to_string();
    if !settings.token.trim().is_empty() {
        let token = settings.token.trim().to_string();
        agent::key_from_token(&token).map_err(|e| e.to_string())?;
        config.token = token;
    }
    config.unattended = settings.unattended;
    agent::websocket_url(&config.server).map_err(|e| e.to_string())?;
    config.save().map_err(|e| format!("{e:#}"))?;
    state.set_config(config);
    Ok(())
}

#[tauri::command]
fn answer_consent(app: AppHandle, id: u64, accepted: bool) {
    state::get(&app).answer_consent(id, accepted);
}

#[tauri::command]
fn disconnect_session(app: AppHandle) {
    state::get(&app).disconnect_session();
}

#[tauri::command]
fn show_main(app: AppHandle) {
    ui::show_main(&app);
}

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Casvisor Desktop", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Casvisor Desktop")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => ui::show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                ui::show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Records a few seconds of the screen to an H.264 file, for checking capture
/// and encoding without Casvisor: `casvisor-desktop --capture-test out.h264`.
fn capture_test(path: &str) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
        let capture = platform::Capture::start(tx, false)?;
        let mut file = Vec::new();
        let (mut frames, mut keys) = (0, 0);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        while let Ok(Some(message)) = tokio::time::timeout_at(deadline, rx.recv()).await {
            match message[0] {
                protocol::VIDEO => {
                    frames += 1;
                    keys += (message[5] & 1) as u32;
                    file.extend_from_slice(&message[14..]);
                }
                protocol::VIDEO_CONFIG | protocol::DISPLAYS | protocol::NOTICE => {
                    println!("{}", String::from_utf8_lossy(&message[5..]));
                }
                _ => {}
            }
        }
        drop(capture);
        std::fs::write(path, &file)?;
        println!("{frames} frames, {keys} key frames, {} bytes", file.len());
        anyhow::Ok(())
    })
}

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,russh=warn"))
        .init();
    platform::init();

    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|arg| arg == "--capture-test") {
        let path = args
            .get(i + 1)
            .map(String::as_str)
            .unwrap_or("capture-test.h264");
        if let Err(err) = capture_test(path) {
            eprintln!("{err:#}");
            std::process::exit(1);
        }
        return;
    }

    let start_hidden = std::env::args().any(|arg| arg == "--hidden");
    let state = State::new(Config::load());

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            ui::show_main(app);
        }))
        .manage(state.clone())
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            save_settings,
            answer_consent,
            disconnect_session,
            show_main
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            state.set_app(handle.clone());
            setup_tray(&handle)?;
            if start_hidden {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            let state: Arc<State> = state.clone();
            tauri::async_runtime::spawn(agent::run(state));
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to run Casvisor Desktop");
}
