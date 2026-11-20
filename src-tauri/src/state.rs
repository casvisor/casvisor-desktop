use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch, Notify};

use crate::config::Config;
use crate::protocol::Hello;

pub const CONSENT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: String,
    pub message: String,
    pub server: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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

/// The settings as the UI shows them, without the token.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigView {
    pub server: String,
    pub fingerprint: String,
    pub has_token: bool,
    pub unattended: bool,
}

impl From<&Config> for ConfigView {
    fn from(config: &Config) -> Self {
        ConfigView {
            server: config.server.clone(),
            fingerprint: config.fingerprint.clone(),
            has_token: !config.token.is_empty(),
            unattended: config.unattended,
        }
    }
}

/// What the person at the computer sees: the app's own windows, or, for the
/// agent the service starts, the app of the signed-in user over a pipe.
pub trait Frontend: Send + Sync {
    fn status(&self, status: &Status);
    fn session(&self, session: Option<&SessionInfo>);
    /// Shows the request to allow a session, or says why nobody can be asked.
    fn ask(&self, session: &SessionInfo) -> Result<(), String>;
    fn config(&self, config: &ConfigView);
}

/// What the app tells the agent of the service.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UiToAgent {
    Answer { id: u64, accepted: bool },
    Disconnect,
}

/// What the agent of the service tells the app.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentToUi {
    Status { status: Status },
    Session { session: Option<SessionInfo> },
    Consent { session: SessionInfo },
    Config { config: ConfigView },
}

pub struct State {
    frontend: OnceLock<Arc<dyn Frontend>>,
    /// set in the app when the service runs the agent
    remote: OnceLock<mpsc::UnboundedSender<UiToAgent>>,
    config: Mutex<Config>,
    config_view: Mutex<Option<ConfigView>>,
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
            frontend: OnceLock::new(),
            remote: OnceLock::new(),
            config: Mutex::new(config),
            config_view: Mutex::new(None),
            config_version: watch::channel(0).0,
            status: Mutex::new(Status {
                state: "offline".into(),
                ..Default::default()
            }),
            next_id: AtomicU64::new(1),
            session: Mutex::new(None),
            consents: Mutex::new(HashMap::new()),
            disconnect: Notify::new(),
        })
    }

    pub fn set_frontend(&self, frontend: Arc<dyn Frontend>) {
        let _ = self.frontend.set(frontend);
    }

    pub fn set_remote(&self, remote: mpsc::UnboundedSender<UiToAgent>) {
        let _ = self.remote.set(remote);
    }

    fn frontend(&self) -> Option<&Arc<dyn Frontend>> {
        self.frontend.get()
    }

    pub fn config(&self) -> Config {
        self.config.lock().unwrap().clone()
    }

    pub fn set_config(&self, config: Config) {
        let view = ConfigView::from(&config);
        *self.config.lock().unwrap() = config;
        self.config_version.send_modify(|v| *v += 1);
        if let Some(frontend) = self.frontend() {
            frontend.config(&view);
        }
    }

    pub fn config_view(&self) -> ConfigView {
        if let Some(view) = self.config_view.lock().unwrap().clone() {
            return view;
        }
        ConfigView::from(&*self.config.lock().unwrap())
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
        self.mirror_status(status);
    }

    /// The app showing what the agent of the service reports.
    pub fn mirror_status(&self, status: Status) {
        *self.status.lock().unwrap() = status.clone();
        if let Some(frontend) = self.frontend() {
            frontend.status(&status);
        }
    }

    pub fn mirror_session(&self, session: Option<SessionInfo>) {
        *self.session.lock().unwrap() = session.clone();
        if let Some(frontend) = self.frontend() {
            frontend.session(session.as_ref());
        }
    }

    pub fn mirror_config(&self, view: ConfigView) {
        *self.config_view.lock().unwrap() = Some(view.clone());
        if let Some(frontend) = self.frontend() {
            frontend.config(&view);
        }
    }

    /// The app showing the request the agent of the service got.
    pub fn mirror_ask(&self, session: SessionInfo) {
        self.mirror_session(Some(session.clone()));
        if let Some(frontend) = self.frontend() {
            let _ = frontend.ask(&session);
        }
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
        if let Some(frontend) = self.frontend() {
            frontend.session(Some(&info));
        }
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
        if let Some(frontend) = self.frontend() {
            frontend.session(Some(&info));
        }
    }

    fn end_session(&self, id: u64) {
        let mut session = self.session.lock().unwrap();
        if session.as_ref().map(|s| s.id) != Some(id) {
            return;
        }
        *session = None;
        drop(session);
        self.consents.lock().unwrap().remove(&id);
        if let Some(frontend) = self.frontend() {
            frontend.session(None);
        }
    }

    /// Asks the person in front of the computer whether to allow the session,
    /// or says why nobody can be asked.
    pub async fn request_consent(&self, id: u64) -> Result<bool, String> {
        let Some(info) = self.session().filter(|s| s.id == id) else {
            return Ok(false);
        };
        let (tx, rx) = oneshot::channel();
        self.consents.lock().unwrap().insert(id, tx);
        if let Some(frontend) = self.frontend() {
            if let Err(reason) = frontend.ask(&info) {
                self.consents.lock().unwrap().remove(&id);
                return Err(reason);
            }
        }
        let accepted = matches!(
            tokio::time::timeout(CONSENT_TIMEOUT, rx).await,
            Ok(Ok(true))
        );
        self.consents.lock().unwrap().remove(&id);
        Ok(accepted)
    }

    pub fn answer_consent(&self, id: u64, accepted: bool) {
        if let Some(remote) = self.remote.get() {
            let _ = remote.send(UiToAgent::Answer { id, accepted });
            return;
        }
        if let Some(tx) = self.consents.lock().unwrap().remove(&id) {
            let _ = tx.send(accepted);
        }
    }

    pub fn disconnect_session(&self) {
        if let Some(remote) = self.remote.get() {
            let _ = remote.send(UiToAgent::Disconnect);
            return;
        }
        self.disconnect.notify_waiters();
    }

    pub async fn disconnected(&self) {
        self.disconnect.notified().await;
    }
}
