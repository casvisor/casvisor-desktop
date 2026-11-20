mod clipboard;
mod convert;
mod dpapi;
mod dxgi;
mod encoder;
mod input;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::mpsc;
use windows::Win32::Foundation::POINT;
use windows::Win32::Media::MediaFoundation::{MFShutdown, MFStartup, MFSTARTUP_FULL, MF_VERSION};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorInfo, CURSORINFO, CURSOR_SHOWING};

pub use dpapi::{protect, unprotect};
use input::Geometry;

use crate::h264;
use crate::protocol::{self, Displays, Key, Mouse, VideoConfig};

const FPS: u32 = 30;

pub fn init() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub enum Command {
    KeyFrame,
    Display(u32),
    Stop,
}

pub struct Capture {
    commands: Sender<Command>,
    geometry: Arc<Mutex<Geometry>>,
    clipboard_seq: Arc<AtomicU32>,
}

impl Capture {
    pub fn start(out: mpsc::Sender<Vec<u8>>, copy: bool) -> Result<Capture> {
        let (commands, rx) = std::sync::mpsc::channel();
        let geometry = Arc::new(Mutex::new(Geometry {
            left: 0,
            top: 0,
            scale: 1,
        }));
        let clipboard_seq = Arc::new(AtomicU32::new(clipboard::sequence()));

        let thread_geometry = geometry.clone();
        let thread_seq = clipboard_seq.clone();
        std::thread::Builder::new()
            .name("capture".into())
            .spawn(move || {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                    let _ = MFStartup(MF_VERSION, MFSTARTUP_FULL);
                }
                if let Err(err) = run(rx, out, thread_geometry, copy, thread_seq) {
                    log::warn!("capture stopped: {err:#}");
                }
                unsafe {
                    let _ = MFShutdown();
                    CoUninitialize();
                }
            })?;

