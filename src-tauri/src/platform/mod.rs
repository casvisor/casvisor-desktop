#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use self::windows::{init, protect, unprotect, Capture, Command};

#[cfg(not(windows))]
mod unsupported {
    use anyhow::{bail, Result};
    use tokio::sync::mpsc;

    use crate::protocol::{Key, Mouse};

    pub fn init() {}

    pub fn protect(data: &[u8]) -> Result<Vec<u8>> {
        Ok(data.to_vec())
    }

    pub fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
        Ok(data.to_vec())
    }

    pub enum Command {
        KeyFrame,
        Display(u32),
        Stop,
    }

    pub struct Capture;

    impl Capture {
        pub fn start(_out: mpsc::Sender<Vec<u8>>, _copy: bool) -> Result<Capture> {
            bail!("remote control of this system is not supported yet")
        }

        pub fn command(&self, _command: Command) {}

        pub fn mouse(&self, _m: Mouse) {}

        pub fn key(&self, _k: Key) {}

        pub fn set_clipboard(&self, _text: &str) {}
    }
}

#[cfg(not(windows))]
pub use unsupported::{init, protect, unprotect, Capture, Command};
