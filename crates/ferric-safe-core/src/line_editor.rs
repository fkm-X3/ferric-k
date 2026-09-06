//! Single-line editor: accumulates printable characters and classifies each
//! key event into a [`LineAction`] the console can render. Allocation-free,
//! so it is host-testable pure logic.

use core::fmt;
use ferric_api::{Key, KeyEvent};

/// Maximum number of characters a command line can hold.
pub const MAX_LINE_CHARS: usize = 128;

/// What the console should draw for a key event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineAction {
    /// A printable character was appended.
    Inserted,
    /// The last character was removed; the console should erase it.
    Erased,
    /// Enter: the line is complete and should be executed.
    Submitted,
    /// A key the editor does not handle (modifiers, arrows, releases).
    Ignored,
}

/// A fixed-capacity line buffer with backspace support.
pub struct LineEditor {
    line: [char; MAX_LINE_CHARS],
    len: usize,
}

impl LineEditor {
    pub const fn new() -> Self {
        Self {
            line: [' '; MAX_LINE_CHARS],
            len: 0,
        }
    }

    /// Feeds one key event and reports how the console should respond.
    pub fn handle(&mut self, event: KeyEvent) -> LineAction {
        match event {
            KeyEvent::Press(Key::Char(c)) if c.is_ascii_graphic() || c == ' ' => {
                if self.len < MAX_LINE_CHARS {
                    self.line[self.len] = c;
                    self.len += 1;
                    LineAction::Inserted
                } else {
                    LineAction::Ignored
                }
            }
            KeyEvent::Press(Key::Enter) => LineAction::Submitted,
            KeyEvent::Press(Key::Backspace) => {
                if self.len == 0 {
                    LineAction::Ignored
                } else {
                    self.len -= 1;
                    LineAction::Erased
                }
            }
            _ => LineAction::Ignored,
        }
    }

    /// The characters typed so far.
    pub fn line(&self) -> &[char] {
        &self.line[..self.len]
    }

    /// Empties the buffer.
    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl Default for LineEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for LineEditor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0u8; 4];
        for &c in self.line() {
            f.write_str(c.encode_utf8(&mut buf))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: Key) -> KeyEvent {
        KeyEvent::Press(key)
    }

    #[test]
    fn printable_keys_accumulate_into_the_line() {
        let mut editor = LineEditor::new();
        for c in ['h', 'e', 'y'] {
            assert_eq!(editor.handle(press(Key::Char(c))), LineAction::Inserted);
        }
        assert_eq!(editor.line(), &['h', 'e', 'y']);
        assert_eq!(editor.to_string(), "hey");
    }

    #[test]
    fn space_is_editable_but_other_control_chars_are_not() {
        let mut editor = LineEditor::new();
        assert_eq!(editor.handle(press(Key::Char(' '))), LineAction::Inserted);
        assert_eq!(editor.handle(press(Key::Tab)), LineAction::Ignored);
        assert_eq!(editor.handle(press(Key::Escape)), LineAction::Ignored);
        assert_eq!(editor.handle(press(Key::Up)), LineAction::Ignored);
        assert_eq!(editor.handle(press(Key::Char('\t'))), LineAction::Ignored);
    }

    #[test]
    fn key_releases_are_ignored() {
        let mut editor = LineEditor::new();
        assert_eq!(
            editor.handle(KeyEvent::Release(Key::Char('a'))),
            LineAction::Ignored
        );
        assert!(editor.line().is_empty());
    }

    #[test]
    fn backspace_removes_the_last_character() {
        let mut editor = LineEditor::new();
        for c in ['a', 'b', 'c'] {
            editor.handle(press(Key::Char(c)));
        }
        assert_eq!(editor.handle(press(Key::Backspace)), LineAction::Erased);
        assert_eq!(editor.handle(press(Key::Backspace)), LineAction::Erased);
        assert_eq!(editor.line(), &['a']);
        assert_eq!(editor.handle(press(Key::Backspace)), LineAction::Erased);
        assert_eq!(editor.handle(press(Key::Backspace)), LineAction::Ignored);
        assert!(editor.line().is_empty());
    }

    #[test]
    fn enter_submits_without_sending_a_character() {
        let mut editor = LineEditor::new();
        editor.handle(press(Key::Char('x')));
        assert_eq!(editor.handle(press(Key::Enter)), LineAction::Submitted);
        assert_eq!(editor.line(), &['x']);
    }

    #[test]
    fn line_caps_at_max_line_chars() {
        let mut editor = LineEditor::new();
        for _ in 0..MAX_LINE_CHARS {
            assert_eq!(editor.handle(press(Key::Char('a'))), LineAction::Inserted);
        }
        assert_eq!(editor.handle(press(Key::Char('a'))), LineAction::Ignored);
        assert_eq!(editor.line().len(), MAX_LINE_CHARS);
    }

    #[test]
    fn clear_empties_the_buffer() {
        let mut editor = LineEditor::new();
        editor.handle(press(Key::Char('x')));
        editor.clear();
        assert!(editor.line().is_empty());
        assert!(editor.to_string().is_empty());
    }
}
