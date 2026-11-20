use std::cell::RefCell;

use anyhow::Result;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, GetThreadDesktop, GetUserObjectInformationW, OpenInputDesktop, SetThreadDesktop,
    DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, DESKTOP_CREATEWINDOW, DESKTOP_HOOKCONTROL,
    DESKTOP_JOURNALPLAYBACK, DESKTOP_READOBJECTS, DESKTOP_SWITCHDESKTOP, DESKTOP_WRITEOBJECTS,
    HDESK, UOI_NAME,
};
use windows::Win32::System::Threading::GetCurrentThreadId;

thread_local! {
    static ATTACHED: RefCell<Option<HDESK>> = const { RefCell::new(None) };
}

fn name_of(desktop: HDESK) -> String {
    let mut buf = [0u16; 256];
    let mut needed = 0u32;
    unsafe {
        if GetUserObjectInformationW(
            HANDLE(desktop.0),
            UOI_NAME,
            Some(buf.as_mut_ptr() as *mut _),
            (buf.len() * 2) as u32,
            Some(&mut needed),
        )
        .is_err()
        {
            return String::new();
        }
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// The desktop that gets the keyboard and mouse now: "Default", or
/// "Winlogon" for the lock screen, the sign-in screen and UAC prompts.
pub fn input_desktop_name() -> Option<String> {
    unsafe {
        let desktop =
            OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS).ok()?;
        let name = name_of(desktop);
        let _ = CloseDesktop(desktop);
        Some(name)
    }
}

pub fn thread_desktop_name() -> String {
    unsafe {
        match GetThreadDesktop(GetCurrentThreadId()) {
            Ok(desktop) => name_of(desktop),
            Err(_) => String::new(),
        }
    }
}

/// Moves the calling thread to the input desktop, so that it captures and
/// injects input where the user is. Only SYSTEM may join "Winlogon".
pub fn attach_input_desktop() -> Result<String> {
    let access = DESKTOP_ACCESS_FLAGS(
        DESKTOP_READOBJECTS.0
            | DESKTOP_CREATEWINDOW.0
            | DESKTOP_WRITEOBJECTS.0
            | DESKTOP_HOOKCONTROL.0
            | DESKTOP_JOURNALPLAYBACK.0
            | DESKTOP_SWITCHDESKTOP.0,
    );
    unsafe {
        let desktop = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, access)?;
        let name = name_of(desktop);
        if let Err(err) = SetThreadDesktop(desktop) {
            let _ = CloseDesktop(desktop);
            return Err(err.into());
        }
        ATTACHED.with(|attached| {
            if let Some(old) = attached.borrow_mut().replace(desktop) {
                let _ = CloseDesktop(old);
            }
        });
        Ok(name)
    }
}

/// Follows the input desktop when it changed since the thread last attached.
pub fn follow_input_desktop() {
    if let Some(input) = input_desktop_name() {
        if input != thread_desktop_name() {
            let _ = attach_input_desktop();
        }
    }
}
