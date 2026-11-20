use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::protocol::{Key, Mouse};

/// Where the video is on the desktop: the top left corner of the display
/// and how many desktop pixels a video pixel is.
#[derive(Debug, Clone, Copy, Default)]
pub struct Geometry {
    pub left: i32,
    pub top: i32,
    pub scale: i32,
}

const XBUTTON1: u32 = 1;
const XBUTTON2: u32 = 2;

fn send(input: INPUT) {
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

fn mouse_input(dx: i32, dy: i32, data: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

pub fn mouse(m: Mouse, geometry: Geometry) {
    let scale = geometry.scale.max(1);
    let x = geometry.left + m.x as i32 * scale;
    let y = geometry.top + m.y as i32 * scale;

    let (vx, vy, vw, vh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2),
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2),
        )
    };
    let nx = ((x - vx) as i64 * 65535 / (vw - 1) as i64) as i32;
    let ny = ((y - vy) as i64 * 65535 / (vh - 1) as i64) as i32;
    let at = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;

    match m.action {
        0 => send(mouse_input(nx, ny, 0, at)),
        1 | 2 => {
            let down = m.action == 1;
            let (flags, data) = match (m.button, down) {
                (0, true) => (MOUSEEVENTF_LEFTDOWN, 0),
                (0, false) => (MOUSEEVENTF_LEFTUP, 0),
                (1, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
                (1, false) => (MOUSEEVENTF_MIDDLEUP, 0),
                (2, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
                (2, false) => (MOUSEEVENTF_RIGHTUP, 0),
                (3, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
                (3, false) => (MOUSEEVENTF_XUP, XBUTTON1),
                (4, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
                (4, false) => (MOUSEEVENTF_XUP, XBUTTON2),
                _ => return,
            };
            send(mouse_input(nx, ny, data as i32, at | flags));
        }
        3 => {
            if m.dy != 0 {
                send(mouse_input(0, 0, m.dy as i32, MOUSEEVENTF_WHEEL));
            }
            if m.dx != 0 {
                send(mouse_input(0, 0, m.dx as i32, MOUSEEVENTF_HWHEEL));
            }
        }
        _ => {}
    }
}

pub fn key(k: Key) {
    let mut flags = KEYEVENTF_SCANCODE;
    if k.scancode & 0xff00 == 0xe000 {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !k.down {
        flags |= KEYEVENTF_KEYUP;
    }
    send(INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: k.scancode & 0xff,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    });
}
