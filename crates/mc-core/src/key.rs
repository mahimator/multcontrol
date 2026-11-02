//! Canonical, layout-independent key identifiers plus the platform mapping tables.
//!
//! Both capture and injection go through [`Key`], which identifies a *physical key
//! position* on a US-layout keyboard rather than a character. That keeps the
//! target's own keyboard layout in charge of what a key actually types, which is
//! what users expect from a KVM-style tool.
//!
//! * Windows capture gives us a Set-1 scan code (+ extended flag); we can only
//!   reconstruct the physical position from that.
//! * macOS capture gives us a hardware virtual key code, which *is* a physical
//!   position.
//!
//! A few keys have no macOS equivalent (PrintScreen, ScrollLock, Pause, Menu,
//! IntlBackslash). Those get synthetic key codes above `0x100`, which the macOS
//! backend translates to the nearest real key at injection time.

/// Modifier bitmask shared by the wire protocol and the platform backends.
pub mod mods {
    pub const SHIFT: u8 = 1 << 0;
    pub const CTRL: u8 = 1 << 1;
    pub const ALT: u8 = 1 << 2;
    pub const META: u8 = 1 << 3;
    pub const CAPS: u8 = 1 << 4;
}

/// A physical key position.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
#[repr(u16)]
pub enum Key {
    A = 1,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    Num0,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,
    Backquote,
    Minus,
    Equal,
    Backslash,
    Semicolon,
    Quote,
    Comma,
    Period,
    Slash,
    BracketLeft,
    BracketRight,
    Backspace,
    Tab,
    Enter,
    Space,
    CapsLock,
    Escape,
    ScrollLock,
    Pause,
    PrintScreen,
    Insert,
    Home,
    PageUp,
    Delete,
    End,
    PageDown,
    Right,
    Left,
    Down,
    Up,
    NumLock,
    NumpadDivide,
    NumpadMultiply,
    NumpadSubtract,
    NumpadAdd,
    NumpadEnter,
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    NumpadDecimal,
    IntlBackslash,
    LShift,
    RShift,
    LCtrl,
    RCtrl,
    LAlt,
    RAlt,
    LMeta,
    RMeta,
    Menu,
}

impl Key {
    /// Every key, in wire-id order. Used for exhaustive table tests.
    pub const ALL: &'static [Key] = &[
        Key::A,
        Key::B,
        Key::C,
        Key::D,
        Key::E,
        Key::F,
        Key::G,
        Key::H,
        Key::I,
        Key::J,
        Key::K,
        Key::L,
        Key::M,
        Key::N,
        Key::O,
        Key::P,
        Key::Q,
        Key::R,
        Key::S,
        Key::T,
        Key::U,
        Key::V,
        Key::W,
        Key::X,
        Key::Y,
        Key::Z,
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
        Key::Num8,
        Key::Num9,
        Key::Num0,
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
        Key::F13,
        Key::F14,
        Key::F15,
        Key::F16,
        Key::F17,
        Key::F18,
        Key::F19,
        Key::F20,
        Key::F21,
        Key::F22,
        Key::F23,
        Key::F24,
        Key::Backquote,
        Key::Minus,
        Key::Equal,
        Key::Backslash,
        Key::Semicolon,
        Key::Quote,
        Key::Comma,
        Key::Period,
        Key::Slash,
        Key::BracketLeft,
        Key::BracketRight,
        Key::Backspace,
        Key::Tab,
        Key::Enter,
        Key::Space,
        Key::CapsLock,
        Key::Escape,
        Key::ScrollLock,
        Key::Pause,
        Key::PrintScreen,
        Key::Insert,
        Key::Home,
        Key::PageUp,
        Key::Delete,
        Key::End,
        Key::PageDown,
        Key::Right,
        Key::Left,
        Key::Down,
        Key::Up,
        Key::NumLock,
        Key::NumpadDivide,
        Key::NumpadMultiply,
        Key::NumpadSubtract,
        Key::NumpadAdd,
        Key::NumpadEnter,
        Key::Numpad0,
        Key::Numpad1,
        Key::Numpad2,
        Key::Numpad3,
        Key::Numpad4,
        Key::Numpad5,
        Key::Numpad6,
        Key::Numpad7,
        Key::Numpad8,
        Key::Numpad9,
        Key::NumpadDecimal,
        Key::IntlBackslash,
        Key::LShift,
        Key::RShift,
        Key::LCtrl,
        Key::RCtrl,
        Key::LAlt,
        Key::RAlt,
        Key::LMeta,
        Key::RMeta,
        Key::Menu,
    ];

    /// Wire id.
    pub fn id(self) -> u16 {
        self as u16
    }

    /// Reconstruct a key from its wire id.
    pub fn from_id(id: u16) -> Option<Key> {
        Key::ALL.iter().copied().find(|k| k.id() == id)
    }

    /// True for keys that are only meaningful as modifiers.
    pub fn is_modifier(self) -> bool {
        matches!(
            self,
            Key::LShift
                | Key::RShift
                | Key::LCtrl
                | Key::RCtrl
                | Key::LAlt
                | Key::RAlt
                | Key::LMeta
                | Key::RMeta
                | Key::CapsLock
        )
    }
}

