//! Named pipes between the processes of Casvisor Desktop when it runs as a
//! service: the agent the service starts in the console session talks to the
//! app of the signed-in user, and asks the service for Ctrl+Alt+Del. Only
//! SYSTEM may connect to the pipes, and the agent checks that the app it
//! connects to is Casvisor Desktop too.

use std::os::windows::io::AsRawHandle;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
use tokio::sync::mpsc;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId};
use windows::Win32::System::RemoteDesktop::{
    ProcessIdToSessionId, WTSFreeMemory, WTSGetActiveConsoleSessionId, WTSQuerySessionInformationW,
    WTSUserName,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::dpapi::SecurityDescriptor;
use crate::state::{AgentToUi, ConfigView, Frontend, SessionInfo, State, Status, UiToAgent};

pub const SERVICE_PIPE: &str = r"\\.\pipe\casvisor-desktop-service";

fn ui_pipe(session_id: u32) -> String {
    format!(r"\\.\pipe\casvisor-desktop-ui-{session_id}")
}

pub fn current_session_id() -> u32 {
    let mut id = 0;
    unsafe {
        let _ = ProcessIdToSessionId(GetCurrentProcessId(), &mut id);
    }
    id
}

pub fn console_session_id() -> u32 {
    unsafe { WTSGetActiveConsoleSessionId() }
}

/// The user signed in to the session, empty at the sign-in screen.
pub fn session_user(session_id: u32) -> String {
    unsafe {
        let mut buf = PWSTR::null();
        let mut len = 0;
        if WTSQuerySessionInformationW(None, session_id, WTSUserName, &mut buf, &mut len).is_err() {
            return String::new();
        }
        let name = buf.to_string().unwrap_or_default();
        WTSFreeMemory(buf.0 as _);
        name
    }
}

fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        result.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

fn is_this_program(pid: u32) -> bool {
    let (Some(path), Ok(exe)) = (process_path(pid), std::env::current_exe()) else {
        return false;
    };
    path.eq_ignore_ascii_case(&exe.to_string_lossy())
}

fn peer_pid(pipe: &impl AsRawHandle, server: bool) -> u32 {
    let handle = HANDLE(pipe.as_raw_handle());
    let mut pid = 0;
    unsafe {
        let _ = if server {
            GetNamedPipeServerProcessId(handle, &mut pid)
        } else {
            GetNamedPipeClientProcessId(handle, &mut pid)
        };
    }
    pid
}

fn create_server(name: &str, sddl: &str) -> Result<NamedPipeServer> {
    let sd = SecurityDescriptor::from_sddl(sddl)?;
    let mut sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0 .0,
        bInheritHandle: false.into(),
    };
    let server = unsafe {
        ServerOptions::new()
            .first_pipe_instance(true)
            .create_with_security_attributes_raw(name, &mut sa as *mut _ as *mut _)?
    };
    Ok(server)
}

async fn send<W: AsyncWrite + Unpin, T: Serialize>(writer: &mut W, message: &T) -> Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    Ok(())
}

fn parse<T: DeserializeOwned>(line: &str) -> Option<T> {
    serde_json::from_str(line).ok()
}

/// The app of the signed-in user, as the agent the service starts sees it.
#[derive(Default)]
pub struct PipeFrontend {
    tx: Mutex<Option<mpsc::UnboundedSender<AgentToUi>>>,
}

impl PipeFrontend {
    fn send(&self, message: AgentToUi) -> bool {
        match self.tx.lock().unwrap().as_ref() {
            Some(tx) => tx.send(message).is_ok(),
            None => false,
        }
    }
}

impl Frontend for PipeFrontend {
    fn status(&self, status: &Status) {
        self.send(AgentToUi::Status {
            status: status.clone(),
        });
    }

    fn session(&self, session: Option<&SessionInfo>) {
        self.send(AgentToUi::Session {
            session: session.cloned(),
        });
    }

    fn ask(&self, session: &SessionInfo) -> Result<(), String> {
        if self.send(AgentToUi::Consent {
            session: session.clone(),
        }) {
            return Ok(());
        }
        if session_user(current_session_id()).is_empty() {
            Err("Nobody is signed in at the remote computer to allow the session. Set it to unattended to connect at the sign-in screen.".into())
        } else {
            Err("Casvisor Desktop isn't open for the signed-in user at the remote computer, so nobody can allow the session.".into())
        }
    }

    fn config(&self, config: &ConfigView) {
        self.send(AgentToUi::Config {
            config: config.clone(),
        });
    }
}

