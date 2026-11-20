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
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WindowEvent};

use config::{Config, Scope};
use state::{ConfigView, SessionInfo, State, Status};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    server: String,
    fingerprint: String,
    token: String,
    unattended: bool,
}

impl Settings {
    fn apply(self, config: &mut Config) -> anyhow::Result<()> {
        let server = self.server.trim().to_string();
        agent::websocket_url(&server)?;
        let token = self.token.trim().to_string();
        if !token.is_empty() {
            agent::key_from_token(&token)?;
            config.token = token;
        }
        config.server = server;
        config.fingerprint = self.fingerprint.trim().to_string();
        config.unattended = self.unattended;
        Ok(())
    }
}

/// Whether the Windows service runs the agent, the app then only asks the
/// user and shows the bar.
struct Mode {
    service: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    status: Status,
    session: Option<SessionInfo>,
    config: ConfigView,
    service: bool,
    version: String,
}

fn get_state(app: &AppHandle) -> Arc<State> {
    app.state::<Arc<State>>().inner().clone()
}

#[tauri::command]
fn get_snapshot(app: AppHandle) -> Snapshot {
    let state = get_state(&app);
    Snapshot {
        status: state.status(),
        session: state.session(),
        config: state.config_view(),
        service: app.state::<Mode>().service,
        version: agent::version().to_string(),
    }
}

#[cfg(windows)]
async fn run_elevated(args: Vec<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        platform::service::run_elevated(&args).map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(not(windows))]
async fn run_elevated(_args: Vec<String>) -> Result<(), String> {
    Err("the service is only for Windows".into())
}

#[tauri::command]
async fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    if app.state::<Mode>().service {
        // The machine's settings are for administrators only.
        settings
            .clone()
            .apply(&mut Config::default())
            .map_err(|e| e.to_string())?;
        let path = std::env::temp_dir().join(format!(
            "casvisor-desktop-settings-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            serde_json::to_vec(&settings).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let result = run_elevated(vec![
            "--configure".into(),
            path.to_string_lossy().to_string(),
        ])
        .await;
        let _ = std::fs::remove_file(&path);
        return result;
    }

    let state = get_state(&app);
    let mut config = state.config();
    settings.apply(&mut config).map_err(|e| e.to_string())?;
    config.save(Scope::User).map_err(|e| format!("{e:#}"))?;
    state.set_config(config);
    Ok(())
}

#[tauri::command]
async fn install_service(app: AppHandle) -> Result<(), String> {
    run_elevated(vec!["--install-service".into()]).await?;
    app.restart();
}

#[tauri::command]
async fn uninstall_service(app: AppHandle) -> Result<(), String> {
    run_elevated(vec!["--uninstall-service".into()]).await?;
    app.restart();
}

#[tauri::command]
fn answer_consent(app: AppHandle, id: u64, accepted: bool) {
    get_state(&app).answer_consent(id, accepted);
}

#[tauri::command]
fn disconnect_session(app: AppHandle) {
    get_state(&app).disconnect_session();
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
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
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

fn logger() -> env_logger::Builder {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,russh=warn"))
}

/// The service and its agent have no console, they log to
/// %ProgramData%\Casvisor Desktop\logs.
fn log_to_file(name: &str) {
    let dir = config::dir(Scope::Machine).join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}.log"));
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 10 * 1024 * 1024) {
        let _ = std::fs::remove_file(&path);
    }
    let mut builder = logger();
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        builder.target(env_logger::Target::Pipe(Box::new(file)));
    }
    builder.init();
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|i| args.get(i + 1).cloned())
}

/// Applies the settings the app passed in a file to the machine.
fn configure(path: &str) -> anyhow::Result<()> {
    let data = std::fs::read(path)?;
    let _ = std::fs::remove_file(path);
    let settings: Settings = serde_json::from_slice(&data)?;
    let mut config = Config::load(Scope::Machine);
    settings.apply(&mut config)?;
    config.save(Scope::Machine)
}

#[cfg(windows)]
fn admin_command(args: &[String]) -> Option<anyhow::Result<()>> {
    if args.iter().any(|a| a == "--install-service") {
        Some(platform::service::install())
    } else if args.iter().any(|a| a == "--uninstall-service") {
        Some(platform::service::uninstall())
    } else {
        arg_value(args, "--configure").map(|path| configure(&path))
    }
}

#[cfg(not(windows))]
fn admin_command(args: &[String]) -> Option<anyhow::Result<()>> {
    arg_value(args, "--configure").map(|path| configure(&path))
}

/// The agent the service starts in the console session: connects to Casvisor
/// with the machine's settings and asks the signed-in user through the app.
#[cfg(windows)]
fn run_agent() -> anyhow::Result<()> {
    let state = State::new(Config::load(Scope::Machine));
    let frontend = Arc::new(platform::pipe::PipeFrontend::default());
    state.set_frontend(frontend.clone());
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        tokio::spawn(agent::run(state.clone()));
        tokio::spawn(platform::pipe::connect_ui(state.clone(), frontend));
        let mut modified = config::modified(Scope::Machine);
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let now = config::modified(Scope::Machine);
            if now != modified {
                modified = now;
                log::info!("the settings changed, reconnecting");
                state.set_config(Config::load(Scope::Machine));
            }
        }
    })
}

fn service_installed() -> bool {
    #[cfg(windows)]
    {
        platform::service::is_installed()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    let has = |name: &str| args.iter().any(|arg| arg == name);

    #[cfg(windows)]
    if has("--service") {
        log_to_file("service");
        if let Err(err) = platform::service::run() {
            log::error!("{err:#}");
        }
        return;
    }

    platform::init();

    #[cfg(windows)]
    if has("--agent") {
        log_to_file("agent");
        if let Err(err) = run_agent() {
            log::error!("{err:#}");
        }
        return;
    }

    logger().init();

    if let Some(result) = admin_command(&args) {
        let text = match &result {
            Ok(()) => "ok".to_string(),
            Err(err) => format!("{err:#}"),
        };
        if let Some(path) = arg_value(&args, "--result") {
            let _ = std::fs::write(path, &text);
        }
        println!("{text}");
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }

    if let Some(path) = arg_value(&args, "--capture-test") {
        if let Err(err) = capture_test(&path) {
            eprintln!("{err:#}");
            std::process::exit(1);
        }
        return;
    }

    let start_hidden = has("--hidden");
    let service = service_installed();
    let state = State::new(if service {
        Config::default()
    } else {
        Config::load(Scope::User)
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            ui::show_main(app);
        }))
        .manage(state.clone())
        .manage(Mode { service })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            save_settings,
            install_service,
            uninstall_service,
            answer_consent,
            disconnect_session,
            show_main
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            state.set_frontend(Arc::new(ui::TauriFrontend {
                app: handle.clone(),
            }));
            setup_tray(&handle)?;
            if start_hidden {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            let state: Arc<State> = state.clone();
            if service {
                #[cfg(windows)]
                tauri::async_runtime::spawn(platform::pipe::serve_ui(state));
            } else {
                tauri::async_runtime::spawn(agent::run(state));
            }
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
