//! Maps one byte read from a serial terminal (PL011 RX) to a [`KeyEvent`],
//! mirroring canonical ANSI/xterm input conventions.

use ferric_api::{Key, KeyEvent};

/// Maps a terminal byte to an input event; `None` for control bytes the
/// kernel does not consume yet (TC, LF is always Enter for a line input).
pub fn terminal_byte_to_key(byte: u8) -> Option<KeyEvent> {
    let key = match byte {
        b'\r' | b'\n' => Key::Enter,
        0x08 | 0x7F => Key::Backspace,
        b'\t' => Key::Tab,
        0x1B => Key::Escape,
        0x20..=0x7E => Key::Char(char::from(byte)),
        _ => return None,
    };
    Some(KeyEvent::Press(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_bytes_map_to_char_keys() {
        assert_eq!(
            terminal_byte_to_key(b'a'),
            Some(KeyEvent::Press(Key::Char('a')))
        );
        assert_eq!(
            terminal_byte_to_key(b' '),
            Some(KeyEvent::Press(Key::Char(' ')))
        );
        assert_eq!(
            terminal_byte_to_key(b'Z'),
            Some(KeyEvent::Press(Key::Char('Z')))
        );
    }

    #[test]
    fn carriage_return_and_newline_mean_enter() {
        assert_eq!(
            terminal_byte_to_key(b'\r'),
            Some(KeyEvent::Press(Key::Enter))
        );
        assert_eq!(
            terminal_byte_to_key(b'\n'),
            Some(KeyEvent::Press(Key::Enter))
        );
    }

    #[test]
    fn backspace_variants_delete_and_tab() {
        assert_eq!(
            terminal_byte_to_key(0x08),
            Some(KeyEvent::Press(Key::Backspace))
        );
        assert_eq!(
            terminal_byte_to_key(0x7F),
            Some(KeyEvent::Press(Key::Backspace))
        );
        assert_eq!(terminal_byte_to_key(b'\t'), Some(KeyEvent::Press(Key::Tab)));
        assert_eq!(
            terminal_byte_to_key(0x1B),
            Some(KeyEvent::Press(Key::Escape))
        );
    }

    #[test]
    fn unassigned_control_bytes_are_ignored() {
        assert_eq!(terminal_byte_to_key(0x01), None);
        assert_eq!(terminal_byte_to_key(0x03), None);
        assert_eq!(terminal_byte_to_key(0x10), None);
        assert_eq!(terminal_byte_to_key(0x16), None);
    }
}
