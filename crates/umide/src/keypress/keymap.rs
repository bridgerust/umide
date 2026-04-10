use std::{fmt::Display, str::FromStr};

use floem::prelude::{Code, Key, Modifiers, NamedKey};
use floem::ui_events::pointer::PointerButton;
use umide_core::mode::Modes;

#[derive(PartialEq, Debug, Clone)]
pub enum KeymapMatch {
    Full(String),
    Multiple(Vec<String>),
    Prefix,
    None,
}

#[derive(PartialEq, Eq, Hash, Clone, Debug)]
pub struct KeyMap {
    pub key: Vec<KeyMapPress>,
    pub modes: Modes,
    pub when: Option<String>,
    pub command: String,
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum KeyMapKey {
    Pointer(PointerButton),
    Logical(Key),
    Physical(Code),
}

impl std::hash::Hash for KeyMapKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Self::Pointer(btn) => btn.hash(state),
            Self::Logical(key) => key.hash(state),
            Self::Physical(code) => code.hash(state),
        }
    }
}

#[derive(PartialEq, Eq, Hash, Clone, Debug)]
pub struct KeyMapPress {
    pub key: KeyMapKey,
    pub mods: Modifiers,
}

impl KeyMapPress {
    pub fn is_char(&self) -> bool {
        let mut mods = self.mods;
        mods.set(Modifiers::SHIFT, false);
        if mods.is_empty() {
            if let KeyMapKey::Logical(Key::Character(_)) = &self.key {
                return true;
            }
        }
        false
    }

    pub fn is_modifiers(&self) -> bool {
        if let KeyMapKey::Physical(code) = &self.key {
            matches!(
                code,
                Code::MetaLeft
                    | Code::MetaRight
                    | Code::ShiftLeft
                    | Code::ShiftRight
                    | Code::ControlLeft
                    | Code::ControlRight
                    | Code::AltLeft
                    | Code::AltRight
            )
        } else if let KeyMapKey::Logical(Key::Named(key)) = &self.key {
            matches!(
                key,
                NamedKey::Meta
                    | NamedKey::Shift
                    | NamedKey::Control
                    | NamedKey::Alt
                    | NamedKey::AltGraph
            )
        } else {
            false
        }
    }

    pub fn label(&self) -> String {
        let mut keys = String::from("");
        if self.mods.ctrl() {
            keys.push_str("Ctrl+");
        }
        if self.mods.alt() {
            keys.push_str("Alt+");
        }
        if self.mods.alt() {
            keys.push_str("AltGr+");
        }
        if self.mods.meta() {
            let keyname = match std::env::consts::OS {
                "macos" => "Cmd+",
                "windows" => "Win+",
                _ => "Meta+",
            };
            keys.push_str(keyname);
        }
        if self.mods.shift() {
            keys.push_str("Shift+");
        }
        keys.push_str(&self.key.to_string());
        keys
    }

    pub fn parse(key: &str) -> Vec<Self> {
        key.split(' ')
            .filter_map(|k| {
                let (modifiers, key) = if k == "+" {
                    ("", "+")
                } else if let Some(remaining) = k.strip_suffix("++") {
                    (remaining, "+")
                } else {
                    match k.rsplit_once('+') {
                        Some(pair) => pair,
                        None => ("", k),
                    }
                };

                let key = match key.parse().ok() {
                    Some(key) => key,
                    None => {
                        // Skip past unrecognized key definitions
                        tracing::warn!("Unrecognized key: {key}");
                        return None;
                    }
                };

                let mut mods = Modifiers::empty();
                for part in modifiers.to_lowercase().split('+') {
                    match part {
                        "ctrl" => mods.set(Modifiers::CONTROL, true),
                        "meta" => mods.set(Modifiers::META, true),
                        "shift" => mods.set(Modifiers::SHIFT, true),
                        "alt" => mods.set(Modifiers::ALT, true),
                        "altgr" => mods.set(Modifiers::ALT, true),
                        "" => (),
                        other => tracing::warn!("Invalid key modifier: {}", other),
                    }
                }

                Some(KeyMapPress { key, mods })
            })
            .collect()
    }
}

