//! Phím tắt dùng để dừng điều khiển.

use std::fmt;

use eframe::egui;
use serde::{Deserialize, Serialize};

const VK_0: u16 = 0x30;
const VK_9: u16 = 0x39;
const VK_A: u16 = 0x41;
const VK_Z: u16 = 0x5A;
const VK_F1: u16 = 0x70;
const VK_F24: u16 = 0x87;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Virtual-key code của phím chính.
    pub vk: u16,
}

impl Default for Hotkey {
    /// Ctrl+Alt+F12.
    fn default() -> Self {
        Self { ctrl: true, alt: true, shift: false, vk: VK_F1 + 11 }
    }
}

impl Hotkey {
    /// Phải có ít nhất một phím bổ trợ để không trùng phím gõ thường, và phím
    /// chính phải là chữ, số hoặc F1–F24.
    pub fn is_valid(&self) -> bool {
        (self.ctrl || self.alt || self.shift) && vk_name(self.vk).is_some()
    }

    /// Tạo từ một lần nhấn phím trong egui. `None` nếu tổ hợp không dùng được.
    pub fn from_egui(key: egui::Key, modifiers: egui::Modifiers) -> Option<Self> {
        let hotkey = Self {
            ctrl: modifiers.ctrl,
            alt: modifiers.alt,
            shift: modifiers.shift,
            vk: egui_key_to_vk(key)?,
        };
        hotkey.is_valid().then_some(hotkey)
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (held, name) in [(self.ctrl, "Ctrl"), (self.alt, "Alt"), (self.shift, "Shift")] {
            if held {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&vk_name(self.vk).unwrap_or_else(|| format!("0x{:02X}", self.vk)))
    }
}

fn vk_name(vk: u16) -> Option<String> {
    match vk {
        // VK của chữ và số trùng mã ASCII.
        VK_0..=VK_9 | VK_A..=VK_Z => Some(char::from(vk as u8).to_string()),
        VK_F1..=VK_F24 => Some(format!("F{}", vk - VK_F1 + 1)),
        _ => None,
    }
}

fn egui_key_to_vk(key: egui::Key) -> Option<u16> {
    let name = key.name();
    if let [c] = name.as_bytes() {
        return c.is_ascii_alphanumeric().then(|| u16::from(c.to_ascii_uppercase()));
    }
    let n: u16 = name.strip_prefix('F')?.parse().ok()?;
    (1..=24).contains(&n).then(|| VK_F1 + n - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_ctrl_alt_f12() {
        let hotkey = Hotkey::default();
        assert!(hotkey.is_valid());
        assert_eq!(hotkey.to_string(), "Ctrl+Alt+F12");
    }

    #[test]
    fn maps_egui_keys() {
        let mods = egui::Modifiers { ctrl: true, shift: true, ..Default::default() };
        let hotkey = Hotkey::from_egui(egui::Key::Q, mods).unwrap();
        assert_eq!(hotkey.vk, 0x51);
        assert_eq!(hotkey.to_string(), "Ctrl+Shift+Q");
        assert_eq!(Hotkey::from_egui(egui::Key::Num7, mods).unwrap().vk, 0x37);
        assert_eq!(Hotkey::from_egui(egui::Key::F5, mods).unwrap().vk, 0x74);
    }

    #[test]
    fn requires_modifier_and_supported_key() {
        assert!(Hotkey::from_egui(egui::Key::Q, egui::Modifiers::NONE).is_none());
        assert!(Hotkey::from_egui(egui::Key::Enter, egui::Modifiers::CTRL).is_none());
        assert!(!Hotkey { ctrl: true, alt: false, shift: false, vk: 0x0D }.is_valid());
    }
}
