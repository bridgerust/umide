use floem::{
    prelude::{Code, Key, NamedKey},
    ui_events::pointer::PointerButton,
};

use super::keymap::KeyMapKey;

#[derive(Clone, Debug)]
pub(crate) enum KeyInput {
    Pointer(PointerButton),
    Keyboard {
        physical: Code,
        logical: Key,
        key_without_modifiers: Key,
        repeat: bool,
    },
}

impl KeyInput {
    pub fn keymap_key(&self) -> Option<KeyMapKey> {
        if let KeyInput::Keyboard {
            repeat, logical, ..
        } = self
        {
            if *repeat
                && (matches!(
                    logical,
                    Key::Named(NamedKey::Meta)
                        | Key::Named(NamedKey::Shift)
                        | Key::Named(NamedKey::Alt)
                        | Key::Named(NamedKey::Control),
                ))
            {
                return None;
            }
        }

        Some(match self {
            KeyInput::Pointer(b) => KeyMapKey::Pointer(*b),
            KeyInput::Keyboard {
                physical,
                key_without_modifiers,
                ..
            } => {
                // Location check removed as KeyLocation is unavailable
                // Assumed Numpad handling logic if specific behavior needed logic here would check logical key
                match key_without_modifiers {
                    Key::Named(_) => {
                        KeyMapKey::Logical(key_without_modifiers.to_owned())
                    }
                    Key::Character(c) => {
                        if c == " " {
                            KeyMapKey::Logical(Key::Character(" ".to_string()))
                        } else if c.len() == 1 && c.is_ascii() {
                            KeyMapKey::Logical(Key::Character(c.to_lowercase()))
                        } else {
                            KeyMapKey::Physical(*physical)
                        }
                    }
                }
            }
        })
    }
}