/// A Windows Set-1 scan code with its extended-prefix flag.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WinScan {
    /// Scan code without the E0 prefix.
    pub code: u8,
    /// True when the key is prefixed with E0 (extended keys).
    pub extended: bool,
}

/// Windows virtual-key code.
pub type WinVk = u16;

/// macOS hardware virtual key code.
pub type MacKeycode = u16;

/// Synthetic macOS key codes for keys the Mac keyboard simply does not have.
pub const MAC_PSEUDO_PRINTSCREEN: MacKeycode = 0x100;
pub const MAC_PSEUDO_SCROLLLOCK: MacKeycode = 0x101;
pub const MAC_PSEUDO_PAUSE: MacKeycode = 0x102;
pub const MAC_PSEUDO_INTL_BACKSLASH: MacKeycode = 0x103;
pub const MAC_PSEUDO_F21: MacKeycode = 0x104;
pub const MAC_PSEUDO_F22: MacKeycode = 0x105;
pub const MAC_PSEUDO_F23: MacKeycode = 0x106;
pub const MAC_PSEUDO_F24: MacKeycode = 0x107;

const WIN_SCAN_TABLE: &[(WinScan, WinVk, Key)] = &[
    (
        WinScan {
            code: 0x01,
            extended: false,
        },
        0x1B,
        Key::Escape,
    ),
    (
        WinScan {
            code: 0x02,
            extended: false,
        },
        0x31,
        Key::Num1,
    ),
    (
        WinScan {
            code: 0x03,
            extended: false,
        },
        0x32,
        Key::Num2,
    ),
    (
        WinScan {
            code: 0x04,
            extended: false,
        },
        0x33,
        Key::Num3,
    ),
    (
        WinScan {
            code: 0x05,
            extended: false,
        },
        0x34,
        Key::Num4,
    ),
    (
        WinScan {
            code: 0x06,
            extended: false,
        },
        0x35,
        Key::Num5,
    ),
    (
        WinScan {
            code: 0x07,
            extended: false,
        },
        0x36,
        Key::Num6,
    ),
    (
        WinScan {
            code: 0x08,
            extended: false,
        },
        0x37,
        Key::Num7,
    ),
    (
        WinScan {
            code: 0x09,
            extended: false,
        },
        0x38,
        Key::Num8,
    ),
    (
        WinScan {
            code: 0x0A,
            extended: false,
        },
        0x39,
        Key::Num9,
    ),
    (
        WinScan {
            code: 0x0B,
            extended: false,
        },
        0x30,
        Key::Num0,
    ),
    (
        WinScan {
            code: 0x0C,
            extended: false,
        },
        0xBD,
        Key::Minus,
    ),
    (
        WinScan {
            code: 0x0D,
            extended: false,
        },
        0xBB,
        Key::Equal,
    ),
    (
        WinScan {
            code: 0x0E,
            extended: false,
        },
        0x08,
        Key::Backspace,
    ),
    (
        WinScan {
            code: 0x0F,
            extended: false,
        },
        0x09,
        Key::Tab,
    ),
    (
        WinScan {
            code: 0x10,
            extended: false,
        },
        0x51,
        Key::Q,
    ),
    (
        WinScan {
            code: 0x11,
            extended: false,
        },
        0x57,
        Key::W,
    ),
    (
        WinScan {
            code: 0x12,
            extended: false,
        },
        0x45,
        Key::E,
    ),
    (
        WinScan {
            code: 0x13,
            extended: false,
        },
        0x52,
        Key::R,
    ),
    (
        WinScan {
            code: 0x14,
            extended: false,
        },
        0x54,
        Key::T,
    ),
    (
        WinScan {
            code: 0x15,
            extended: false,
        },
        0x59,
        Key::Y,
    ),
    (
        WinScan {
            code: 0x16,
            extended: false,
        },
        0x55,
        Key::U,
    ),
    (
        WinScan {
            code: 0x17,
            extended: false,
        },
        0x49,
        Key::I,
    ),
    (
        WinScan {
            code: 0x18,
            extended: false,
        },
        0x4F,
        Key::O,
    ),
    (
        WinScan {
            code: 0x19,
            extended: false,
        },
        0x50,
        Key::P,
    ),
    (
        WinScan {
            code: 0x1A,
            extended: false,
        },
        0xDB,
        Key::BracketLeft,
    ),
    (
        WinScan {
            code: 0x1B,
            extended: false,
        },
        0xDD,
        Key::BracketRight,
    ),
    (
        WinScan {
            code: 0x1C,
            extended: false,
        },
        0x0D,
        Key::Enter,
    ),
    (
        WinScan {
            code: 0x1C,
            extended: true,
        },
        0x0D,
        Key::NumpadEnter,
    ),
    (
        WinScan {
            code: 0x1D,
            extended: false,
        },
        0x11,
        Key::LCtrl,
    ),
    (
        WinScan {
            code: 0x1D,
            extended: true,
        },
        0xA3,
        Key::RCtrl,
    ),
    (
        WinScan {
            code: 0x1E,
            extended: false,
        },
        0x41,
        Key::A,
    ),
    (
        WinScan {
            code: 0x1F,
            extended: false,
        },
        0x53,
        Key::S,
    ),
    (
        WinScan {
            code: 0x20,
            extended: false,
        },
        0x44,
        Key::D,
    ),
    (
        WinScan {
            code: 0x21,
            extended: false,
        },
        0x46,
        Key::F,
    ),
    (
        WinScan {
            code: 0x22,
            extended: false,
        },
        0x47,
        Key::G,
    ),
    (
        WinScan {
            code: 0x23,
            extended: false,
        },
        0x48,
        Key::H,
    ),
    (
        WinScan {
            code: 0x24,
            extended: false,
        },
        0x4A,
        Key::J,
    ),
    (
        WinScan {
            code: 0x25,
            extended: false,
        },
        0x4B,
        Key::K,
    ),
    (
        WinScan {
            code: 0x26,
            extended: false,
        },
        0x4C,
        Key::L,
    ),
    (
        WinScan {
            code: 0x27,
            extended: false,
        },
        0xBA,
        Key::Semicolon,
    ),
    (
        WinScan {
            code: 0x28,
            extended: false,
        },
        0xDE,
        Key::Quote,
    ),
    (
        WinScan {
            code: 0x29,
            extended: false,
        },
        0xC0,
        Key::Backquote,
    ),
    (
        WinScan {
            code: 0x2A,
            extended: false,
        },
        0xA0,
        Key::LShift,
    ),
    (
        WinScan {
            code: 0x2B,
            extended: false,
        },
        0xDC,
        Key::Backslash,
    ),
    (
        WinScan {
            code: 0x2C,
            extended: false,
        },
        0x5A,
        Key::Z,
    ),
    (
        WinScan {
            code: 0x2D,
            extended: false,
        },
        0x58,
        Key::X,
    ),
    (
        WinScan {
            code: 0x2E,
            extended: false,
        },
        0x43,
        Key::C,
    ),
    (
        WinScan {
            code: 0x2F,
            extended: false,
        },
        0x56,
        Key::V,
    ),
    (
        WinScan {
            code: 0x30,
            extended: false,
        },
        0x42,
        Key::B,
    ),
    (
        WinScan {
            code: 0x31,
            extended: false,
        },
        0x4E,
        Key::N,
    ),
    (
        WinScan {
            code: 0x32,
            extended: false,
        },
        0x4D,
        Key::M,
    ),
    (
        WinScan {
            code: 0x33,
            extended: false,
        },
        0xBC,
        Key::Comma,
    ),
    (
        WinScan {
            code: 0x34,
            extended: false,
        },
        0xBE,
        Key::Period,
    ),
    (
        WinScan {
            code: 0x35,
            extended: false,
        },
        0xBF,
        Key::Slash,
    ),
    (
        WinScan {
            code: 0x35,
            extended: true,
        },
        0x6F,
        Key::NumpadDivide,
    ),
    (
        WinScan {
            code: 0x36,
            extended: false,
        },
        0xA1,
        Key::RShift,
    ),
    (
        WinScan {
            code: 0x37,
            extended: true,
        },
        0x2C,
        Key::PrintScreen,
    ),
    (
        WinScan {
            code: 0x37,
            extended: true,
        },
        0x6A,
        Key::NumpadMultiply,
    ),
    (
        WinScan {
            code: 0x38,
            extended: false,
        },
        0xA4,
        Key::LAlt,
    ),
    (
        WinScan {
            code: 0x38,
            extended: true,
        },
        0xA5,
        Key::RAlt,
    ),
    (
        WinScan {
            code: 0x39,
            extended: false,
        },
        0x20,
        Key::Space,
    ),
    (
        WinScan {
            code: 0x3A,
            extended: false,
        },
        0x14,
        Key::CapsLock,
    ),
    (
        WinScan {
            code: 0x3B,
            extended: false,
        },
        0x70,
        Key::F1,
    ),
    (
        WinScan {
            code: 0x3C,
            extended: false,
        },
        0x71,
        Key::F2,
    ),
    (
        WinScan {
            code: 0x3D,
            extended: false,
        },
        0x72,
        Key::F3,
    ),
    (
        WinScan {
            code: 0x3E,
            extended: false,
        },
        0x73,
        Key::F4,
    ),
    (
        WinScan {
            code: 0x3F,
            extended: false,
        },
        0x74,
        Key::F5,
    ),
    (
        WinScan {
            code: 0x40,
            extended: false,
        },
        0x75,
        Key::F6,
    ),
    (
        WinScan {
            code: 0x41,
            extended: false,
        },
        0x76,
        Key::F7,
    ),
    (
        WinScan {
            code: 0x42,
            extended: false,
        },
        0x77,
        Key::F8,
    ),
    (
        WinScan {
            code: 0x43,
            extended: false,
        },
        0x78,
        Key::F9,
    ),
    (
        WinScan {
            code: 0x44,
            extended: false,
        },
        0x79,
        Key::F10,
    ),
    (
        WinScan {
            code: 0x45,
            extended: false,
        },
        0x13,
        Key::Pause,
    ),
    (
        WinScan {
            code: 0x45,
            extended: true,
        },
        0x90,
        Key::NumLock,
    ),
    (
        WinScan {
            code: 0x46,
            extended: false,
        },
        0x91,
        Key::ScrollLock,
    ),
    (
        WinScan {
            code: 0x47,
            extended: true,
        },
        0x24,
        Key::Home,
    ),
    (
        WinScan {
            code: 0x47,
            extended: true,
        },
        0x67,
        Key::Numpad7,
    ),
    (
        WinScan {
            code: 0x48,
            extended: true,
        },
        0x26,
        Key::Up,
    ),
    (
        WinScan {
            code: 0x48,
            extended: true,
        },
        0x68,
        Key::Numpad8,
    ),
    (
        WinScan {
            code: 0x49,
            extended: true,
        },
        0x21,
        Key::PageUp,
    ),
    (
        WinScan {
            code: 0x49,
            extended: true,
        },
        0x69,
        Key::Numpad9,
    ),
    (
        WinScan {
            code: 0x4A,
            extended: false,
        },
        0x6D,
        Key::NumpadSubtract,
    ),
    (
        WinScan {
            code: 0x4B,
            extended: true,
        },
        0x25,
        Key::Left,
    ),
    (
        WinScan {
            code: 0x4B,
            extended: true,
        },
        0x64,
        Key::Numpad4,
    ),
    (
        WinScan {
            code: 0x4C,
            extended: false,
        },
        0x65,
        Key::Numpad5,
    ),
    (
        WinScan {
            code: 0x4D,
            extended: true,
        },
        0x27,
        Key::Right,
    ),
    (
        WinScan {
            code: 0x4D,
            extended: true,
        },
        0x66,
        Key::Numpad6,
    ),
    (
        WinScan {
            code: 0x4E,
            extended: false,
        },
        0x6B,
        Key::NumpadAdd,
    ),
    (
        WinScan {
            code: 0x4F,
            extended: true,
        },
        0x23,
        Key::End,
    ),
    (
        WinScan {
            code: 0x4F,
            extended: true,
        },
        0x61,
        Key::Numpad1,
    ),
    (
        WinScan {
            code: 0x50,
            extended: true,
        },
        0x28,
        Key::Down,
    ),
    (
        WinScan {
            code: 0x50,
            extended: true,
        },
        0x62,
        Key::Numpad2,
    ),
    (
        WinScan {
            code: 0x51,
            extended: true,
        },
        0x22,
        Key::PageDown,
    ),
    (
        WinScan {
            code: 0x51,
            extended: true,
        },
        0x63,
        Key::Numpad3,
    ),
    (
        WinScan {
            code: 0x52,
            extended: true,
        },
        0x2D,
        Key::Insert,
    ),
    (
        WinScan {
            code: 0x52,
            extended: true,
        },
        0x60,
        Key::Numpad0,
    ),
    (
        WinScan {
            code: 0x53,
            extended: true,
        },
        0x2E,
        Key::Delete,
    ),
    (
        WinScan {
            code: 0x53,
            extended: true,
        },
        0x6E,
        Key::NumpadDecimal,
    ),
    (
        WinScan {
            code: 0x56,
            extended: false,
        },
        0xE2,
        Key::IntlBackslash,
    ),
    (
        WinScan {
            code: 0x57,
            extended: false,
        },
        0x7A,
        Key::F11,
    ),
    (
        WinScan {
            code: 0x58,
            extended: false,
        },
        0x7B,
        Key::F12,
    ),
    (
        WinScan {
            code: 0x5B,
            extended: true,
        },
        0x5B,
        Key::LMeta,
    ),
    (
        WinScan {
            code: 0x5C,
            extended: true,
        },
        0x5C,
        Key::RMeta,
    ),
    (
        WinScan {
            code: 0x5D,
            extended: true,
        },
        0x5D,
        Key::Menu,
    ),
    (
        WinScan {
            code: 0x64,
            extended: false,
        },
        0x7C,
        Key::F13,
    ),
    (
        WinScan {
            code: 0x65,
            extended: false,
        },
        0x7D,
        Key::F14,
    ),
    (
        WinScan {
            code: 0x66,
            extended: false,
        },
        0x7E,
        Key::F15,
    ),
    (
        WinScan {
            code: 0x67,
            extended: false,
        },
        0x7F,
        Key::F16,
    ),
    (
        WinScan {
            code: 0x68,
            extended: false,
        },
        0x80,
        Key::F17,
    ),
    (
        WinScan {
            code: 0x69,
            extended: false,
        },
        0x81,
        Key::F18,
    ),
    (
        WinScan {
            code: 0x6A,
            extended: false,
        },
        0x82,
        Key::F19,
    ),
    (
        WinScan {
            code: 0x6B,
            extended: false,
        },
        0x83,
        Key::F20,
    ),
    (
        WinScan {
            code: 0x6C,
            extended: false,
        },
        0x84,
        Key::F21,
    ),
    (
        WinScan {
            code: 0x6D,
            extended: false,
        },
        0x85,
        Key::F22,
    ),
    (
        WinScan {
            code: 0x6E,
            extended: false,
        },
        0x86,
        Key::F23,
    ),
    (
        WinScan {
            code: 0x6F,
            extended: false,
        },
        0x87,
        Key::F24,
    ),
];

