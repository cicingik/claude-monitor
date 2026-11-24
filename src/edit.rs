//! The prompt panel's line being typed: its text and where the cursor is.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Default, Clone, PartialEq)]
pub struct LineEdit {
    chars: Vec<char>,
    at: usize, // the cursor, before chars[at]; at == chars.len() is the end
}

impl LineEdit {
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }
    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }
    /// Replaces the text, the cursor at its end.
    pub fn set(&mut self, text: &str) {
        self.chars = text.chars().collect();
        self.at = self.chars.len();
    }
    pub fn clear(&mut self) {
        self.set("");
    }
    /// Inserts text at the cursor, which ends up after it; a line break
    /// (\r\n, \r or \n) goes in as one \n.
    pub fn insert(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let n = text.chars().count();
        self.chars.splice(self.at..self.at, text.chars());
        self.at += n;
    }

    /// Applies an editing key; false for a key that does not edit.
    pub fn key(&mut self, k: KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let word = ctrl || k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Char('a') if ctrl => self.at = 0,
            KeyCode::Char('e') if ctrl => self.at = self.chars.len(),
            KeyCode::Char('u') if ctrl => {
                self.chars.drain(..self.at);
                self.at = 0;
            }
            KeyCode::Char('k') if ctrl => self.chars.truncate(self.at),
            KeyCode::Char('w') if ctrl => {
                let from = self.word_start();
                self.chars.drain(from..self.at);
                self.at = from;
            }
            KeyCode::Char(_) if ctrl => return false,
            KeyCode::Char(c) => {
                self.chars.insert(self.at, c);
                self.at += 1;
            }
            KeyCode::Backspace if self.at > 0 => {
                self.at -= 1;
                self.chars.remove(self.at);
            }
            KeyCode::Delete if self.at < self.chars.len() => _ = self.chars.remove(self.at),
            KeyCode::Left if word => self.at = self.word_start(),
            KeyCode::Right if word => self.at = self.word_end(),
            KeyCode::Left => self.at = self.at.saturating_sub(1),
            KeyCode::Right => self.at = (self.at + 1).min(self.chars.len()),
            KeyCode::Home => self.at = 0,
            KeyCode::End => self.at = self.chars.len(),
            KeyCode::Backspace | KeyCode::Delete => {}
            _ => return false,
        }
        true
    }

    /// Where the word before the cursor starts, spaces before it skipped.
    fn word_start(&self) -> usize {
        let mut i = self.at;
        while i > 0 && self.chars[i - 1] == ' ' {
            i -= 1;
        }
        while i > 0 && self.chars[i - 1] != ' ' {
            i -= 1;
        }
        i
    }
    /// Where the word after the cursor ends, spaces before it skipped.
    fn word_end(&self) -> usize {
        let mut i = self.at;
        while i < self.chars.len() && self.chars[i] == ' ' {
            i += 1;
        }
        while i < self.chars.len() && self.chars[i] != ' ' {
            i += 1;
        }
        i
    }

    /// The text cut into rows of w chars, a line break shown as ↵, and the
    /// row and column of the cursor.
    pub fn rows(&self, w: usize) -> (Vec<String>, usize, usize) {
        let w = w.max(1);
        let mut rows: Vec<String> = self.chars.chunks(w).map(|c| c.iter().map(|&c| if c == '\n' { '↵' } else { c }).collect()).collect();
        let (row, col) = (self.at / w, self.at % w);
        if rows.len() <= row {
            rows.push(String::new()); // the cursor after a full last row starts the next
        }
        (rows, row, col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(e: &mut LineEdit, code: KeyCode, m: KeyModifiers) {
        assert!(e.key(KeyEvent::new(code, m)), "{code:?} edits");
    }
    fn typed(text: &str) -> LineEdit {
        let mut e = LineEdit::default();
        e.set(text);
        e
    }

    /// The cursor moves by char, by word and to either end, and typing,
    /// backspace and delete act where it is.
    #[test]
    fn edits_at_the_cursor() {
        let n = KeyModifiers::NONE;
        let mut e = typed("fix bug");
        press(&mut e, KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(e.at, 4);
        for c in "the ".chars() {
            press(&mut e, KeyCode::Char(c), n);
        }
        assert_eq!(e.text(), "fix the bug");
        press(&mut e, KeyCode::Home, n);
        press(&mut e, KeyCode::Delete, n);
        press(&mut e, KeyCode::Char('F'), n);
        assert_eq!((e.text().as_str(), e.at), ("Fix the bug", 1));
        press(&mut e, KeyCode::Right, KeyModifiers::ALT);
        press(&mut e, KeyCode::Backspace, n);
        assert_eq!(e.text(), "Fi the bug");
        press(&mut e, KeyCode::End, n);
        press(&mut e, KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert_eq!(e.text(), "Fi the ");
        press(&mut e, KeyCode::Left, n);
        press(&mut e, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!((e.text().as_str(), e.at), (" ", 0));
        press(&mut e, KeyCode::Backspace, n);
        assert_eq!(e.text(), " ", "backspace at the start keeps the text");
        assert!(!e.key(KeyEvent::new(KeyCode::Enter, n)));
        assert!(!e.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn rows_wrap_the_text_and_place_the_cursor() {
        assert_eq!(typed("abcdef").rows(4), (vec!["abcd".into(), "ef".into()], 1, 2));
        assert_eq!(typed("abcd").rows(4), (vec!["abcd".into(), String::new()], 1, 0));
        let mut e = typed("abcdef");
        e.key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(e.rows(4).1, 0);
        assert_eq!(LineEdit::default().rows(4), (vec![String::new()], 0, 0));
    }
}
