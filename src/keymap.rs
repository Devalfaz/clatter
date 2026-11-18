//! Keyboard geometry and key classification.
//!
//! Keys are identified by a 9-bit "key code": the PC set-1 scan code in the low
//! byte, plus 0x100 when Windows reports the key as extended (LLKHF_EXTENDED).

pub const NUM_CODES: usize = 512;

/// Which family of sounds a key uses. Bigger keys sound lower and rattle more.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyGroup {
    Regular,
    Wide,
    Backspace,
    Enter,
    Space,
}

impl KeyGroup {
    pub const ALL: [KeyGroup; 5] = [
        KeyGroup::Regular,
        KeyGroup::Wide,
        KeyGroup::Backspace,
        KeyGroup::Enter,
        KeyGroup::Space,
    ];

    pub fn name(self) -> &'static str {
        match self {
            KeyGroup::Regular => "regular",
            KeyGroup::Wide => "wide",
            KeyGroup::Backspace => "backspace",
            KeyGroup::Enter => "enter",
            KeyGroup::Space => "space",
        }
    }

    pub fn from_name(s: &str) -> Option<KeyGroup> {
        KeyGroup::ALL.into_iter().find(|g| g.name() == s)
    }
}

pub fn code(scan: u32, extended: bool) -> u16 {
    ((scan & 0xFF) as u16) | if extended { 0x100 } else { 0 }
}

pub fn group(code: u16) -> KeyGroup {
    match code {
        0x039 => KeyGroup::Space,
        0x01C | 0x11C => KeyGroup::Enter,
        0x00E => KeyGroup::Backspace,
        // Tab, Caps, Shifts, Ctrls, Alts, Win, Menu
        0x00F | 0x03A | 0x02A | 0x036 | 0x01D | 0x11D | 0x038 | 0x138 | 0x15B | 0x15C | 0x15D => {
            KeyGroup::Wide
        }
        _ => KeyGroup::Regular,
    }
}

pub fn is_modifier(code: u16) -> bool {
    matches!(
        code,
        0x02A | 0x036 | 0x01D | 0x11D | 0x038 | 0x138 | 0x15B | 0x15C
    )
}

pub fn is_numpad(code: u16) -> bool {
    matches!(code, 0x037 | 0x04A | 0x04E | 0x047..=0x053 | 0x135 | 0x11C | 0x145)
}

pub fn is_function(code: u16) -> bool {
    match code {
        0x03B..=0x044 | 0x057 | 0x058 => true, // F1-F12
        0x137 | 0x046 | 0x045 => true,         // PrtSc, ScrLk, Pause
        c if c >= 0x100 => position_units(c).is_none(), // media / browser keys
        _ => false,
    }
}

/// Horizontal centre of the key on a full-size ANSI board, in key units (1u = one letter key).
fn position_units(code: u16) -> Option<f32> {
    let x = match code {
        // Function row
        0x001 => 0.5,
        0x03B => 2.5,
        0x03C => 3.5,
        0x03D => 4.5,
        0x03E => 5.5,
        0x03F => 7.0,
        0x040 => 8.0,
        0x041 => 9.0,
        0x042 => 10.0,
        0x043 => 11.5,
        0x044 => 12.5,
        0x057 => 13.5,
        0x058 => 14.5,
        0x137 => 15.75,
        0x046 => 16.75,
        0x045 => 17.75,
        // Number row
        0x029 => 0.5,
        0x002..=0x00B => 1.5 + (code - 0x002) as f32,
        0x00C => 11.5,
        0x00D => 12.5,
        0x00E => 14.0,
        0x152 => 15.75,
        0x147 => 16.75,
        0x149 => 17.75,
        0x145 => 19.0,
        0x135 => 20.0,
        0x037 => 21.0,
        0x04A => 22.0,
        // QWERTY row
        0x00F => 0.75,
        0x010..=0x019 => 2.0 + (code - 0x010) as f32,
        0x01A => 12.0,
        0x01B => 13.0,
        0x02B => 14.25,
        0x153 => 15.75,
        0x14F => 16.75,
        0x151 => 17.75,
        0x047 => 19.0,
        0x048 => 20.0,
        0x049 => 21.0,
        0x04E => 22.0,
        // Home row
        0x03A => 0.9,
        0x01E..=0x026 => 2.25 + (code - 0x01E) as f32,
        0x027 => 11.25,
        0x028 => 12.25,
        0x01C => 13.9,
        0x04B => 19.0,
        0x04C => 20.0,
        0x04D => 21.0,
        // Bottom letter row
        0x02A => 1.15,
        0x02C..=0x032 => 2.75 + (code - 0x02C) as f32,
        0x033 => 9.75,
        0x034 => 10.75,
        0x035 => 11.75,
        0x036 => 13.6,
        0x148 => 16.75,
        0x04F => 19.0,
        0x050 => 20.0,
        0x051 => 21.0,
        0x11C => 22.0,
        // Space row
        0x01D => 0.6,
        0x15B => 1.9,
        0x038 => 3.1,
        0x039 => 7.0,
        0x138 => 10.9,
        0x15C => 12.1,
        0x15D => 13.4,
        0x11D => 14.4,
        0x14B => 15.75,
        0x150 => 16.75,
        0x14D => 17.75,
        0x052 => 19.5,
        0x053 => 21.0,
        _ => return None,
    };
    Some(x)
}

