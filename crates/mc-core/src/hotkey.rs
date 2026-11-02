//! Hotkey description and parsing.

use crate::key::{mods, Key};
use std::fmt;

/// A global hotkey: a set of required modifiers plus one non-modifier key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hotkey {
    /// Required modifier bits (see [`mods`]).
    pub mods: u8,
    /// The trigger key.
    pub key: Key,
}

impl Hotkey {
    /// Parse a hotkey string such as `ctrl+alt+space`.
    pub fn parse(spec: &str) -> Result<Hotkey, HotkeyError> {
        let mut modifier_bits = 0u8;
        let mut key: Option<Key> = None;
        for part in spec.split(['+', '-']) {
            let part = part.trim().to_ascii_lowercase();
            if part.is_empty() {
                continue;
            }
            match part.as_str() {
                "ctrl" | "control" => modifier_bits |= mods::CTRL,
                "alt" | "option" => modifier_bits |= mods::ALT,
                "shift" => modifier_bits |= mods::SHIFT,
                "meta" | "cmd" | "command" | "super" | "win" => modifier_bits |= mods::META,
                other => {
                    if key.is_some() {
                        return Err(HotkeyError::MultipleKeys);
                    }
                    key = Some(
                        key_from_name(other)
                            .ok_or_else(|| HotkeyError::UnknownKey(other.to_string()))?,
                    );
                }
            }
        }
        let key = key.ok_or(HotkeyError::NoKey)?;
        if key.is_modifier() {
            return Err(HotkeyError::ModifierAsTrigger);
        }
        Ok(Hotkey {
            mods: modifier_bits,
            key,
        })
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods & mods::CTRL != 0 {
            write!(f, "ctrl+")?;
        }
        if self.mods & mods::ALT != 0 {
            write!(f, "alt+")?;
        }
        if self.mods & mods::SHIFT != 0 {
            write!(f, "shift+")?;
        }
        if self.mods & mods::META != 0 {
            write!(f, "meta+")?;
        }
        write!(f, "{}", key_name(self.key))
    }
}

/// Errors from parsing a hotkey specification.
#[derive(Debug, thiserror::Error)]
pub enum HotkeyError {
    /// The key name was not recognised.
    #[error("unknown key name: {0}")]
    UnknownKey(String),
    /// More than one non-modifier key was given.
    #[error("a hotkey can only have one trigger key")]
    MultipleKeys,
    /// No trigger key was given.
    #[error("no trigger key in hotkey")]
    NoKey,
    /// The trigger key was itself a modifier.
    #[error("a modifier cannot be the trigger key")]
    ModifierAsTrigger,
}