const MAC_KEYCODE_TABLE: &[(MacKeycode, Key)] = &[
    (0x00, Key::A),
    (0x01, Key::S),
    (0x02, Key::D),
    (0x03, Key::F),
    (0x04, Key::H),
    (0x05, Key::G),
    (0x06, Key::Z),
    (0x07, Key::X),
    (0x08, Key::C),
    (0x09, Key::V),
    (0x0B, Key::B),
    (0x0C, Key::Q),
    (0x0D, Key::W),
    (0x0E, Key::E),
    (0x0F, Key::R),
    (0x10, Key::Y),
    (0x11, Key::T),
    (0x12, Key::Num1),
    (0x13, Key::Num2),
    (0x14, Key::Num3),
    (0x15, Key::Num4),
    (0x16, Key::Num6),
    (0x17, Key::Num5),
    (0x18, Key::Equal),
    (0x19, Key::Num9),
    (0x1A, Key::Num7),
    (0x1B, Key::Minus),
    (0x1C, Key::Num8),
    (0x1D, Key::Num0),
    (0x1E, Key::BracketRight),
    (0x1F, Key::O),
    (0x20, Key::U),
    (0x21, Key::BracketLeft),
    (0x22, Key::I),
    (0x23, Key::P),
    (0x24, Key::Enter),
    (0x25, Key::L),
    (0x26, Key::J),
    (0x27, Key::Quote),
    (0x28, Key::K),
    (0x29, Key::Semicolon),
    (0x2A, Key::Backslash),
    (0x2B, Key::Comma),
    (0x2C, Key::Slash),
    (0x2D, Key::N),
    (0x2E, Key::M),
    (0x2F, Key::Period),
    (0x30, Key::Tab),
    (0x31, Key::Space),
    (0x32, Key::Backquote),
    (0x33, Key::Backspace),
    (0x35, Key::Escape),
    (0x36, Key::RMeta),
    (0x37, Key::LMeta),
    (0x38, Key::LShift),
    (0x39, Key::CapsLock),
    (0x3A, Key::LAlt),
    (0x3B, Key::LCtrl),
    (0x3C, Key::RShift),
    (0x3D, Key::RAlt),
    (0x3E, Key::RCtrl),
    (0x3F, Key::F17),
    (0x41, Key::NumpadDecimal),
    (0x43, Key::NumpadMultiply),
    (0x45, Key::NumpadAdd),
    (0x47, Key::NumLock),
    (0x4B, Key::NumpadDivide),
    (0x4C, Key::NumpadEnter),
    (0x4E, Key::NumpadSubtract),
    (0x4F, Key::F18),
    (0x50, Key::F19),
    (0x52, Key::Numpad0),
    (0x53, Key::Numpad1),
    (0x54, Key::Numpad2),
    (0x55, Key::Numpad3),
    (0x56, Key::Numpad4),
    (0x57, Key::Numpad5),
    (0x58, Key::Numpad6),
    (0x59, Key::Numpad7),
    (0x5A, Key::F20),
    (0x5B, Key::Numpad8),
    (0x5C, Key::Numpad9),
    (0x60, Key::F5),
    (0x61, Key::F6),
    (0x62, Key::F7),
    (0x63, Key::F3),
    (0x64, Key::F8),
    (0x65, Key::F9),
    (0x67, Key::F11),
    (0x69, Key::F13),
    (0x6A, Key::F16),
    (0x6B, Key::F14),
    (0x6D, Key::F10),
    (0x6F, Key::F12),
    (0x71, Key::F15),
    (0x72, Key::Insert),
    (0x73, Key::Home),
    (0x74, Key::PageUp),
    (0x75, Key::Delete),
    (0x76, Key::F4),
    (0x77, Key::End),
    (0x78, Key::F2),
    (0x79, Key::PageDown),
    (0x7A, Key::F1),
    (0x7B, Key::Left),
    (0x7C, Key::Right),
    (0x7D, Key::Down),
    (0x7E, Key::Up),
    // Synthetic codes for keys the Mac keyboard does not have.
    (MAC_PSEUDO_PRINTSCREEN, Key::PrintScreen),
    (MAC_PSEUDO_SCROLLLOCK, Key::ScrollLock),
    (MAC_PSEUDO_PAUSE, Key::Pause),
    (MAC_PSEUDO_INTL_BACKSLASH, Key::IntlBackslash),
    (MAC_PSEUDO_F21, Key::F21),
    (MAC_PSEUDO_F22, Key::F22),
    (MAC_PSEUDO_F23, Key::F23),
    (MAC_PSEUDO_F24, Key::F24),
    // The Mac "menu" key sits where a PC context-menu key does.
    (0x5D, Key::Menu),
];

