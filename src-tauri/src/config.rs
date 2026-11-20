use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::platform;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default)]
    pub server: String,
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub unattended: bool,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredConfig {
    #[serde(default)]
    server: String,
    #[serde(default)]
    fingerprint: String,
    #[serde(default)]
    protected_token: String,
    #[serde(default)]
    unattended: bool,
}

/// Where the settings are: the user's own when the app runs the agent itself,
/// the machine's when the service does, readable only by SYSTEM and administrators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    User,
    Machine,
}

pub fn dir(scope: Scope) -> PathBuf {
    let base = match scope {
        Scope::User => std::env::var_os("APPDATA")
            .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))),
        Scope::Machine => std::env::var_os("ProgramData").map(PathBuf::from),
    };
    base.unwrap_or_else(|| PathBuf::from("."))
        .join("Casvisor Desktop")
}

fn path(scope: Scope) -> PathBuf {
    dir(scope).join("config.json")
}

pub fn modified(scope: Scope) -> Option<SystemTime> {
    std::fs::metadata(path(scope))
        .and_then(|m| m.modified())
        .ok()
}

impl Config {
    pub fn load(scope: Scope) -> Config {
        let stored: StoredConfig = std::fs::read(path(scope))
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();

        let token = STANDARD
            .decode(stored.protected_token.as_bytes())
            .ok()
            .and_then(|data| platform::unprotect(&data, scope == Scope::Machine).ok())
            .and_then(|data| String::from_utf8(data).ok())
            .unwrap_or_default();

        Config {
            server: stored.server,
            fingerprint: stored.fingerprint,
            token,
            unattended: stored.unattended,
        }
    }

    pub fn save(&self, scope: Scope) -> Result<()> {
        let protected = platform::protect(self.token.as_bytes(), scope == Scope::Machine)?;
        let stored = StoredConfig {
            server: self.server.trim().to_string(),
            fingerprint: self.fingerprint.trim().to_string(),
            protected_token: STANDARD.encode(protected),
            unattended: self.unattended,
        };
        let dir = dir(scope);
        std::fs::create_dir_all(&dir).context("failed to create the config directory")?;
        if scope == Scope::Machine {
            platform::restrict_to_admins(&dir)?;
        }
        std::fs::write(path(scope), serde_json::to_vec_pretty(&stored)?)
            .context("failed to write the config")?;
        Ok(())
    }

    pub fn is_complete(&self) -> bool {
        !self.server.is_empty() && !self.token.is_empty() && !self.fingerprint.is_empty()
    }
}
