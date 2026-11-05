//! The prompt editor: a single line that grows to five. Readline keys, nothing invented.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

#[derive(Default)]
pub struct Input {
    text: Vec<char>,
    cursor: usize,
}

impl Input {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn as_string(&self) -> String {
        self.text.iter().collect()
    }

    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            if c == '\r' {
                continue;
            }
            self.insert(c);
        }
    }

    pub fn take(&mut self) -> String {
        let s = self.as_string();
        self.clear();
        s
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.text.remove(self.cursor);
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }

    fn kill_word(&mut self) {
        let mut i = self.cursor;
        while i > 0 && self.text[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !self.text[i - 1].is_whitespace() {
            i -= 1;
        }
        self.text.drain(i..self.cursor);
        self.cursor = i;
    }

    fn kill_to_start(&mut self) {
        self.text.drain(..self.cursor);
        self.cursor = 0;
    }

    fn kill_to_end(&mut self) {
        self.text.truncate(self.cursor);
    }

    fn word_left(&mut self) {
        while self.cursor > 0 && self.text[self.cursor - 1].is_whitespace() {
            self.cursor -= 1;
        }
        while self.cursor > 0 && !self.text[self.cursor - 1].is_whitespace() {
            self.cursor -= 1;
        }
    }

    fn word_right(&mut self) {
        let n = self.text.len();
        while self.cursor < n && !self.text[self.cursor].is_whitespace() {
            self.cursor += 1;
        }
        while self.cursor < n && self.text[self.cursor].is_whitespace() {
            self.cursor += 1;
        }
    }

    /// Editing keys. Returns true when the key was consumed.
    pub fn handle(&mut self, k: &KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match (k.code, ctrl, alt) {
            (KeyCode::Backspace, false, false) => self.backspace(),
            (KeyCode::Char('h'), true, false) => self.backspace(),
            (KeyCode::Backspace, _, true) | (KeyCode::Char('w'), true, false) => self.kill_word(),
            (KeyCode::Delete, false, false) | (KeyCode::Char('d'), true, false) => self.delete(),
            (KeyCode::Left, false, false) | (KeyCode::Char('b'), true, false) => self.cursor = self.cursor.saturating_sub(1),
            (KeyCode::Right, false, false) | (KeyCode::Char('f'), true, false) => self.cursor = (self.cursor + 1).min(self.text.len()),
            (KeyCode::Left, true, false) | (KeyCode::Char('b'), false, true) => self.word_left(),
            (KeyCode::Right, true, false) | (KeyCode::Char('f'), false, true) => self.word_right(),
            (KeyCode::Home, _, _) | (KeyCode::Char('a'), true, false) => self.cursor = 0,
            (KeyCode::End, _, _) | (KeyCode::Char('e'), true, false) => self.cursor = self.text.len(),
            (KeyCode::Char('u'), true, false) => self.kill_to_start(),
            (KeyCode::Char('k'), true, false) => self.kill_to_end(),
            _ => return false,
        }
        true
    }

    /// Visual lines for a box `width` columns wide, and the cursor's (row, col).
    pub fn layout(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(1);
        let mut lines: Vec<String> = Vec::new();
        let mut line = String::new();
        let mut col = 0usize;
        let mut cursor_pos = (0usize, 0usize);
        for (i, &c) in self.text.iter().enumerate() {
            if i == self.cursor {
                cursor_pos = (lines.len(), col);
            }
            if c == '\n' {
                lines.push(std::mem::take(&mut line));
                col = 0;
                continue;
            }
            let w = c.width().unwrap_or(1);
            if col + w > width {
                lines.push(std::mem::take(&mut line));
                col = 0;
                if i == self.cursor {
                    cursor_pos = (lines.len(), 0);
                }
            }
            line.push(c);
            col += w;
        }
        if self.cursor == self.text.len() {
            if col >= width {
                lines.push(std::mem::take(&mut line));
                col = 0;
            }
            cursor_pos = (lines.len(), col);
        }
        lines.push(line);
        (lines, cursor_pos)
    }
}
