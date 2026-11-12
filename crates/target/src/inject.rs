//! Bơm input vào Windows bằng `SendInput` và nhớ những gì đang được giữ để
//! nhả lại khi phiên kết thúc.

use std::mem::size_of;

use smartmaupad_shared::protocol::{Message, MouseButton, INJECT_TAG};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN,
    MOUSEEVENTF_XUP, MOUSEINPUT,
};

const XBUTTON1: i32 = 1;
const XBUTTON2: i32 = 2;

const VK_PAUSE: u16 = 0x13;
const VK_SNAPSHOT: u16 = 0x2C;
const VK_NUMLOCK: u16 = 0x90;
const VK_RSHIFT: u16 = 0xA1;

#[derive(Clone, Copy, PartialEq, Eq)]
struct KeyId {
    scan: u16,
    vk: u16,
    extended: bool,
}

#[derive(Default)]
pub struct Injector {
    held_keys: Vec<KeyId>,
    held_buttons: Vec<MouseButton>,
}

impl Injector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, msg: &Message) {
        match *msg {
            Message::MouseMove { dx, dy } => send_mouse(dx, dy, 0, MOUSEEVENTF_MOVE),
            Message::MouseButton { button, down } => {
                self.held_buttons.retain(|b| *b != button);
                if down {
                    self.held_buttons.push(button);
                }
                send_button(button, down);
            }
            Message::Wheel { delta, horizontal } => {
                let flags = if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL };
                send_mouse(0, 0, i32::from(delta), flags);
            }
            Message::Key { scan, vk, extended, down } => {
                let key = KeyId { scan, vk, extended };
                self.held_keys.retain(|k| *k != key);
                if down {
                    self.held_keys.push(key);
                }
                send_key(key, down);
            }
            Message::ReleaseAll => self.release_all(),
            Message::Ping(_) | Message::Pong(_) => {}
        }
    }

    /// Nhả mọi phím và nút đang giữ. Gọi khi phiên kết thúc vì bất kỳ lý do gì
    /// để target không bị kẹt Ctrl/Alt hay đang kéo dở.
    pub fn release_all(&mut self) {
        for key in self.held_keys.drain(..).rev() {
            send_key(key, false);
        }
        for button in self.held_buttons.drain(..) {
            send_button(button, false);
        }
    }
}

fn send(input: &INPUT) {
    // SAFETY: `input` là một INPUT hợp lệ, kích thước truyền đúng với struct.
    unsafe {
        SendInput(1, input, size_of::<INPUT>() as i32);
    }
}

fn send_mouse(dx: i32, dy: i32, data: i32, flags: u32) {
    send(&INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data as _,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECT_TAG,
            },
        },
    });
}

fn send_button(button: MouseButton, down: bool) {
    let (flags, data) = match (button, down) {
        (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (MouseButton::X1, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
        (MouseButton::X1, false) => (MOUSEEVENTF_XUP, XBUTTON1),
        (MouseButton::X2, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
        (MouseButton::X2, false) => (MOUSEEVENTF_XUP, XBUTTON2),
    };
    send_mouse(0, 0, data, flags);
}

fn send_key(key: KeyId, down: bool) {
    // Bơm theo scancode để không phụ thuộc bố cục bàn phím và để game nhận được.
    // Vài phím có scancode mập mờ khi đi qua low-level hook (Pause/NumLock dùng
    // chung 0x45, PrintScreen) và phím media không có scancode: bơm theo virtual key.
    let by_vk = key.scan == 0 || matches!(key.vk, VK_PAUSE | VK_SNAPSHOT | VK_NUMLOCK);
    // Hook báo Shift phải là phím extended dù scancode 0x36 không có tiền tố E0.
    let extended = key.extended && key.vk != VK_RSHIFT;

    let mut flags = 0;
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    let (vk, scan) = if by_vk {
        (key.vk, 0)
    } else {
        flags |= KEYEVENTF_SCANCODE;
        (0, key.scan)
    };

    send(&INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: INJECT_TAG },
        },
    });
}