/// Stereo position of a key from -1 (far left) to 1 (far right).
/// The main typing block spans most of the field; navigation keys and the
/// number pad sit at the right edge.
pub fn pan(code: u16) -> f32 {
    match position_units(code) {
        Some(x) => ((x - 7.5) / 7.5).clamp(-1.0, 1.0),
        None => 0.0,
    }
}

/// Mechvibes / libuiohook key code for a key code, as used in Mechvibes `config.json`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn to_mechvibes(code: u16) -> u32 {
    let scan = (code & 0xFF) as u32;
    let ext = code & 0x100 != 0;
    if ext {
        match scan {
            0x48 | 0x4B | 0x4D | 0x50 => 0xE000 | scan, // arrow keys
            0x45 => 0x45,                               // Num Lock
            _ => 0x0E00 | scan,
        }
    } else if scan == 0x45 {
        0x0E45 // Pause
    } else {
        scan
    }
}

/// Inverse of [`to_mechvibes`].
pub fn from_mechvibes(mv: u32) -> Option<u16> {
    let c = match mv {
        0xE048 | 0xE04B | 0xE04D | 0xE050 => 0x100 | (mv & 0xFF),
        0x0045 => 0x145,
        0x0E45 => 0x045,
        0x0E00..=0x0EFF => 0x100 | (mv & 0xFF),
        0x0001..=0x00FF => mv,
        _ => return None,
    };
    Some(c as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups() {
        assert_eq!(group(0x039), KeyGroup::Space);
        assert_eq!(group(0x01C), KeyGroup::Enter);
        assert_eq!(group(0x11C), KeyGroup::Enter);
        assert_eq!(group(0x00E), KeyGroup::Backspace);
        assert_eq!(group(0x02A), KeyGroup::Wide);
        assert_eq!(group(0x01E), KeyGroup::Regular);
    }

    #[test]
    fn panning_left_to_right() {
        assert!(pan(0x01E) < -0.5); // A
        assert!(pan(0x026) > 0.2); // L
        assert!(pan(0x039).abs() < 0.1); // Space
        assert_eq!(pan(0x14D), 1.0); // Right arrow
    }

    #[test]
    fn classification() {
        assert!(is_modifier(0x02A));
        assert!(!is_modifier(0x01E));
        assert!(is_numpad(0x04C));
        assert!(is_numpad(0x11C));
        assert!(!is_numpad(0x01C));
        assert!(is_function(0x03B));
        assert!(is_function(0x120)); // volume mute
        assert!(!is_function(0x148)); // up arrow
    }

    #[test]
    fn mechvibes_roundtrip() {
        for code in 1..NUM_CODES as u16 {
            if position_units(code).is_some() {
                let mv = to_mechvibes(code);
                assert_eq!(from_mechvibes(mv), Some(code), "code {code:#x} -> mv {mv:#x}");
            }
        }
        assert_eq!(to_mechvibes(0x039), 57); // space
        assert_eq!(to_mechvibes(0x11C), 3612); // keypad enter
        assert_eq!(to_mechvibes(0x148), 57416); // up arrow
        assert_eq!(to_mechvibes(0x15B), 3675); // left win
    }
}
