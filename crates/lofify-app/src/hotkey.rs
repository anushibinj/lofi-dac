//! Global hotkey strings such as "Ctrl+Alt+L": parsing, canonical formatting and conversion.
//!
//! The parser is our own and pure (so it is tested on every OS); conversion to the
//! `global-hotkey` crate's type exists only on Windows and macOS.

use std::fmt;

/// The non-modifier key of a hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Letter(char),
    Digit(u8),
    Function(u8),
    Space,
}

/// A parsed hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Windows key / Command key.
    pub meta: bool,
    pub key: Key,
}

/// Why a hotkey string was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyError {
    Empty,
    NoModifier,
    NoKey,
    TwoKeys(String, String),
    UnknownKey(String),
}

impl fmt::Display for HotkeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HotkeyError::Empty => write!(f, "the hotkey is empty"),
            HotkeyError::NoModifier => {
                write!(f, "use at least one modifier (Ctrl, Alt, Shift or Win/Cmd)")
            }
            HotkeyError::NoKey => write!(f, "the hotkey needs a key, e.g. Ctrl+Alt+L"),
            HotkeyError::TwoKeys(a, b) => write!(f, "only one key is allowed, found {a} and {b}"),
            HotkeyError::UnknownKey(k) => {
                write!(f, "unknown key {k:?} (use A-Z, 0-9, F1-F24 or Space)")
            }
        }
    }
}

impl std::error::Error for HotkeyError {}

fn parse_key(token: &str) -> Result<Key, HotkeyError> {
    let t = token.to_ascii_uppercase();
    let mut chars = t.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_uppercase() {
            return Ok(Key::Letter(c));
        }
        if let Some(d) = c.to_digit(10) {
            return Ok(Key::Digit(d as u8));
        }
    }
    if t == "SPACE" {
        return Ok(Key::Space);
    }
    if let Some(n) = t.strip_prefix('F').and_then(|n| n.parse::<u8>().ok())
        && (1..=24).contains(&n)
    {
        return Ok(Key::Function(n));
    }
    Err(HotkeyError::UnknownKey(token.to_string()))
}

/// Parses "Ctrl+Alt+L" (case and spacing insensitive; also accepts Control, Option, Cmd, Win, Super).
pub fn parse(text: &str) -> Result<Hotkey, HotkeyError> {
    if text.trim().is_empty() {
        return Err(HotkeyError::Empty);
    }
    let (mut ctrl, mut alt, mut shift, mut meta) = (false, false, false, false);
    let mut key: Option<(String, Key)> = None;
    for raw in text.split('+') {
        let token = raw.trim();
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "alt" | "option" | "opt" => alt = true,
            "shift" => shift = true,
            "meta" | "cmd" | "command" | "super" | "win" | "windows" => meta = true,
            "" => return Err(HotkeyError::NoKey),
            _ => {
                let parsed = parse_key(token)?;
                if let Some((first, _)) = &key {
                    return Err(HotkeyError::TwoKeys(first.clone(), token.to_string()));
                }
                key = Some((token.to_string(), parsed));
            }
        }
    }
    if !(ctrl || alt || shift || meta) {
        return Err(HotkeyError::NoModifier);
    }
    let (_, key) = key.ok_or(HotkeyError::NoKey)?;
    Ok(Hotkey {
        ctrl,
        alt,
        shift,
        meta,
        key,
    })
}

impl fmt::Display for Hotkey {
    /// Canonical form: modifiers in the order Ctrl, Alt, Shift, Win, then the key.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            write!(f, "Ctrl+")?;
        }
        if self.alt {
            write!(f, "Alt+")?;
        }
        if self.shift {
            write!(f, "Shift+")?;
        }
        if self.meta {
            write!(
                f,
                "{}+",
                if cfg!(target_os = "macos") {
                    "Cmd"
                } else {
                    "Win"
                }
            )?;
        }
        match self.key {
            Key::Letter(c) => write!(f, "{c}"),
            Key::Digit(d) => write!(f, "{d}"),
            Key::Function(n) => write!(f, "F{n}"),
            Key::Space => write!(f, "Space"),
        }
    }
}

impl Hotkey {
    /// The string form understood by the `global-hotkey` crate's parser, e.g. "control+alt+KeyL".
    pub fn to_global_string(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("control".into());
        }
        if self.alt {
            parts.push("alt".into());
        }
        if self.shift {
            parts.push("shift".into());
        }
        if self.meta {
            parts.push("super".into());
        }
        parts.push(match self.key {
            Key::Letter(c) => format!("Key{c}"),
            Key::Digit(d) => format!("Digit{d}"),
            Key::Function(n) => format!("F{n}"),
            Key::Space => "Space".into(),
        });
        parts.join("+")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_default() {
        let h = parse("Ctrl+Alt+L").unwrap();
        assert_eq!(
            h,
            Hotkey {
                ctrl: true,
                alt: true,
                shift: false,
                meta: false,
                key: Key::Letter('L')
            }
        );
        assert_eq!(h.to_string(), "Ctrl+Alt+L");
        assert_eq!(h.to_global_string(), "control+alt+KeyL");
    }

    #[test]
    fn is_case_space_and_alias_insensitive() {
        assert_eq!(
            parse(" control + option + l ").unwrap().to_string(),
            "Ctrl+Alt+L"
        );
        assert!(parse("shift+CMD+f5").unwrap().meta);
        assert_eq!(parse("SUPER+shift+9").unwrap().key, Key::Digit(9));
    }

    #[test]
    fn canonical_order_is_stable() {
        assert_eq!(
            parse("Shift+Alt+Ctrl+F12").unwrap().to_string(),
            "Ctrl+Alt+Shift+F12"
        );
        assert_eq!(parse("Ctrl+Space").unwrap().to_string(), "Ctrl+Space");
    }

    #[test]
    fn roundtrips_through_display() {
        for text in [
            "Ctrl+Alt+L",
            "Ctrl+Shift+F1",
            "Alt+7",
            "Ctrl+Alt+Shift+Space",
        ] {
            let h = parse(text).unwrap();
            assert_eq!(parse(&h.to_string()).unwrap(), h, "{text}");
        }
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(parse(""), Err(HotkeyError::Empty));
        assert_eq!(parse("   "), Err(HotkeyError::Empty));
        assert_eq!(parse("L"), Err(HotkeyError::NoModifier));
        assert_eq!(parse("Ctrl+Alt"), Err(HotkeyError::NoKey));
        assert_eq!(parse("Ctrl+"), Err(HotkeyError::NoKey));
        assert!(matches!(parse("Ctrl+A+B"), Err(HotkeyError::TwoKeys(_, _))));
        assert!(matches!(
            parse("Ctrl+Banana"),
            Err(HotkeyError::UnknownKey(_))
        ));
        assert!(matches!(parse("Ctrl+F25"), Err(HotkeyError::UnknownKey(_))));
        assert!(matches!(parse("Ctrl+F0"), Err(HotkeyError::UnknownKey(_))));
    }

    #[test]
    fn errors_are_readable() {
        assert!(parse("L").unwrap_err().to_string().contains("modifier"));
        assert!(
            parse("Ctrl+Banana")
                .unwrap_err()
                .to_string()
                .contains("Banana")
        );
    }
}
