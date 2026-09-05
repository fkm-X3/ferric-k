//! Streaming PS/2 scancode set-1 decoder: feeds raw bytes from the keyboard
//! and yields press/release [`KeyEvent`]s using a US layout. Stateful because
//! single make codes are split by 0xE0 prefixes and the pause sequence spans
//! several bytes. Host-testable; no hardware knowledge beyond the byte stream.

use ferric_api::{Key, KeyEvent};

/// Unshifted US-layout character for each set-1 make code; 0 = not printable.
const UNSHIFTED: [u8; 128] = {
    let mut t = [0u8; 128];
    t[0x02] = b'1';
    t[0x03] = b'2';
    t[0x04] = b'3';
    t[0x05] = b'4';
    t[0x06] = b'5';
    t[0x07] = b'6';
    t[0x08] = b'7';
    t[0x09] = b'8';
    t[0x0A] = b'9';
    t[0x0B] = b'0';
    t[0x0C] = b'-';
    t[0x0D] = b'=';
    t[0x10] = b'q';
    t[0x11] = b'w';
    t[0x12] = b'e';
    t[0x13] = b'r';
    t[0x14] = b't';
    t[0x15] = b'y';
    t[0x16] = b'u';
    t[0x17] = b'i';
    t[0x18] = b'o';
    t[0x19] = b'p';
    t[0x1A] = b'[';
    t[0x1B] = b']';
    t[0x1E] = b'a';
    t[0x1F] = b's';
    t[0x20] = b'd';
    t[0x21] = b'f';
    t[0x22] = b'g';
    t[0x23] = b'h';
    t[0x24] = b'j';
    t[0x25] = b'k';
    t[0x26] = b'l';
    t[0x27] = b';';
    t[0x28] = b'\'';
    t[0x29] = b'`';
    t[0x2B] = b'\\';
    t[0x2C] = b'z';
    t[0x2D] = b'x';
    t[0x2E] = b'c';
    t[0x2F] = b'v';
    t[0x30] = b'b';
    t[0x31] = b'n';
    t[0x32] = b'm';
    t[0x33] = b',';
    t[0x34] = b'.';
    t[0x35] = b'/';
    t[0x39] = b' ';
    t
};

/// Shifted US-layout character for each set-1 make code (letters unused;
/// letter case is derived from lowercase + shift/caps logic).
const SHIFTED: [u8; 128] = {
    let mut t = [0u8; 128];
    t[0x02] = b'!';
    t[0x03] = b'@';
    t[0x04] = b'#';
    t[0x05] = b'$';
    t[0x06] = b'%';
    t[0x07] = b'^';
    t[0x08] = b'&';
    t[0x09] = b'*';
    t[0x0A] = b'(';
    t[0x0B] = b')';
    t[0x0C] = b'_';
    t[0x0D] = b'+';
    t[0x10] = b'Q';
    t[0x11] = b'W';
    t[0x12] = b'E';
    t[0x13] = b'R';
    t[0x14] = b'T';
    t[0x15] = b'Y';
    t[0x16] = b'U';
    t[0x17] = b'I';
    t[0x18] = b'O';
    t[0x19] = b'P';
    t[0x1A] = b'{';
    t[0x1B] = b'}';
    t[0x1E] = b'A';
    t[0x1F] = b'S';
    t[0x20] = b'D';
    t[0x21] = b'F';
    t[0x22] = b'G';
    t[0x23] = b'H';
    t[0x24] = b'J';
    t[0x25] = b'K';
    t[0x26] = b'L';
    t[0x27] = b':';
    t[0x28] = b'"';
    t[0x29] = b'~';
    t[0x2B] = b'|';
    t[0x2C] = b'Z';
    t[0x2D] = b'X';
    t[0x2E] = b'C';
    t[0x2F] = b'V';
    t[0x30] = b'B';
    t[0x31] = b'N';
    t[0x32] = b'M';
    t[0x33] = b'<';
    t[0x34] = b'>';
    t[0x35] = b'?';
    t[0x39] = b' ';
    t
};