impl FromStr for KeyMapKey {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let key = if s.starts_with('[') && s.ends_with(']') {
            let code = match s[1..s.len() - 2].to_lowercase().as_str() {
                "esc" => Code::Escape,
                "space" => Code::Space,
                "bs" => Code::Backspace,
                "up" => Code::ArrowUp,
                "down" => Code::ArrowDown,
                "left" => Code::ArrowLeft,
                "right" => Code::ArrowRight,
                "del" => Code::Delete,
                "alt" => Code::AltLeft,
                "altgraph" => Code::AltRight,
                "capslock" => Code::CapsLock,
                "control" => Code::ControlLeft,
                "fn" => Code::Fn,
                "fnlock" => Code::FnLock,
                "meta" => Code::MetaRight,
                "numlock" => Code::NumLock,
                "scrolllock" => Code::ScrollLock,
                "shift" => Code::ShiftLeft,
                "hyper" => Code::MetaLeft,
                "super" => Code::MetaRight,
                "enter" => Code::Enter,
                "tab" => Code::Tab,
                "arrowdown" => Code::ArrowDown,
                "arrowleft" => Code::ArrowLeft,
                "arrowright" => Code::ArrowRight,
                "arrowup" => Code::ArrowUp,
                "end" => Code::End,
                "home" => Code::Home,
                "pagedown" => Code::PageDown,
                "pageup" => Code::PageUp,
                "backspace" => Code::Backspace,
                "copy" => Code::Copy,
                "cut" => Code::Cut,
                "delete" => Code::Delete,
                "insert" => Code::Insert,
                "paste" => Code::Paste,
                "undo" => Code::Undo,
                "again" => Code::Again,
                "contextmenu" => Code::ContextMenu,
                "escape" => Code::Escape,
                "find" => Code::Find,
                "help" => Code::Help,
                "pause" => Code::Pause,
                "play" => Code::MediaPlayPause,
                "props" => Code::Props,
                "select" => Code::Select,
                "eject" => Code::Eject,
                "power" => Code::Power,
                "printscreen" => Code::PrintScreen,
                "wakeup" => Code::WakeUp,
                "convert" => Code::Convert,
                "nonconvert" => Code::NonConvert,
                "hiragana" => Code::Hiragana,
                "katakana" => Code::Katakana,
                "f1" => Code::F1,
                "f2" => Code::F2,
                "f3" => Code::F3,
                "f4" => Code::F4,
                "f5" => Code::F5,
                "f6" => Code::F6,
                "f7" => Code::F7,
                "f8" => Code::F8,
                "f9" => Code::F9,
                "f10" => Code::F10,
                "f11" => Code::F11,
                "f12" => Code::F12,
                "mediastop" => Code::MediaStop,
                "open" => Code::Open,
                _ => {
                    return Err(anyhow::anyhow!(
                        "unrecognized physical key code {}",
                        &s[1..s.len() - 2]
                    ));
                }
            };
            KeyMapKey::Physical(code)
        } else {
            let key = match s.to_lowercase().as_str() {
                "esc" => Key::Named(NamedKey::Escape),
                "bs" => Key::Named(NamedKey::Backspace),
                "up" => Key::Named(NamedKey::ArrowUp),
                "down" => Key::Named(NamedKey::ArrowDown),
                "left" => Key::Named(NamedKey::ArrowLeft),
                "right" => Key::Named(NamedKey::ArrowRight),
                "del" => Key::Named(NamedKey::Delete),
                "alt" => Key::Named(NamedKey::Alt),
                "altgraph" => Key::Named(NamedKey::AltGraph),
                "capslock" => Key::Named(NamedKey::CapsLock),
                "control" => Key::Named(NamedKey::Control),
                "fn" => Key::Named(NamedKey::Fn),
                "fnlock" => Key::Named(NamedKey::FnLock),
                "meta" => Key::Named(NamedKey::Meta),
                "numlock" => Key::Named(NamedKey::NumLock),
                "scrolllock" => Key::Named(NamedKey::ScrollLock),
                "shift" => Key::Named(NamedKey::Shift),
                "hyper" => Key::Named(NamedKey::Hyper),
                "super" => Key::Named(NamedKey::Meta),
                "enter" => Key::Named(NamedKey::Enter),
                "tab" => Key::Named(NamedKey::Tab),
                "arrowdown" => Key::Named(NamedKey::ArrowDown),
                "arrowleft" => Key::Named(NamedKey::ArrowLeft),
                "arrowright" => Key::Named(NamedKey::ArrowRight),
                "arrowup" => Key::Named(NamedKey::ArrowUp),
                "end" => Key::Named(NamedKey::End),
                "home" => Key::Named(NamedKey::Home),
                "pagedown" => Key::Named(NamedKey::PageDown),
                "pageup" => Key::Named(NamedKey::PageUp),
                "backspace" => Key::Named(NamedKey::Backspace),
                "copy" => Key::Named(NamedKey::Copy),
                "cut" => Key::Named(NamedKey::Cut),
                "delete" => Key::Named(NamedKey::Delete),
                "insert" => Key::Named(NamedKey::Insert),
                "paste" => Key::Named(NamedKey::Paste),
                "undo" => Key::Named(NamedKey::Undo),
                "again" => Key::Named(NamedKey::Again),
                "contextmenu" => Key::Named(NamedKey::ContextMenu),
                "escape" => Key::Named(NamedKey::Escape),
                "find" => Key::Named(NamedKey::Find),
                "help" => Key::Named(NamedKey::Help),
                "pause" => Key::Named(NamedKey::Pause),
                "play" => Key::Named(NamedKey::MediaPlayPause),
                "props" => Key::Named(NamedKey::Props),
                "select" => Key::Named(NamedKey::Select),
                "eject" => Key::Named(NamedKey::Eject),
                "power" => Key::Named(NamedKey::Power),
                "printscreen" => Key::Named(NamedKey::PrintScreen),
                "wakeup" => Key::Named(NamedKey::WakeUp),
                "convert" => Key::Named(NamedKey::Convert),
                "nonconvert" => Key::Named(NamedKey::NonConvert),
                "hiragana" => Key::Named(NamedKey::Hiragana),
                "katakana" => Key::Named(NamedKey::Katakana),
                "f1" => Key::Named(NamedKey::F1),
                "f2" => Key::Named(NamedKey::F2),
                "f3" => Key::Named(NamedKey::F3),
                "f4" => Key::Named(NamedKey::F4),
                "f5" => Key::Named(NamedKey::F5),
                "f6" => Key::Named(NamedKey::F6),
                "f7" => Key::Named(NamedKey::F7),
                "f8" => Key::Named(NamedKey::F8),
                "f9" => Key::Named(NamedKey::F9),
                "f10" => Key::Named(NamedKey::F10),
                "f11" => Key::Named(NamedKey::F11),
                "f12" => Key::Named(NamedKey::F12),
                "mediastop" => Key::Named(NamedKey::MediaStop),
                "open" => Key::Named(NamedKey::Open),
                _ => Key::Character(s.to_lowercase()),
            };
            KeyMapKey::Logical(key)
        };
        Ok(key)
    }
}

