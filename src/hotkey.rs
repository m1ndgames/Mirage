use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};

pub const DEFAULT: &str = "Ctrl+Shift+R";

/// A parsed global hotkey: modifier flags plus a virtual-key code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: u32,
    pub vk: u32,
}

/// Parses "Ctrl+Alt+R", "Shift+F9", "Win+M" (case-insensitive, any order).
/// Keys: a letter, a digit, or F1–F24. At least one modifier for non-F keys is
/// not enforced – the user's choice.
pub fn parse(text: &str) -> Option<Hotkey> {
    let mut modifiers = HOT_KEY_MODIFIERS(0);
    let mut vk = None;
    for part in text.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= MOD_CONTROL,
            "alt" => modifiers |= MOD_ALT,
            "shift" => modifiers |= MOD_SHIFT,
            "win" | "super" => modifiers |= MOD_WIN,
            key => {
                if vk.is_some() {
                    return None;
                }
                vk = Some(key_code(key)?);
            }
        }
    }
    Some(Hotkey { modifiers: modifiers.0, vk: vk? })
}

fn key_code(key: &str) -> Option<u32> {
    let mut chars = key.chars();
    match (chars.next()?, chars.next()) {
        (c, None) if c.is_ascii_alphanumeric() => Some(c.to_ascii_uppercase() as u32),
        ('f', Some(_)) => {
            let n: u32 = key[1..].parse().ok()?;
            (1..=24).contains(&n).then(|| 0x70 + n - 1) // VK_F1 = 0x70
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modifiers_and_letter() {
        let h = parse("Ctrl+Alt+R").unwrap();
        assert_eq!(h.modifiers, (MOD_CONTROL | MOD_ALT).0);
        assert_eq!(h.vk, 'R' as u32);
    }

    #[test]
    fn is_case_and_order_insensitive() {
        assert_eq!(parse("alt + ctrl + r"), parse("Ctrl+Alt+R"));
        assert_eq!(parse("SHIFT+m").unwrap().vk, 'M' as u32);
    }

    #[test]
    fn parses_function_keys_and_digits() {
        assert_eq!(parse("F9").unwrap(), Hotkey { modifiers: 0, vk: 0x78 });
        assert_eq!(parse("Win+F24").unwrap().vk, 0x87);
        assert_eq!(parse("Ctrl+1").unwrap().vk, '1' as u32);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("").is_none());
        assert!(parse("Ctrl+Alt").is_none());
        assert!(parse("Ctrl+R+M").is_none());
        assert!(parse("F0").is_none());
        assert!(parse("F25").is_none());
        assert!(parse("Ctrl+Enter").is_none());
    }

    #[test]
    fn default_parses() {
        assert!(parse(DEFAULT).is_some());
    }
}