/// Non-character keys for a make code; scancode set 1, US layout.
const fn control_key(extended: bool, make: u8) -> Option<Key> {
    match (extended, make) {
        (false, 0x0E) => Some(Key::Backspace),
        (false, 0x0F) => Some(Key::Tab),
        (false, 0x1C) => Some(Key::Enter),
        (false, 0x01) => Some(Key::Escape),
        (false, 0x2A) => Some(Key::LeftShift),
        (false, 0x36) => Some(Key::RightShift),
        (false, 0x1D) => Some(Key::LeftControl),
        (false, 0x38) => Some(Key::LeftAlt),
        (false, 0x3A) => Some(Key::CapsLock),
        (false, 0x3B..=0x44) => Some(Key::F(make - 0x3B + 1)),
        (false, 0x57) => Some(Key::F(11)),
        (false, 0x58) => Some(Key::F(12)),
        (false, 0x47) => Some(Key::Home),
        (false, 0x48) => Some(Key::Up),
        (false, 0x49) => Some(Key::PageUp),
        (false, 0x4B) => Some(Key::Left),
        (false, 0x4D) => Some(Key::Right),
        (false, 0x4F) => Some(Key::End),
        (false, 0x50) => Some(Key::Down),
        (false, 0x51) => Some(Key::PageDown),
        (false, 0x52) => Some(Key::Insert),
        (false, 0x53) => Some(Key::Delete),
        (true, 0x1D) => Some(Key::RightControl),
        (true, 0x38) => Some(Key::RightAlt),
        (true, 0x47) => Some(Key::Home),
        (true, 0x48) => Some(Key::Up),
        (true, 0x49) => Some(Key::PageUp),
        (true, 0x4B) => Some(Key::Left),
        (true, 0x4D) => Some(Key::Right),
        (true, 0x4F) => Some(Key::End),
        (true, 0x50) => Some(Key::Down),
        (true, 0x51) => Some(Key::PageDown),
        (true, 0x52) => Some(Key::Insert),
        (true, 0x53) => Some(Key::Delete),
        _ => None,
    }
}

/// Decodes make/break scancode bytes into logical keys.
pub struct ScancodeDecoder {
    extended: bool,
    pause: bool,
    left_shift: bool,
    right_shift: bool,
    caps_lock: bool,
}

impl Default for ScancodeDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ScancodeDecoder {
    /// A decoder starting in the idle state (no prefix, no modifiers held).
    pub const fn new() -> Self {
        Self {
            extended: false,
            pause: false,
            left_shift: false,
            right_shift: false,
            caps_lock: false,
        }
    }

    fn shift_active(&self) -> bool {
        self.left_shift || self.right_shift
    }

    /// Maps a make code to a key, applying shift/caps to printable codes.
    fn map(&self, extended: bool, make: u8) -> Key {
        if let Some(key) = control_key(extended, make) {
            return key;
        }
        let base = UNSHIFTED[usize::from(make)];
        if base != 0 {
            let shifted = self.shift_active();
            let ch = if base.is_ascii_lowercase() {
                if self.caps_lock ^ shifted {
                    base.to_ascii_uppercase()
                } else {
                    base
                }
            } else if shifted {
                SHIFTED[usize::from(make)]
            } else {
                base
            };
            return Key::Char(ch as char);
        }
        Key::Unknown(make)
    }