impl Display for KeyMapPress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.mods.contains(Modifiers::CONTROL) {
            if let Err(err) = f.write_str("Ctrl+") {
                tracing::error!("{:?}", err);
            }
        }
        if self.mods.contains(Modifiers::ALT) {
            if let Err(err) = f.write_str("Alt+") {
                tracing::error!("{:?}", err);
            }
        }
        if self.mods.contains(Modifiers::ALT) {
            if let Err(err) = f.write_str("AltGr+") {
                tracing::error!("{:?}", err);
            }
        }
        if self.mods.contains(Modifiers::META) {
            if let Err(err) = f.write_str("Meta+") {
                tracing::error!("{:?}", err);
            }
        }
        if self.mods.contains(Modifiers::SHIFT) {
            if let Err(err) = f.write_str("Shift+") {
                tracing::error!("{:?}", err);
            }
        }
        f.write_str(&self.key.to_string())
    }
}

impl Display for KeyMapKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Physical(code) => {
                f.write_str("[")?;
                match code {
                    Code::Backquote => f.write_str("Backquote"),
                    Code::Backslash => f.write_str("Backslash"),
                    Code::BracketLeft => f.write_str("BracketLeft"),
                    Code::BracketRight => f.write_str("BracketRight"),
                    Code::Comma => f.write_str("Comma"),
                    Code::Digit0 => f.write_str("0"),
                    Code::Digit1 => f.write_str("1"),
                    Code::Digit2 => f.write_str("2"),
                    Code::Digit3 => f.write_str("3"),
                    Code::Digit4 => f.write_str("4"),
                    Code::Digit5 => f.write_str("5"),
                    Code::Digit6 => f.write_str("6"),
                    Code::Digit7 => f.write_str("7"),
                    Code::Digit8 => f.write_str("8"),
                    Code::Digit9 => f.write_str("9"),
                    Code::Equal => f.write_str("Equal"),
                    Code::IntlBackslash => f.write_str("IntlBackslash"),
                    Code::IntlRo => f.write_str("IntlRo"),
                    Code::IntlYen => f.write_str("IntlYen"),
                    Code::KeyA => f.write_str("A"),
                    Code::KeyB => f.write_str("B"),
                    Code::KeyC => f.write_str("C"),
                    Code::KeyD => f.write_str("D"),
                    Code::KeyE => f.write_str("E"),
                    Code::KeyF => f.write_str("F"),
                    Code::KeyG => f.write_str("G"),
                    Code::KeyH => f.write_str("H"),
                    Code::KeyI => f.write_str("I"),
                    Code::KeyJ => f.write_str("J"),
                    Code::KeyK => f.write_str("K"),
                    Code::KeyL => f.write_str("L"),
                    Code::KeyM => f.write_str("M"),
                    Code::KeyN => f.write_str("N"),
                    Code::KeyO => f.write_str("O"),
                    Code::KeyP => f.write_str("P"),
                    Code::KeyQ => f.write_str("Q"),
                    Code::KeyR => f.write_str("R"),
                    Code::KeyS => f.write_str("S"),
                    Code::KeyT => f.write_str("T"),
                    Code::KeyU => f.write_str("U"),
                    Code::KeyV => f.write_str("V"),
                    Code::KeyW => f.write_str("W"),
                    Code::KeyX => f.write_str("X"),
                    Code::KeyY => f.write_str("Y"),
                    Code::KeyZ => f.write_str("Z"),
                    Code::Minus => f.write_str("Minus"),
                    Code::Period => f.write_str("Period"),
                    Code::Quote => f.write_str("Quote"),
                    Code::Semicolon => f.write_str("Semicolon"),
                    Code::Slash => f.write_str("Slash"),
                    Code::AltLeft => f.write_str("Alt"),
                    Code::AltRight => f.write_str("Alt"),
                    Code::Backspace => f.write_str("Backspace"),
                    Code::CapsLock => f.write_str("CapsLock"),
                    Code::ContextMenu => f.write_str("ContextMenu"),
                    Code::ControlLeft => f.write_str("Ctrl"),
                    Code::ControlRight => f.write_str("Ctrl"),
                    Code::Enter => f.write_str("Enter"),
                    Code::MetaLeft => f.write_str("Meta"),
                    Code::MetaRight => f.write_str("Meta"),
                    Code::ShiftLeft => f.write_str("Shift"),
                    Code::ShiftRight => f.write_str("Shift"),
                    Code::Space => f.write_str("Space"),
                    Code::Tab => f.write_str("Tab"),
                    Code::Convert => f.write_str("Convert"),
                    Code::KanaMode => f.write_str("KanaMode"),
                    Code::Lang1 => f.write_str("Lang1"),
                    Code::Lang2 => f.write_str("Lang2"),
                    Code::Lang3 => f.write_str("Lang3"),
                    Code::Lang4 => f.write_str("Lang4"),
                    Code::Lang5 => f.write_str("Lang5"),
                    Code::NonConvert => f.write_str("NonConvert"),
                    Code::Delete => f.write_str("Delete"),
                    Code::End => f.write_str("End"),
                    Code::Help => f.write_str("Help"),
                    Code::Home => f.write_str("Home"),
                    Code::Insert => f.write_str("Insert"),
                    Code::PageDown => f.write_str("PageDown"),
                    Code::PageUp => f.write_str("PageUp"),
                    Code::ArrowDown => f.write_str("Down"),
                    Code::ArrowLeft => f.write_str("Left"),
                    Code::ArrowRight => f.write_str("Right"),
                    Code::ArrowUp => f.write_str("Up"),
                    Code::NumLock => f.write_str("NumLock"),
                    Code::Numpad0 => f.write_str("Numpad0"),
                    Code::Numpad1 => f.write_str("Numpad1"),
                    Code::Numpad2 => f.write_str("Numpad2"),
                    Code::Numpad3 => f.write_str("Numpad3"),
                    Code::Numpad4 => f.write_str("Numpad4"),
                    Code::Numpad5 => f.write_str("Numpad5"),
                    Code::Numpad6 => f.write_str("Numpad6"),
                    Code::Numpad7 => f.write_str("Numpad7"),
                    Code::Numpad8 => f.write_str("Numpad8"),
                    Code::Numpad9 => f.write_str("Numpad9"),
                    Code::NumpadAdd => f.write_str("NumpadAdd"),
                    Code::NumpadBackspace => f.write_str("NumpadBackspace"),
                    Code::NumpadClear => f.write_str("NumpadClear"),
                    Code::NumpadClearEntry => f.write_str("NumpadClearEntry"),
                    Code::NumpadComma => f.write_str("NumpadComma"),
                    Code::NumpadDecimal => f.write_str("NumpadDecimal"),
                    Code::NumpadDivide => f.write_str("NumpadDivide"),
                    Code::NumpadEnter => f.write_str("NumpadEnter"),
                    Code::NumpadEqual => f.write_str("NumpadEqual"),
                    Code::NumpadHash => f.write_str("NumpadHash"),
                    Code::NumpadMemoryAdd => f.write_str("NumpadMemoryAdd"),
                    Code::NumpadMemoryClear => f.write_str("NumpadMemoryClear"),
                    Code::NumpadMemoryRecall => f.write_str("NumpadMemoryRecall"),
                    Code::NumpadMemoryStore => f.write_str("NumpadMemoryStore"),
                    Code::NumpadMemorySubtract => {
                        f.write_str("NumpadMemorySubtract")
                    }
                    Code::NumpadMultiply => f.write_str("NumpadMultiply"),
                    Code::NumpadParenLeft => f.write_str("NumpadParenLeft"),
                    Code::NumpadParenRight => f.write_str("NumpadParenRight"),
                    Code::NumpadStar => f.write_str("NumpadStar"),
                    Code::NumpadSubtract => f.write_str("NumpadSubtract"),
                    Code::Escape => f.write_str("Escape"),
                    Code::Fn => f.write_str("Fn"),
                    Code::FnLock => f.write_str("FnLock"),
                    Code::PrintScreen => f.write_str("PrintScreen"),
                    Code::ScrollLock => f.write_str("ScrollLock"),
                    Code::Pause => f.write_str("Pause"),
                    Code::BrowserBack => f.write_str("BrowserBack"),
                    Code::BrowserFavorites => f.write_str("BrowserFavorites"),
                    Code::BrowserForward => f.write_str("BrowserForward"),
                    Code::BrowserHome => f.write_str("BrowserHome"),
                    Code::BrowserRefresh => f.write_str("BrowserRefresh"),
                    Code::BrowserSearch => f.write_str("BrowserSearch"),
                    Code::BrowserStop => f.write_str("BrowserStop"),
                    Code::Eject => f.write_str("Eject"),
                    Code::LaunchApp1 => f.write_str("LaunchApp1"),
                    Code::LaunchApp2 => f.write_str("LaunchApp2"),
                    Code::LaunchMail => f.write_str("LaunchMail"),
                    Code::MediaPlayPause => f.write_str("MediaPlayPause"),
                    Code::MediaSelect => f.write_str("MediaSelect"),
                    Code::MediaStop => f.write_str("MediaStop"),
                    Code::MediaTrackNext => f.write_str("MediaTrackNext"),
                    Code::MediaTrackPrevious => f.write_str("MediaTrackPrevious"),
                    Code::Power => f.write_str("Power"),
                    Code::Sleep => f.write_str("Sleep"),
                    Code::AudioVolumeDown => f.write_str("AudioVolumeDown"),
                    Code::AudioVolumeMute => f.write_str("AudioVolumeMute"),
                    Code::AudioVolumeUp => f.write_str("AudioVolumeUp"),
                    Code::WakeUp => f.write_str("WakeUp"),
                    Code::Hyper => f.write_str("Hyper"),
                    Code::Turbo => f.write_str("Turbo"),
                    Code::Abort => f.write_str("Abort"),
                    Code::Resume => f.write_str("Resume"),
                    Code::Suspend => f.write_str("Suspend"),
                    Code::Again => f.write_str("Again"),
                    Code::Copy => f.write_str("Copy"),
                    Code::Cut => f.write_str("Cut"),
                    Code::Find => f.write_str("Find"),
                    Code::Open => f.write_str("Open"),
                    Code::Paste => f.write_str("Paste"),
                    Code::Props => f.write_str("Props"),
                    Code::Select => f.write_str("Select"),
                    Code::Undo => f.write_str("Undo"),
                    Code::Hiragana => f.write_str("Hiragana"),
                    Code::Katakana => f.write_str("Katakana"),
                    Code::F1 => f.write_str("F1"),
                    Code::F2 => f.write_str("F2"),
                    Code::F3 => f.write_str("F3"),
                    Code::F4 => f.write_str("F4"),
                    Code::F5 => f.write_str("F5"),
                    Code::F6 => f.write_str("F6"),
                    Code::F7 => f.write_str("F7"),
                    Code::F8 => f.write_str("F8"),
                    Code::F9 => f.write_str("F9"),
                    Code::F10 => f.write_str("F10"),
                    Code::F11 => f.write_str("F11"),
                    Code::F12 => f.write_str("F12"),
                    Code::F13 => f.write_str("F13"),
                    Code::F14 => f.write_str("F14"),
                    Code::F15 => f.write_str("F15"),
                    Code::F16 => f.write_str("F16"),
                    Code::F17 => f.write_str("F17"),
                    Code::F18 => f.write_str("F18"),
                    Code::F19 => f.write_str("F19"),
                    Code::F20 => f.write_str("F20"),
                    Code::F21 => f.write_str("F21"),
                    Code::F22 => f.write_str("F22"),
                    Code::F23 => f.write_str("F23"),
                    Code::F24 => f.write_str("F24"),
                    Code::F25 => f.write_str("F25"),
                    Code::F26 => f.write_str("F26"),
                    Code::F27 => f.write_str("F27"),
                    Code::F28 => f.write_str("F28"),
                    Code::F29 => f.write_str("F29"),
                    Code::F30 => f.write_str("F30"),
                    Code::F31 => f.write_str("F31"),
                    Code::F32 => f.write_str("F32"),
                    Code::F33 => f.write_str("F33"),
                    Code::F34 => f.write_str("F34"),
                    Code::F35 => f.write_str("F35"),
                    _ => f.write_str("Unidentified"),
                }?;
                f.write_str("]")
            }
            Self::Logical(key) => match key {
                Key::Named(key) => match key {
                    NamedKey::Backspace => f.write_str("Backspace"),
                    NamedKey::CapsLock => f.write_str("CapsLock"),
                    NamedKey::Enter => f.write_str("Enter"),
                    NamedKey::Delete => f.write_str("Delete"),
                    NamedKey::End => f.write_str("End"),
                    NamedKey::Home => f.write_str("Home"),
                    NamedKey::PageDown => f.write_str("PageDown"),
                    NamedKey::PageUp => f.write_str("PageUp"),
                    NamedKey::ArrowDown => f.write_str("ArrowDown"),
                    NamedKey::ArrowUp => f.write_str("ArrowUp"),
                    NamedKey::ArrowLeft => f.write_str("ArrowLeft"),
                    NamedKey::ArrowRight => f.write_str("ArrowRight"),
                    NamedKey::Escape => f.write_str("Escape"),
                    NamedKey::Fn => f.write_str("Fn"),
                    NamedKey::Shift => f.write_str("Shift"),
                    NamedKey::Meta => f.write_str("Meta"),
                    NamedKey::Super => f.write_str("Meta"),
                    NamedKey::Control => f.write_str("Ctrl"),
                    NamedKey::Alt => f.write_str("Alt"),
                    NamedKey::AltGraph => f.write_str("AltGraph"),
                    NamedKey::Tab => f.write_str("Tab"),
                    NamedKey::F1 => f.write_str("F1"),
                    NamedKey::F2 => f.write_str("F2"),
                    NamedKey::F3 => f.write_str("F3"),
                    NamedKey::F4 => f.write_str("F4"),
                    NamedKey::F5 => f.write_str("F5"),
                    NamedKey::F6 => f.write_str("F6"),
                    NamedKey::F7 => f.write_str("F7"),
                    NamedKey::F8 => f.write_str("F8"),
                    NamedKey::F9 => f.write_str("F9"),
                    NamedKey::F10 => f.write_str("F10"),
                    NamedKey::F11 => f.write_str("F11"),
                    NamedKey::F12 => f.write_str("F12"),
                    _ => f.write_str("Unidentified"),
                },
                Key::Character(s) => f.write_str(s),
            },
            Self::Pointer(PointerButton::Auxiliary) => f.write_str("MouseMiddle"),
            Self::Pointer(PointerButton::X2) => f.write_str("MouseForward"),
            Self::Pointer(PointerButton::X1) => f.write_str("MouseBackward"),
            Self::Pointer(_) => f.write_str("MouseUnimplemented"),
        }
    }
}
