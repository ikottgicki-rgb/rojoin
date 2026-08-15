//! Platform-independent key names.
//!
//! Deliberately a small, closed set rather than every key a keyboard has: this
//! covers what Roblox movement and gameplay actually use, and a closed enum
//! means the platform mappings below are exhaustively checked by the compiler.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    W, A, S, D,
    Q, E, R, F, C, V, X, Z, G, H, T, B, N, M,
    Space,
    Shift,
    Ctrl,
    Alt,
    Tab,
    Escape,
    Enter,
    Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    Up, Down, Left, Right,
}

impl Key {
    /// Every key, for the hotkey picker.
    pub const ALL: &'static [Key] = &[
        Key::W, Key::A, Key::S, Key::D,
        Key::Q, Key::E, Key::R, Key::F, Key::C, Key::V, Key::X, Key::Z,
        Key::G, Key::H, Key::T, Key::B, Key::N, Key::M,
        Key::Space, Key::Shift, Key::Ctrl, Key::Alt, Key::Tab, Key::Escape, Key::Enter,
        Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4,
        Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9,
        Key::F1, Key::F2, Key::F3, Key::F4, Key::F5, Key::F6,
        Key::F7, Key::F8, Key::F9, Key::F10, Key::F11, Key::F12,
        Key::Up, Key::Down, Key::Left, Key::Right,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Key::W => "W", Key::A => "A", Key::S => "S", Key::D => "D",
            Key::Q => "Q", Key::E => "E", Key::R => "R", Key::F => "F",
            Key::C => "C", Key::V => "V", Key::X => "X", Key::Z => "Z",
            Key::G => "G", Key::H => "H", Key::T => "T", Key::B => "B",
            Key::N => "N", Key::M => "M",
            Key::Space => "Space",
            Key::Shift => "Shift",
            Key::Ctrl => "Ctrl",
            Key::Alt => "Alt",
            Key::Tab => "Tab",
            Key::Escape => "Esc",
            Key::Enter => "Enter",
            Key::Num0 => "0", Key::Num1 => "1", Key::Num2 => "2", Key::Num3 => "3",
            Key::Num4 => "4", Key::Num5 => "5", Key::Num6 => "6", Key::Num7 => "7",
            Key::Num8 => "8", Key::Num9 => "9",
            Key::F1 => "F1", Key::F2 => "F2", Key::F3 => "F3", Key::F4 => "F4",
            Key::F5 => "F5", Key::F6 => "F6", Key::F7 => "F7", Key::F8 => "F8",
            Key::F9 => "F9", Key::F10 => "F10", Key::F11 => "F11", Key::F12 => "F12",
            Key::Up => "Up", Key::Down => "Down", Key::Left => "Left", Key::Right => "Right",
        }
    }

    pub fn from_label(label: &str) -> Option<Key> {
        Key::ALL.iter().copied().find(|k| k.label().eq_ignore_ascii_case(label))
    }

    /// Linux evdev key code.
    #[cfg(unix)]
    pub fn evdev_code(self) -> u16 {
        // Values from linux/input-event-codes.h.
        match self {
            Key::A => 30, Key::B => 48, Key::C => 46, Key::D => 32,
            Key::E => 18, Key::F => 33, Key::G => 34, Key::H => 35,
            Key::M => 50, Key::N => 49, Key::Q => 16, Key::R => 19,
            Key::S => 31, Key::T => 20, Key::V => 47, Key::W => 17,
            Key::X => 45, Key::Z => 44,
            Key::Space => 57,
            Key::Shift => 42,   // KEY_LEFTSHIFT
            Key::Ctrl => 29,    // KEY_LEFTCTRL
            Key::Alt => 56,     // KEY_LEFTALT
            Key::Tab => 15,
            Key::Escape => 1,
            Key::Enter => 28,
            Key::Num1 => 2, Key::Num2 => 3, Key::Num3 => 4, Key::Num4 => 5,
            Key::Num5 => 6, Key::Num6 => 7, Key::Num7 => 8, Key::Num8 => 9,
            Key::Num9 => 10, Key::Num0 => 11,
            Key::F1 => 59, Key::F2 => 60, Key::F3 => 61, Key::F4 => 62,
            Key::F5 => 63, Key::F6 => 64, Key::F7 => 65, Key::F8 => 66,
            Key::F9 => 67, Key::F10 => 68, Key::F11 => 87, Key::F12 => 88,
            Key::Up => 103, Key::Down => 108, Key::Left => 105, Key::Right => 106,
        }
    }

    /// Windows virtual-key code.
    #[cfg(windows)]
    pub fn vk_code(self) -> u16 {
        match self {
            Key::A => 0x41, Key::B => 0x42, Key::C => 0x43, Key::D => 0x44,
            Key::E => 0x45, Key::F => 0x46, Key::G => 0x47, Key::H => 0x48,
            Key::M => 0x4D, Key::N => 0x4E, Key::Q => 0x51, Key::R => 0x52,
            Key::S => 0x53, Key::T => 0x54, Key::V => 0x56, Key::W => 0x57,
            Key::X => 0x58, Key::Z => 0x5A,
            Key::Space => 0x20,
            Key::Shift => 0x10,
            Key::Ctrl => 0x11,
            Key::Alt => 0x12,
            Key::Tab => 0x09,
            Key::Escape => 0x1B,
            Key::Enter => 0x0D,
            Key::Num0 => 0x30, Key::Num1 => 0x31, Key::Num2 => 0x32, Key::Num3 => 0x33,
            Key::Num4 => 0x34, Key::Num5 => 0x35, Key::Num6 => 0x36, Key::Num7 => 0x37,
            Key::Num8 => 0x38, Key::Num9 => 0x39,
            Key::F1 => 0x70, Key::F2 => 0x71, Key::F3 => 0x72, Key::F4 => 0x73,
            Key::F5 => 0x74, Key::F6 => 0x75, Key::F7 => 0x76, Key::F8 => 0x77,
            Key::F9 => 0x78, Key::F10 => 0x79, Key::F11 => 0x7A, Key::F12 => 0x7B,
            Key::Up => 0x26, Key::Down => 0x28, Key::Left => 0x25, Key::Right => 0x27,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn labels_round_trip() {
        for k in Key::ALL {
            assert_eq!(Key::from_label(k.label()), Some(*k), "{k:?} did not round-trip");
        }
    }

    #[test]
    fn labels_are_unique() {
        let mut seen = HashSet::new();
        for k in Key::ALL {
            assert!(seen.insert(k.label()), "duplicate label {}", k.label());
        }
    }

    #[test]
    fn label_lookup_is_case_insensitive() {
        assert_eq!(Key::from_label("space"), Some(Key::Space));
        assert_eq!(Key::from_label("SPACE"), Some(Key::Space));
        assert_eq!(Key::from_label("nonsense"), None);
    }

    #[cfg(unix)]
    #[test]
    fn evdev_codes_are_unique_and_nonzero_where_expected() {
        // A duplicate code would silently make two keys the same key.
        let mut seen = HashSet::new();
        for k in Key::ALL {
            let code = k.evdev_code();
            assert!(seen.insert(code), "duplicate evdev code {code} for {k:?}");
        }
        assert_eq!(Key::W.evdev_code(), 17);
        assert_eq!(Key::Space.evdev_code(), 57);
    }

    #[test]
    fn keys_serialise_as_stable_snake_case() {
        // Config files hold these; the spelling must not drift.
        assert_eq!(serde_json::to_string(&Key::Space).unwrap(), "\"space\"");
        assert_eq!(serde_json::to_string(&Key::F1).unwrap(), "\"f1\"");
        let k: Key = serde_json::from_str("\"w\"").unwrap();
        assert_eq!(k, Key::W);
    }
}