    /// Feeds one scancode byte; `None` while an event is still partial
    /// (prefix byte received) or when the byte belongs to the pause sequence.
    pub fn push(&mut self, byte: u8) -> Option<KeyEvent> {
        if self.pause {
            if byte == 0xC5 {
                self.pause = false;
            }
            return None;
        }
        if byte == 0xE0 {
            self.extended = true;
            return None;
        }
        if byte == 0xE1 {
            self.pause = true;
            return None;
        }

        let extended = self.extended;
        let event = if byte & 0x80 != 0 {
            KeyEvent::Release(self.map(extended, byte & 0x7F))
        } else {
            KeyEvent::Press(self.map(extended, byte))
        };
        self.extended = false;
        match event {
            KeyEvent::Press(key) => match key {
                Key::LeftShift => self.left_shift = true,
                Key::RightShift => self.right_shift = true,
                Key::CapsLock => self.caps_lock = !self.caps_lock,
                _ => {}
            },
            KeyEvent::Release(key) => match key {
                Key::LeftShift => self.left_shift = false,
                Key::RightShift => self.right_shift = false,
                _ => {}
            },
        }
        Some(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(decoder: &mut ScancodeDecoder, bytes: &[u8]) -> Vec<KeyEvent> {
        bytes.iter().filter_map(|b| decoder.push(*b)).collect()
    }

    #[test]
    fn make_and_break_of_a_letter() {
        let mut d = ScancodeDecoder::new();
        assert_eq!(feed(&mut d, &[0x1E]), vec![KeyEvent::Press(Key::Char('a'))]);
        assert_eq!(
            feed(&mut d, &[0x9E]),
            vec![KeyEvent::Release(Key::Char('a'))]
        );
    }

    #[test]
    fn space_enter_and_backspace() {
        let mut d = ScancodeDecoder::new();
        assert_eq!(
            feed(&mut d, &[0x39, 0x1C, 0x0E, 0x0F]),
            vec![
                KeyEvent::Press(Key::Char(' ')),
                KeyEvent::Press(Key::Enter),
                KeyEvent::Press(Key::Backspace),
                KeyEvent::Press(Key::Tab),
            ]
        );
    }

    #[test]
    fn extended_prefix_tracks_arrows() {
        let mut d = ScancodeDecoder::new();
        assert_eq!(feed(&mut d, &[0xE0, 0x48]), vec![KeyEvent::Press(Key::Up)],);
        assert_eq!(
            feed(&mut d, &[0xE0, 0xC8]),
            vec![KeyEvent::Release(Key::Up)],
        );
        // A bare code after the prefix stays a plain key.
        assert_eq!(
            feed(&mut d, &[0xE0, 0x1D]),
            vec![KeyEvent::Press(Key::RightControl)],
        );
    }

    #[test]
    fn shift_produces_upper_case_and_punctuation() {
        let mut d = ScancodeDecoder::new();
        assert_eq!(feed(&mut d, &[0x2A]), vec![KeyEvent::Press(Key::LeftShift)]);
        assert_eq!(
            feed(&mut d, &[0x1E, 0x02]),
            vec![
                KeyEvent::Press(Key::Char('A')),
                KeyEvent::Press(Key::Char('!')),
            ]
        );
        assert_eq!(
            feed(&mut d, &[0xAA]),
            vec![KeyEvent::Release(Key::LeftShift)]
        );
        assert_eq!(feed(&mut d, &[0x1E]), vec![KeyEvent::Press(Key::Char('a'))]);
    }

    #[test]
    fn right_shift_behaves_like_left_shift() {
        let mut d = ScancodeDecoder::new();
        assert_eq!(
            feed(&mut d, &[0x36]),
            vec![KeyEvent::Press(Key::RightShift)]
        );
        assert_eq!(feed(&mut d, &[0x0C]), vec![KeyEvent::Press(Key::Char('_'))]);
        assert_eq!(
            feed(&mut d, &[0xB6]),
            vec![KeyEvent::Release(Key::RightShift)]
        );
        assert_eq!(feed(&mut d, &[0x0C]), vec![KeyEvent::Press(Key::Char('-'))]);
    }

    #[test]
    fn caps_lock_uppercases_letters_but_not_punctuation() {
        let mut d = ScancodeDecoder::new();
        feed(&mut d, &[0x3A]); // caps on
        assert_eq!(
            feed(&mut d, &[0x1E, 0x02, 0x10]),
            vec![
                KeyEvent::Press(Key::Char('A')),
                KeyEvent::Press(Key::Char('1')),
                KeyEvent::Press(Key::Char('Q')),
            ]
        );
        // Caps + shift cancels out for letters, so they come out lowercase.
        assert_eq!(
            feed(&mut d, &[0x2A, 0x1E, 0xAA]),
            vec![
                KeyEvent::Press(Key::LeftShift),
                KeyEvent::Press(Key::Char('a')),
                KeyEvent::Release(Key::LeftShift),
            ]
        );
        feed(&mut d, &[0x3A]); // caps off (make toggles; caps has no break)
        assert_eq!(feed(&mut d, &[0x1E]), vec![KeyEvent::Press(Key::Char('a'))]);
    }

    #[test]
    fn pause_sequence_is_swallowed() {
        let mut d = ScancodeDecoder::new();
        assert_eq!(feed(&mut d, &[0xE1, 0x1D, 0x45, 0xE1, 0x9D, 0xC5]), vec![]);
        assert_eq!(feed(&mut d, &[0x1E]), vec![KeyEvent::Press(Key::Char('a'))]);
    }

    #[test]
    fn unassigned_codes_surface_as_unknown() {
        let mut d = ScancodeDecoder::new();
        assert_eq!(
            feed(&mut d, &[0x7F]),
            vec![KeyEvent::Press(Key::Unknown(0x7F))]
        );
    }

    #[test]
    fn function_keys_map_to_f() {
        let mut d = ScancodeDecoder::new();
        assert_eq!(
            feed(&mut d, &[0x3B, 0x44, 0x57, 0x58]),
            vec![
                KeyEvent::Press(Key::F(1)),
                KeyEvent::Press(Key::F(10)),
                KeyEvent::Press(Key::F(11)),
                KeyEvent::Press(Key::F(12)),
            ]
        );
    }
}
