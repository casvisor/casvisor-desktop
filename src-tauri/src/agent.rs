use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use russh::client::{self, AuthResult, ChannelOpenHandle, Msg, Session};
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{Channel, ChannelId};
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::protocol::{CHANNEL_DESKTOP, TOKEN_PREFIX, USERNAME};
use crate::state::State;
use crate::ws::WsStream;

const DIAL_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
pub const CONNECT_PATH: &str = "/api/connect-agent";

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The device's SSH key, derived from its token the same way as `casvisor agent`.
pub fn key_from_token(token: &str) -> Result<PrivateKey> {
    let token = token.trim();
    if !token.starts_with(TOKEN_PREFIX) {
        bail!("invalid device token, it should start with {TOKEN_PREFIX}");
    }
    let mut hasher = Sha256::new();
    hasher.update(b"casvisor-agent:");
    hasher.update(token.as_bytes());
    let seed: [u8; 32] = hasher.finalize().into();
    Ok(PrivateKey::from(Ed25519Keypair::from_seed(&seed)))
}

/// "https://casvisor.example.com" becomes "wss://casvisor.example.com/api/connect-agent",
/// "host:port" (Casvisor's SSH gateway) returns None.
pub fn websocket_url(server: &str) -> Result<Option<String>> {
    let server = server.trim().trim_end_matches('/');
    let Some((scheme, rest)) = server.split_once("://") else {
        return Ok(None);
    };
    let scheme = match scheme.to_ascii_lowercase().as_str() {
        "https" | "wss" => "wss",
        "http" | "ws" => "ws",
        other => bail!("unsupported scheme: {other}"),
    };
    if rest.is_empty() {
        bail!("no host in {server}");
    }
    Ok(Some(format!("{scheme}://{rest}{CONNECT_PATH}")))
}

struct Client {
    state: Arc<State>,
    fingerprint: String,
    pending: HashSet<ChannelId>,
}

impl client::Handler for Client {
    type Error = anyhow::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return Ok(false);
        };
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        if fingerprint != self.fingerprint {
            log::warn!(
                "the host key of the server is {fingerprint}, not {} in the settings",
                self.fingerprint
            );
            self.state.set_status(
                "error",
                &format!("The server's host key is {fingerprint}, which doesn't match the fingerprint in the settings."),
            );
            return Ok(false);
        }
        Ok(true)
    }

    async fn should_accept_unknown_server_channel(
        &mut self,
        id: ChannelId,
        channel_type: &str,
    ) -> bool {
        if channel_type == CHANNEL_DESKTOP {
            self.pending.insert(id);
            true
        } else {
            false
        }
    }

    async fn server_channel_open_unknown(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<()> {
        if !self.pending.remove(&channel.id()) {
            reply
                .reject(russh::ChannelOpenFailure::UnknownChannelType)
                .await;
            return Ok(());
        }
        reply.accept().await;
        let state = self.state.clone();
        tokio::spawn(async move {
            if let Err(err) = crate::session::serve(state, channel).await {
                log::info!("the remote session ended: {err:#}");
            }
        });
        Ok(())
    }
}

async fn connect(state: &Arc<State>, config: &Config) -> Result<()> {
    let key = key_from_token(&config.token)?;
    let ssh_config = Arc::new(client::Config {
        client_id: russh::SshId::Standard(format!("SSH-2.0-CasvisorDesktop_{}", version()).into()),
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 2,
        inactivity_timeout: None,
        ..Default::default()
    });
    let handler = Client {
        state: state.clone(),
        fingerprint: config.fingerprint.trim().to_string(),
        pending: HashSet::new(),
    };

    let mut handle = match websocket_url(&config.server)? {
        Some(url) => {
            let (ws, _) =
                tokio::time::timeout(DIAL_TIMEOUT, tokio_tungstenite::connect_async(url.as_str()))
                    .await
                    .map_err(|_| anyhow!("timed out connecting to {url}"))?
                    .with_context(|| format!("failed to connect to {url}"))?;
            client::connect_stream(ssh_config, WsStream::new(ws), handler).await?
        }
        None => {
            let tcp = tokio::time::timeout(
                DIAL_TIMEOUT,
                tokio::net::TcpStream::connect(config.server.trim()),
            )
            .await
            .map_err(|_| anyhow!("timed out connecting to {}", config.server))??;
            tcp.set_nodelay(true)?;
            client::connect_stream(ssh_config, tcp, handler).await?
        }
    };

    let auth = handle
        .authenticate_publickey(USERNAME, PrivateKeyWithHashAlg::new(Arc::new(key), None))
        .await?;
    if !matches!(auth, AuthResult::Success) {
        bail!("the device token was rejected, it may have been revoked or replaced, or the device is disabled");
    }

    log::info!("connected to {}", config.server);
    state.set_status("online", "");
    handle.await?;
    Ok(())
}

/// Keeps the device connected to Casvisor, reconnecting with backoff and
/// starting over whenever the settings change.
pub async fn run(state: Arc<State>) {
    let mut version = state.config_version.subscribe();
    let mut delay = Duration::from_secs(1);
    loop {
        version.mark_unchanged();
        let config = state.config();
        if !config.is_complete() {
            state.set_status("unconfigured", "");
            let _ = version.changed().await;
            continue;
        }

        state.set_status("connecting", "");
        let started = std::time::Instant::now();
        let result = tokio::select! {
            result = connect(&state, &config) => result,
            _ = version.changed() => {
                delay = Duration::from_secs(1);
                continue;
            }
        };
        if started.elapsed() > Duration::from_secs(10) {
            delay = Duration::from_secs(1);
        }
        let message = match result {
            Ok(()) => "the connection was closed".to_string(),
            Err(err) => format!("{err:#}"),
        };
        log::info!(
            "disconnected from {}: {message}, reconnecting in {delay:?}",
            config.server
        );
        if state.status().state != "error" {
            state.set_status("offline", &message);
        }

        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = version.changed() => {}
        }
        delay = (delay * 2).min(MAX_RETRY_DELAY);
    }
}