        Ok(Capture {
            commands,
            geometry,
            clipboard_seq,
        })
    }

    pub fn command(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn mouse(&self, m: Mouse) {
        let geometry = *self.geometry.lock().unwrap();
        input::mouse(m, geometry);
    }

    pub fn key(&self, k: Key) {
        input::key(k);
    }

    pub fn set_clipboard(&self, text: &str) {
        match clipboard::set_text(text) {
            Ok(()) => self
                .clipboard_seq
                .store(clipboard::sequence(), Ordering::SeqCst),
            Err(err) => log::warn!("failed to set the clipboard: {err:#}"),
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
    }
}

fn bitrate(width: u32, height: u32) -> u32 {
    ((width as u64 * height as u64 * FPS as u64 / 25) as u32).clamp(1_000_000, 8_000_000)
}

struct Sink {
    out: mpsc::Sender<Vec<u8>>,
}

impl Sink {
    fn send(&self, message: Vec<u8>) -> bool {
        self.out.blocking_send(message).is_ok()
    }
}

fn run(
    commands: Receiver<Command>,
    out: mpsc::Sender<Vec<u8>>,
    geometry: Arc<Mutex<Geometry>>,
    copy: bool,
    clipboard_seq: Arc<AtomicU32>,
) -> Result<()> {
    let sink = Sink { out };
    let start = Instant::now();
    let frame_interval = Duration::from_millis(1000 / FPS as u64);
    let mut current: Option<u32> = None;
    let mut secure_notified = false;
    let mut last_cursor = (i32::MIN, i32::MIN, false);
    let mut last_displays = Vec::new();

    'outer: loop {
        let outputs = dxgi::outputs()?;
        if outputs.is_empty() {
            anyhow::bail!("no display");
        }
        let index = current
            .filter(|&i| (i as usize) < outputs.len())
            .unwrap_or_else(|| outputs.iter().position(|o| o.display.primary).unwrap_or(0) as u32);
        current = Some(index);
        let output = &outputs[index as usize];
        let displays = Displays {
            displays: outputs.iter().map(|o| o.display.clone()).collect(),
            current: index,
        };
        let message = protocol::encode_json(protocol::DISPLAYS, &displays);
        if message != last_displays {
            if !sink.send(message.clone()) {
                break;
            }
            last_displays = message;
        }

        let mut duplicator = match dxgi::Duplicator::new(output) {
            Ok(d) => d,
            Err(err) => {
                if !secure_notified {
                    log::info!("can't capture the screen now: {err:#}");
                    secure_notified = true;
                    if !sink.send(protocol::notice(
                        "secure-desktop",
                        "The remote computer is showing a secure screen such as the lock screen or a UAC prompt.",
                    )) {
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(500));
                loop {
                    match commands.try_recv() {
                        Ok(Command::Stop) | Err(TryRecvError::Disconnected) => break 'outer,
                        Ok(Command::Display(i)) => current = Some(i),
                        Ok(Command::KeyFrame) | Err(TryRecvError::Empty) => break,
                    }
                }
                continue;
            }
        };
        if secure_notified {
            secure_notified = false;
            if !sink.send(protocol::notice("desktop", "")) {
                break;
            }
        }

        let scale = if duplicator.width > 2560 { 2 } else { 1 };
        let width = (duplicator.width / scale) & !1;
        let height = (duplicator.height / scale) & !1;
        *geometry.lock().unwrap() = Geometry {
            left: output.display.left,
            top: output.display.top,
            scale: scale as i32,
        };

        let (src_width, src_height) = (duplicator.width, duplicator.height);
        let mut encoder = encoder::Encoder::new(
            width as u32,
            height as u32,
            FPS,
            bitrate(width as u32, height as u32),
        )?;
        let mut nv12 = Vec::new();
        let mut have_frame = false;
        let mut want_key = true;
        let mut configured = false;
        let mut next = Instant::now();

        loop {
            loop {
                match commands.try_recv() {
                    Ok(Command::KeyFrame) => want_key = true,
                    Ok(Command::Display(i)) => {
                        current = Some(i);
                        continue 'outer;
                    }
                    Ok(Command::Stop) | Err(TryRecvError::Disconnected) => break 'outer,
                    Err(TryRecvError::Empty) => break,
                }
            }

            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            }
            next = Instant::now() + frame_interval;

            let cursor = cursor_position(output.display.left, output.display.top, scale as i32);
            if cursor != last_cursor {
                last_cursor = cursor;
                if !sink.send(protocol::encode_cursor(cursor.0, cursor.1, cursor.2)) {
                    break 'outer;
                }
            }

            if copy {
                let seq = clipboard::sequence();
                if clipboard_seq.swap(seq, Ordering::SeqCst) != seq {
                    if let Ok(Some(text)) = clipboard::get_text() {
                        if !sink.send(protocol::encode(protocol::CLIPBOARD, text.as_bytes())) {
                            break 'outer;
                        }
                    }
                }
            }

            let t0 = Instant::now();
            let frame = duplicator.acquire(0, |data, pitch| {
                convert::bgra_to_nv12(
                    data, pitch, src_width, src_height, scale, width, height, &mut nv12,
                );
            });
            let updated = match frame {
                Ok(dxgi::Frame::Updated) => {
                    have_frame = true;
                    true
                }
                Ok(dxgi::Frame::Unchanged) => false,
                Ok(dxgi::Frame::AccessLost) => continue 'outer,
                Err(err) => {
                    log::info!("failed to capture the screen: {err:#}");
                    std::thread::sleep(Duration::from_millis(500));
                    continue 'outer;
                }
            };
            if !have_frame || !(updated || want_key) {
                continue;
            }

            let t1 = Instant::now();
            let timestamp = start.elapsed().as_micros() as u64;
            let packets = encoder.encode(&nv12, (timestamp * 10) as i64, want_key)?;
            log::debug!(
                "capture {:?}, encode {:?}, {} packets, key requested {want_key}",
                t1 - t0,
                t1.elapsed(),
                packets.len()
            );
            for packet in packets {
                if packet.key {
                    want_key = false;
                    if let Some(codec) = h264::codec_string(&packet.data) {
                        let config = VideoConfig {
                            codec,
                            width: width as u32,
                            height: height as u32,
                        };
                        if !sink.send(protocol::encode_json(protocol::VIDEO_CONFIG, &config)) {
                            break 'outer;
                        }
                        configured = true;
                    }
                }
                if !configured {
                    continue;
                }
                if !sink.send(protocol::encode_video(packet.key, timestamp, &packet.data)) {
                    break 'outer;
                }
            }
        }
    }
    Ok(())
}

fn cursor_position(left: i32, top: i32, scale: i32) -> (i32, i32, bool) {
    let mut info = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetCursorInfo(&mut info) }.is_err() {
        return (0, 0, false);
    }
    let POINT { x, y } = info.ptScreenPos;
    (
        (x - left) / scale,
        (y - top) / scale,
        info.flags.0 & CURSOR_SHOWING.0 != 0,
    )
}