impl Key {
    /// Map a Windows scan code (plus extended flag and virtual-key code) to a key.
    ///
    /// A handful of scan codes are ambiguous between a navigation key and a
    /// numpad key (both use the E0 prefix). Those are resolved with the
    /// virtual-key code that the low-level hook also reports.
    pub fn from_win_scan(scan: WinScan, vk: WinVk) -> Option<Key> {
        let mut candidates = WIN_SCAN_TABLE
            .iter()
            .filter(|(s, _, _)| *s == scan)
            .map(|(_, v, k)| (*v, *k));
        let first = candidates.next()?;
        match candidates.next() {
            None => Some(first.1),
            Some((other_vk, other_key)) => {
                if first.0 == vk {
                    Some(first.1)
                } else if other_vk == vk {
                    Some(other_key)
                } else {
                    None
                }
            }
        }
    }

    /// Scan code and virtual-key code used to inject this key on Windows.
    pub fn to_win_scan(self) -> Option<(WinScan, WinVk)> {
        WIN_SCAN_TABLE
            .iter()
            .find(|(_, _, k)| *k == self)
            .map(|(s, v, _)| (*s, *v))
    }

    /// Map a macOS hardware virtual key code to a key.
    pub fn from_mac_keycode(code: MacKeycode) -> Option<Key> {
        MAC_KEYCODE_TABLE
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, k)| *k)
    }

    /// macOS hardware virtual key code used to inject this key.
    pub fn to_mac_keycode(self) -> Option<MacKeycode> {
        MAC_KEYCODE_TABLE
            .iter()
            .find(|(_, k)| *k == self)
            .map(|(c, _)| *c)
    }

    /// The real macOS key code to post for a key, resolving synthetic codes.
    ///
    /// macOS has no PrintScreen/ScrollLock/Pause hardware key, so those are
    /// delivered as their closest function-key equivalents.
    pub fn mac_injection_keycode(self) -> Option<MacKeycode> {
        Some(match self {
            Key::PrintScreen => 0x69, // F13
            Key::ScrollLock => 0x6B,  // F14
            Key::Pause => 0x71,       // F15
            // ISO section key, where a PC has its extra backslash key.
            Key::IntlBackslash => 0x0A,
            // F21-F24 have no Apple equivalent at all.
            Key::F21 | Key::F22 | Key::F23 | Key::F24 => return None,
            other => return other.to_mac_keycode(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win_scan_table_resolves() {
        for (scan, vk, key) in WIN_SCAN_TABLE {
            let got = Key::from_win_scan(*scan, *vk).expect("mapped");
            assert_eq!(got, *key, "scan {:?} vk {:x}", scan, vk);
        }
    }

    #[test]
    fn win_ambiguous_codes_resolve_by_vk() {
        // E0 37 is both PrintScreen and numpad multiply.
        assert_eq!(
            Key::from_win_scan(
                WinScan {
                    code: 0x37,
                    extended: true
                },
                0x2C
            ),
            Some(Key::PrintScreen)
        );
        assert_eq!(
            Key::from_win_scan(
                WinScan {
                    code: 0x37,
                    extended: true
                },
                0x6A
            ),
            Some(Key::NumpadMultiply)
        );
        // E0 45 is NumLock; Pause arrives as plain 45 with VK_PAUSE.
        assert_eq!(
            Key::from_win_scan(
                WinScan {
                    code: 0x45,
                    extended: true
                },
                0x90
            ),
            Some(Key::NumLock)
        );
        assert_eq!(
            Key::from_win_scan(
                WinScan {
                    code: 0x45,
                    extended: false
                },
                0x13
            ),
            Some(Key::Pause)
        );
        // E0 52 is both Insert and numpad 0.
        assert_eq!(
            Key::from_win_scan(
                WinScan {
                    code: 0x52,
                    extended: true
                },
                0x2D
            ),
            Some(Key::Insert)
        );
        assert_eq!(
            Key::from_win_scan(
                WinScan {
                    code: 0x52,
                    extended: true
                },
                0x60
            ),
            Some(Key::Numpad0)
        );
    }

    #[test]
    fn every_key_has_a_windows_mapping() {
        for key in Key::ALL {
            assert!(key.to_win_scan().is_some(), "{:?} missing win scan", key);
        }
    }

    #[test]
    fn every_key_has_a_macos_mapping() {
        for key in Key::ALL {
            assert!(
                key.to_mac_keycode().is_some(),
                "{:?} missing mac keycode",
                key
            );
            // F21-F24 exist on a PC keyboard but have no Apple equivalent, so
            // they are represented by synthetic codes and are simply not
            // injectable on the Mac.
            if matches!(key, Key::F21 | Key::F22 | Key::F23 | Key::F24) {
                continue;
            }
            assert!(
                key.mac_injection_keycode().is_some(),
                "{:?} missing mac injection keycode",
                key
            );
        }
    }

    #[test]
    fn mac_keycode_table_resolves() {
        for (code, key) in MAC_KEYCODE_TABLE {
            assert_eq!(Key::from_mac_keycode(*code), Some(*key));
        }
    }

    #[test]
    fn ids_are_unique_and_reversible() {
        for key in Key::ALL {
            assert_eq!(Key::from_id(key.id()), Some(*key));
        }
    }
}