/// Map a key name to a canonical key.
pub fn key_from_name(name: &str) -> Option<Key> {
    let name = name.to_ascii_lowercase();
    Some(match name.as_str() {
        "a" => Key::A,
        "b" => Key::B,
        "c" => Key::C,
        "d" => Key::D,
        "e" => Key::E,
        "f" => Key::F,
        "g" => Key::G,
        "h" => Key::H,
        "i" => Key::I,
        "j" => Key::J,
        "k" => Key::K,
        "l" => Key::L,
        "m" => Key::M,
        "n" => Key::N,
        "o" => Key::O,
        "p" => Key::P,
        "q" => Key::Q,
        "r" => Key::R,
        "s" => Key::S,
        "t" => Key::T,
        "u" => Key::U,
        "v" => Key::V,
        "w" => Key::W,
        "x" => Key::X,
        "y" => Key::Y,
        "z" => Key::Z,
        "0" => Key::Num0,
        "1" => Key::Num1,
        "2" => Key::Num2,
        "3" => Key::Num3,
        "4" => Key::Num4,
        "5" => Key::Num5,
        "6" => Key::Num6,
        "7" => Key::Num7,
        "8" => Key::Num8,
        "9" => Key::Num9,
        "space" | "spacebar" => Key::Space,
        "enter" | "return" => Key::Enter,
        "esc" | "escape" => Key::Escape,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "caps" | "capslock" => Key::CapsLock,
        "del" | "delete" => Key::Delete,
        "ins" | "insert" => Key::Insert,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "pgup" => Key::PageUp,
        "pagedown" | "pgdn" => Key::PageDown,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "grave" | "backtick" => Key::Backquote,
        "minus" | "dash" => Key::Minus,
        "equal" | "equals" => Key::Equal,
        "backslash" => Key::Backslash,
        "semicolon" => Key::Semicolon,
        "quote" | "apostrophe" => Key::Quote,
        "comma" => Key::Comma,
        "period" | "dot" => Key::Period,
        "slash" => Key::Slash,
        "lbracket" | "bracketleft" => Key::BracketLeft,
        "rbracket" | "bracketright" => Key::BracketRight,
        "prtsc" | "print" | "printscreen" | "snapshot" => Key::PrintScreen,
        "scrolllock" => Key::ScrollLock,
        "pause" | "break" => Key::Pause,
        "menu" | "apps" => Key::Menu,
        "numlock" => Key::NumLock,
        "numadd" | "kpadd" => Key::NumpadAdd,
        "numsub" | "kpsub" => Key::NumpadSubtract,
        "nummul" | "kpmul" => Key::NumpadMultiply,
        "numdiv" | "kpdiv" => Key::NumpadDivide,
        "numenter" | "kpenter" => Key::NumpadEnter,
        "numdec" | "kpdec" => Key::NumpadDecimal,
        "intlbackslash" | "102" => Key::IntlBackslash,
        other => {
            if let Some(num) = other.strip_prefix('f') {
                if let Ok(n) = num.parse::<u8>() {
                    if (1..=24).contains(&n) {
                        return Some(match n {
                            1 => Key::F1,
                            2 => Key::F2,
                            3 => Key::F3,
                            4 => Key::F4,
                            5 => Key::F5,
                            6 => Key::F6,
                            7 => Key::F7,
                            8 => Key::F8,
                            9 => Key::F9,
                            10 => Key::F10,
                            11 => Key::F11,
                            12 => Key::F12,
                            13 => Key::F13,
                            14 => Key::F14,
                            15 => Key::F15,
                            16 => Key::F16,
                            17 => Key::F17,
                            18 => Key::F18,
                            19 => Key::F19,
                            20 => Key::F20,
                            21 => Key::F21,
                            22 => Key::F22,
                            23 => Key::F23,
                            24 => Key::F24,
                            _ => unreachable!(),
                        });
                    }
                }
            }
            return None;
        }
    })
}

/// Canonical name for a key, used when displaying the hotkey.
pub fn key_name(key: Key) -> String {
    match key {
        Key::A => "a".into(),
        Key::B => "b".into(),
        Key::C => "c".into(),
        Key::D => "d".into(),
        Key::E => "e".into(),
        Key::F => "f".into(),
        Key::G => "g".into(),
        Key::H => "h".into(),
        Key::I => "i".into(),
        Key::J => "j".into(),
        Key::K => "k".into(),
        Key::L => "l".into(),
        Key::M => "m".into(),
        Key::N => "n".into(),
        Key::O => "o".into(),
        Key::P => "p".into(),
        Key::Q => "q".into(),
        Key::R => "r".into(),
        Key::S => "s".into(),
        Key::T => "t".into(),
        Key::U => "u".into(),
        Key::V => "v".into(),
        Key::W => "w".into(),
        Key::X => "x".into(),
        Key::Y => "y".into(),
        Key::Z => "z".into(),
        other => format!("{other:?}").to_ascii_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_hotkeys() {
        let hk = Hotkey::parse("ctrl+alt+space").unwrap();
        assert_eq!(hk.key, Key::Space);
        assert_eq!(hk.mods, mods::CTRL | mods::ALT);

        let hk = Hotkey::parse("Ctrl + Alt + F12").unwrap();
        assert_eq!(hk.key, Key::F12);
        assert_eq!(hk.mods, mods::CTRL | mods::ALT);

        let hk = Hotkey::parse("cmd+shift+k").unwrap();
        assert_eq!(hk.key, Key::K);
        assert_eq!(hk.mods, mods::META | mods::SHIFT);
    }

    #[test]
    fn rejects_bad_hotkeys() {
        assert!(Hotkey::parse("ctrl+alt").is_err());
        assert!(Hotkey::parse("ctrl+alt+space+enter").is_err());
        assert!(Hotkey::parse("ctrl+notakey").is_err());
        assert!(Hotkey::parse("ctrl+shift").is_err());
    }

    #[test]
    fn display_roundtrip() {
        for spec in ["ctrl+alt+space", "ctrl+shift+f9", "meta+q"] {
            let hk = Hotkey::parse(spec).unwrap();
            let rendered = hk.to_string();
            let again = Hotkey::parse(&rendered).unwrap();
            assert_eq!(hk, again, "spec {spec} rendered {rendered}");
        }
    }
}