/// Keeps the agent connected to the app of the user signed in to its session.
pub async fn connect_ui(state: Arc<State>, frontend: Arc<PipeFrontend>) {
    let name = ui_pipe(current_session_id());
    loop {
        match ClientOptions::new().open(&name) {
            Ok(client) => {
                if !is_this_program(peer_pid(&client, true)) {
                    log::warn!("{name} isn't served by Casvisor Desktop, ignoring it");
                } else if let Err(err) = serve_ui_link(&state, &frontend, client).await {
                    log::info!("the app went away: {err:#}");
                }
                *frontend.tx.lock().unwrap() = None;
            }
            Err(err) if err.raw_os_error() == Some(231) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn serve_ui_link(
    state: &Arc<State>,
    frontend: &Arc<PipeFrontend>,
    client: tokio::net::windows::named_pipe::NamedPipeClient,
) -> Result<()> {
    let (reader, mut writer) = tokio::io::split(client);
    let (tx, mut rx) = mpsc::unbounded_channel();
    tx.send(AgentToUi::Config {
        config: state.config_view(),
    })?;
    tx.send(AgentToUi::Status {
        status: state.status(),
    })?;
    match state.session() {
        Some(session) if !session.accepted => tx.send(AgentToUi::Consent { session })?,
        session => tx.send(AgentToUi::Session { session })?,
    }
    *frontend.tx.lock().unwrap() = Some(tx);

    let writer_task = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            send(&mut writer, &message).await?;
        }
        anyhow::Ok(())
    });

    let mut lines = BufReader::new(reader).lines();
    let result = loop {
        match lines.next_line().await {
            Ok(Some(line)) => match parse::<UiToAgent>(&line) {
                Some(UiToAgent::Answer { id, accepted }) => state.answer_consent(id, accepted),
                Some(UiToAgent::Disconnect) => state.disconnect_session(),
                None => {}
            },
            Ok(None) => break Ok(()),
            Err(err) => break Err(err.into()),
        }
    };
    writer_task.abort();
    result
}

/// Serves the agent of the service in the app, showing what it reports and
/// passing on what the user answers.
pub async fn serve_ui(state: Arc<State>) {
    let (tx, mut rx) = mpsc::unbounded_channel::<UiToAgent>();
    state.set_remote(tx);
    let name = ui_pipe(current_session_id());
    let offline = |message: &str| Status {
        state: "offline".into(),
        message: message.into(),
        server: state.config_view().server,
    };
    state.mirror_status(offline("Waiting for the Casvisor Desktop service."));

    loop {
        // Only SYSTEM may connect: the agent runs as SYSTEM, and the app,
        // which isn't elevated, couldn't look into a SYSTEM process anyway.
        let server = match create_server(&name, "D:P(A;;GA;;;SY)") {
            Ok(server) => server,
            Err(err) => {
                log::warn!("failed to serve {name}: {err:#}");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        if server.connect().await.is_err() {
            continue;
        }

        let (reader, mut writer) = tokio::io::split(server);
        let mut lines = BufReader::new(reader).lines();
        loop {
            tokio::select! {
                line = lines.next_line() => {
                    let Ok(Some(line)) = line else { break };
                    match parse::<AgentToUi>(&line) {
                        Some(AgentToUi::Status { status }) => state.mirror_status(status),
                        Some(AgentToUi::Session { session }) => state.mirror_session(session),
                        Some(AgentToUi::Consent { session }) => state.mirror_ask(session),
                        Some(AgentToUi::Config { config }) => state.mirror_config(config),
                        None => {}
                    }
                }
                message = rx.recv() => {
                    let Some(message) = message else { return };
                    if send(&mut writer, &message).await.is_err() {
                        break;
                    }
                }
            }
        }
        state.mirror_session(None);
        state.mirror_status(offline("The Casvisor Desktop service isn't running."));
    }
}

/// Serves the agent's requests to the service, only SYSTEM may connect.
pub async fn serve_service(handle: impl Fn(&str) + Send + Sync + 'static) {
    loop {
        let server = match create_server(SERVICE_PIPE, "D:P(A;;GA;;;SY)") {
            Ok(server) => server,
            Err(err) => {
                log::warn!("failed to serve {SERVICE_PIPE}: {err:#}");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        if server.connect().await.is_err() || !is_this_program(peer_pid(&server, false)) {
            continue;
        }
        let mut lines = BufReader::new(server).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            handle(line.trim());
        }
    }
}

pub async fn request_service(command: &str) -> Result<()> {
    let mut client = ClientOptions::new()
        .open(SERVICE_PIPE)
        .map_err(|err| anyhow!("the Casvisor Desktop service isn't running: {err}"))?;
    if !is_this_program(peer_pid(&client, true)) {
        bail!("{SERVICE_PIPE} isn't served by Casvisor Desktop");
    }
    client.write_all(format!("{command}\n").as_bytes()).await?;
    client.flush().await?;
    Ok(())
}
