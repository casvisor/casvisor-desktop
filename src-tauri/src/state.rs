use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{oneshot, watch, Notify};

use crate::config::Config;
use crate::protocol::Hello;

pub const CONSENT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: String,
    pub message: String,
    pub server: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: u64,
    pub session: String,
    pub user: String,
    pub user_display_name: String,
    pub client_ip: String,
    pub permissions: Vec<String>,
    pub accepted: bool,
}

impl SessionInfo {
    pub fn name(&self) -> String {
        if !self.user_display_name.is_empty() {
            self.user_display_name.clone()
        } else {
            self.user.clone()
        }
    }
}

pub struct State {
    app: OnceLock<AppHandle>,
    config: Mutex<Config>,
    pub config_version: watch::Sender<u64>,
    status: Mutex<Status>,
    next_id: AtomicU64,
    session: Mutex<Option<SessionInfo>>,
    consents: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    disconnect: Notify,
}

/// Ends the session when dropped.
pub struct SessionGuard {
    state: Arc<State>,
    pub id: u64,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.state.end_session(self.id);
    }
}

impl State {
    pub fn new(config: Config) -> Arc<State> {
        Arc::new(State {
            app: OnceLock::new(),
            config: Mutex::new(config),
            config_version: watch::channel(0).0,
            status: Mutex::new(Status {
                state: "offline".into(),
                message: String::new(),
                server: String::new(),
            }),
            next_id: AtomicU64::new(1),
            session: Mutex::new(None),
            consents: Mutex::new(HashMap::new()),
            disconnect: Notify::new(),
        })
    }

    pub fn set_app(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }

    fn emit<T: Serialize + Clone>(&self, event: &str, payload: T) {
        if let Some(app) = self.app.get() {
            let _ = app.emit(event, payload);
        }
    }

    pub fn config(&self) -> Config {
        self.config.lock().unwrap().clone()
    }

    pub fn set_config(&self, config: Config) {
        *self.config.lock().unwrap() = config;
        self.config_version.send_modify(|v| *v += 1);
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn set_status(&self, state: &str, message: &str) {
        let status = Status {
            state: state.into(),
            message: message.into(),
            server: self.config().server,
        };
        *self.status.lock().unwrap() = status.clone();
        self.emit("status", status);
    }

    pub fn session(&self) -> Option<SessionInfo> {
        self.session.lock().unwrap().clone()
    }

    /// Starts a session, unless another one is going on.
    pub fn begin_session(self: &Arc<Self>, hello: &Hello) -> Option<SessionGuard> {
        let mut session = self.session.lock().unwrap();
        if session.is_some() {
            return None;
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let info = SessionInfo {
            id,
            session: hello.session.clone(),
            user: hello.user.clone(),
            user_display_name: hello.user_display_name.clone(),
            client_ip: hello.client_ip.clone(),
            permissions: hello.permissions.clone(),
            accepted: false,
        };
        *session = Some(info.clone());
        drop(session);
        self.emit("session", Some(info));
        Some(SessionGuard {
            state: self.clone(),
            id,
        })
    }

    pub fn accept_session(&self, id: u64) {
        let info = {
            let mut session = self.session.lock().unwrap();
            match session.as_mut() {
                Some(info) if info.id == id => {
                    info.accepted = true;
                    info.clone()
                }
                _ => return,
            }
        };
        self.emit("session", Some(info.clone()));
        crate::ui::show_banner(self.app.get(), &info);
    }

    fn end_session(&self, id: u64) {
        let mut session = self.session.lock().unwrap();
        if session.as_ref().map(|s| s.id) != Some(id) {
            return;
        }
        *session = None;
        drop(session);
        self.consents.lock().unwrap().remove(&id);
        self.emit("session", None::<SessionInfo>);
        crate::ui::hide_banner(self.app.get());
    }

    /// Asks the person in front of the computer whether to allow the session.
    pub async fn request_consent(&self, id: u64) -> bool {
        let (tx, rx) = oneshot::channel();
        self.consents.lock().unwrap().insert(id, tx);
        if let Some(app) = self.app.get() {
            crate::ui::show_main(app);
        }
        let accepted = matches!(
            tokio::time::timeout(CONSENT_TIMEOUT, rx).await,
            Ok(Ok(true))
        );
        self.consents.lock().unwrap().remove(&id);
        accepted
    }

    pub fn answer_consent(&self, id: u64, accepted: bool) {
        if let Some(tx) = self.consents.lock().unwrap().remove(&id) {
            let _ = tx.send(accepted);
        }
    }

    pub fn disconnect_session(&self) {
        self.disconnect.notify_waiters();
    }

    pub async fn disconnected(&self) {
        self.disconnect.notified().await;
    }
}

pub fn get(app: &AppHandle) -> Arc<State> {
    app.state::<Arc<State>>().inner().clone()
}
