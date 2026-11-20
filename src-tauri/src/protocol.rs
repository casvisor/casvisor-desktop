use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const CHANNEL_DESKTOP: &str = "desktop@casvisor.org";
pub const USERNAME: &str = "casvisor-agent";
pub const TOKEN_PREFIX: &str = "cva_";

pub const HELLO: u8 = 1;
pub const ACCEPT: u8 = 2;
pub const VIDEO_CONFIG: u8 = 16;
pub const VIDEO: u8 = 17;
pub const CURSOR: u8 = 18;
pub const DISPLAYS: u8 = 19;
pub const MOUSE: u8 = 32;
pub const KEY: u8 = 33;
pub const CONTROL: u8 = 34;
pub const CLIPBOARD: u8 = 48;
pub const NOTICE: u8 = 64;

const MAX_MESSAGE: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub session: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub user_display_name: String,
    #[serde(default)]
    pub client_ip: String,
    #[serde(default)]
    pub permissions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Accept {
    pub accepted: bool,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct VideoConfig {
    pub codec: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Display {
    pub index: u32,
    pub name: String,
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

#[derive(Debug, Serialize)]
pub struct Displays {
    pub displays: Vec<Display>,
    pub current: u32,
}

#[derive(Debug, Deserialize)]
pub struct Control {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub index: u32,
}

#[derive(Debug, Serialize)]
pub struct Notice {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy)]
pub struct Mouse {
    pub action: u8,
    pub button: u8,
    pub x: u16,
    pub y: u16,
    pub dx: i16,
    pub dy: i16,
}

#[derive(Debug, Clone, Copy)]
pub struct Key {
    pub scancode: u16,
    pub down: bool,
}

pub fn encode(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(5 + payload.len());
    buf.push(kind);
    buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    buf.extend_from_slice(payload);
    buf
}

pub fn encode_json<T: Serialize>(kind: u8, value: &T) -> Vec<u8> {
    encode(kind, &serde_json::to_vec(value).unwrap_or_default())
}

pub fn encode_video(key: bool, timestamp_us: u64, data: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(9 + data.len());
    payload.push(if key { 1 } else { 0 });
    payload.extend_from_slice(&timestamp_us.to_be_bytes());
    payload.extend_from_slice(data);
    encode(VIDEO, &payload)
}

pub fn encode_cursor(x: i32, y: i32, visible: bool) -> Vec<u8> {
    let mut payload = Vec::with_capacity(9);
    payload.extend_from_slice(&x.to_be_bytes());
    payload.extend_from_slice(&y.to_be_bytes());
    payload.push(visible as u8);
    encode(CURSOR, &payload)
}

pub fn notice(kind: &str, message: &str) -> Vec<u8> {
    encode_json(
        NOTICE,
        &Notice {
            kind: kind.to_string(),
            message: message.to_string(),
        },
    )
}

pub async fn read_message<R: AsyncRead + Unpin>(reader: &mut R) -> Result<(u8, Vec<u8>)> {
    let mut header = [0u8; 5];
    reader.read_exact(&mut header).await?;
    let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if len > MAX_MESSAGE {
        bail!("message too large: {len} bytes");
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    Ok((header[0], payload))
}

pub async fn write_raw<W: AsyncWrite + Unpin>(writer: &mut W, message: &[u8]) -> Result<()> {
    writer.write_all(message).await?;
    writer.flush().await?;
    Ok(())
}

pub fn parse_mouse(payload: &[u8]) -> Option<Mouse> {
    if payload.len() < 10 {
        return None;
    }
    Some(Mouse {
        action: payload[0],
        button: payload[1],
        x: u16::from_be_bytes([payload[2], payload[3]]),
        y: u16::from_be_bytes([payload[4], payload[5]]),
        dx: i16::from_be_bytes([payload[6], payload[7]]),
        dy: i16::from_be_bytes([payload[8], payload[9]]),
    })
}

pub fn parse_key(payload: &[u8]) -> Option<Key> {
    if payload.len() < 3 {
        return None;
    }
    Some(Key {
        scancode: u16::from_be_bytes([payload[0], payload[1]]),
        down: payload[2] != 0,
    })
}
