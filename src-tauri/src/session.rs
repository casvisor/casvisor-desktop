use std::sync::Arc;

use anyhow::{bail, Result};
use russh::client::Msg;
use russh::Channel;
use tokio::sync::mpsc;

use crate::platform::{Capture, Command};
use crate::protocol::{self, Accept, Control, Hello};
use crate::state::State;

/// Serves one `desktop@casvisor.org` channel: asks for consent, then streams
/// the screen and applies the viewer's input until either side ends it.
pub async fn serve(state: Arc<State>, channel: Channel<Msg>) -> Result<()> {
    let (mut reader, mut writer) = tokio::io::split(channel.into_stream());

    let (kind, payload) = protocol::read_message(&mut reader).await?;
    if kind != protocol::HELLO {
        bail!("the session didn't start with hello");
    }
    let hello: Hello = serde_json::from_slice(&payload)?;
    log::info!(
        "remote session {} requested by {} from {}",
        hello.session,
        hello.user,
        hello.client_ip
    );

    let Some(guard) = state.begin_session(&hello) else {
        let accept = Accept {
            accepted: false,
            message: "Another remote session is in progress on this computer.".into(),
        };
        protocol::write_raw(
            &mut writer,
            &protocol::encode_json(protocol::ACCEPT, &accept),
        )
        .await?;
        return Ok(());
    };

    let consent = if state.config().unattended {
        Ok(true)
    } else {
        tokio::select! {
            consent = state.request_consent(guard.id) => consent,
            _ = wait_closed(&mut reader) => return Ok(()),
        }
    };
    let accepted = consent == Ok(true);
    let accept = Accept {
        accepted,
        message: match consent {
            Ok(true) => String::new(),
            Ok(false) => "The person at the remote computer didn't allow the session.".into(),
            Err(reason) => reason,
        },
    };
    protocol::write_raw(
        &mut writer,
        &protocol::encode_json(protocol::ACCEPT, &accept),
    )
    .await?;
    if !accepted {
        return Ok(());
    }
    state.accept_session(guard.id);

    let has = |permission: &str| hello.permissions.iter().any(|p| p == permission);
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(8);
    let capture = match Capture::start(tx.clone(), has("Copy")) {
        Ok(capture) => capture,
        Err(err) => {
            let message = protocol::notice("error", &format!("{err:#}"));
            protocol::write_raw(&mut writer, &message).await?;
            return Err(err);
        }
    };

    let mut writer_task = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            protocol::write_raw(&mut writer, &message).await?;
        }
        anyhow::Ok(())
    });

    let result = loop {
        tokio::select! {
            message = protocol::read_message(&mut reader) => {
                let (kind, payload) = match message {
                    Ok(m) => m,
                    Err(err) => break Err(err),
                };
                handle(&capture, kind, &payload, has("Paste"));
            }
            _ = state.disconnected() => {
                let _ = tx.send(protocol::notice("disconnected", "The person at the remote computer ended the session.")).await;
                break Ok(());
            }
            result = &mut writer_task => {
                break match result {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(err)) => Err(err),
                    Err(err) => Err(err.into()),
                };
            }
        }
    };

    drop(capture);
    drop(tx);
    if !writer_task.is_finished() {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), &mut writer_task).await;
        writer_task.abort();
    }
    drop(guard);
    result
}

/// Resolves when Casvisor closes the channel, e.g. the viewer went away.
async fn wait_closed<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) {
    while protocol::read_message(reader).await.is_ok() {}
}

fn handle(capture: &Capture, kind: u8, payload: &[u8], paste: bool) {
    match kind {
        protocol::MOUSE => {
            if let Some(m) = protocol::parse_mouse(payload) {
                capture.mouse(m);
            }
        }
        protocol::KEY => {
            if let Some(k) = protocol::parse_key(payload) {
                capture.key(k);
            }
        }
        protocol::CONTROL => {
            let Ok(control) = serde_json::from_slice::<Control>(payload) else {
                return;
            };
            match control.kind.as_str() {
                "keyframe" => capture.command(Command::KeyFrame),
                "display" => capture.command(Command::Display(control.index)),
                "ctrl-alt-del" => {
                    tokio::spawn(async {
                        if let Err(err) = crate::platform::send_ctrl_alt_del().await {
                            log::info!("can't send Ctrl+Alt+Del: {err:#}");
                        }
                    });
                }
                _ => {}
            }
        }
        protocol::CLIPBOARD if paste => {
            if let Ok(text) = std::str::from_utf8(payload) {
                capture.set_clipboard(text);
            }
        }
        _ => {}
    }
}
