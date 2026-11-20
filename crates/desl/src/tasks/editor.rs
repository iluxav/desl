//! A text field for a task's title and description: typing, the arrows, word moves, the shell's
//! ctrl keys, a paste, and lines wrapped at words for the width the field is drawn at.

use unicode_width::UnicodeWidthChar;

#[derive(Clone, Debug)]
pub struct Field {
    lines: Vec<Vec<char>>,
    row: usize,
    /// The cursor, as a character index into its line.
    col: usize,
    multiline: bool,
    /// The column `up` and `down` keep to, across short lines.
    want: Option<usize>,
    /// The first visual row drawn.
    pub scroll: usize,
}

fn width(chars: &[char]) -> usize {
    chars.iter().map(|c| c.width().unwrap_or(0)).sum()
}

/// Where a line breaks at `width`: the character range of each row it takes, a word kept whole
/// when it fits on a row of its own.
fn segments(line: &[char], width_: usize) -> Vec<(usize, usize)> {
    let w = width_.max(1);
    let mut out = Vec::new();
    let (mut start, mut used, mut i) = (0, 0, 0);
    let mut space: Option<usize> = None;
    while i < line.len() {
        let cw = line[i].width().unwrap_or(0);
        if used + cw > w && i > start {
            let cut = match space {
                Some(s) if s >= start => s + 1,
                _ => i,
            };
            out.push((start, cut));
            start = cut;
            used = width(&line[cut..i]);
            space = line[cut..i].iter().rposition(|c| *c == ' ').map(|p| cut + p);
            continue;
        }
        if line[i] == ' ' {
            space = Some(i);
        }
        used += cw;
        i += 1;
    }
    out.push((start, line.len()));
    out
}

/// One row on screen: the line it belongs to and the characters it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Visual {
    row: usize,
    start: usize,
    end: usize,
    last: bool,
}

impl Field {
    pub fn new(text: &str, multiline: bool) -> Field {
        let text = if multiline { text.replace("\r\n", "\n") } else { crate::tasks::list::one_line(text) };
        let mut lines: Vec<Vec<char>> = text.split('\n').map(|l| l.chars().collect()).collect();
        if lines.is_empty() {
            lines.push(Vec::new());
        }
        let row = lines.len() - 1;
        let col = lines[row].len();
        Field { lines, row, col, multiline, want: None, scroll: 0 }
    }

    pub fn text(&self) -> String {
        self.lines.iter().map(|l| l.iter().collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    pub fn is_blank(&self) -> bool {
        self.lines.iter().all(|l| l.iter().all(|c| c.is_whitespace()))
    }

    fn line(&mut self) -> &mut Vec<char> {
        &mut self.lines[self.row]
    }

    pub fn insert(&mut self, c: char) {
        if c == '\n' {
            return self.newline();
        }
        if c.is_control() {
            return;
        }
        let col = self.col;
        self.line().insert(col, c);
        self.col += 1;
        self.want = None;
    }

    pub fn paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n").replace('\t', "    ");
        for c in text.chars() {
            match c {
                '\n' if !self.multiline => self.insert(' '),
                c => self.insert(c),
            }
        }
    }

    pub fn newline(&mut self) {
        if !self.multiline {
            return;
        }
        let col = self.col;
        let rest = self.line().split_off(col);
        self.lines.insert(self.row + 1, rest);
        self.row += 1;
        self.col = 0;
        self.want = None;
    }

    pub fn backspace(&mut self) {
        self.want = None;
        if self.col > 0 {
            self.col -= 1;
            let col = self.col;
            self.line().remove(col);
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].len();
            self.line().extend(line);
        }
    }

    pub fn delete(&mut self) {
        self.want = None;
        if self.col < self.lines[self.row].len() {
            let col = self.col;
            self.line().remove(col);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.line().extend(next);
        }
    }

    pub fn left(&mut self) {
        self.want = None;
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].len();
        }
    }

    pub fn right(&mut self) {
        self.want = None;
        if self.col < self.lines[self.row].len() {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    /// To the start of the word before the cursor, as alt+b does in a shell.
    pub fn word_left(&mut self) {
        self.want = None;
        if self.col == 0 {
            return self.left();
        }
        let l = &self.lines[self.row];
        let mut c = self.col;
        while c > 0 && !l[c - 1].is_alphanumeric() {
            c -= 1;
        }
        while c > 0 && l[c - 1].is_alphanumeric() {
            c -= 1;
        }
        self.col = c;
    }

    pub fn word_right(&mut self) {
        self.want = None;
        let l = &self.lines[self.row];
        if self.col == l.len() {
            return self.right();
        }
        let mut c = self.col;
        while c < l.len() && !l[c].is_alphanumeric() {
            c += 1;
        }
        while c < l.len() && l[c].is_alphanumeric() {
            c += 1;
        }
        self.col = c;
    }

    pub fn home(&mut self) {
        self.want = None;
        self.col = 0;
    }

    pub fn end(&mut self) {
        self.want = None;
        self.col = self.lines[self.row].len();
    }

    /// ctrl+u: the line before the cursor goes.
    pub fn kill_back(&mut self) {
        let col = self.col;
        self.line().drain(..col);
        self.col = 0;
        self.want = None;
    }

    /// ctrl+k: the line after the cursor goes.
    pub fn kill_forward(&mut self) {
        let col = self.col;
        self.line().truncate(col);
        self.want = None;
    }

    /// ctrl+w: the word before the cursor goes.
    pub fn kill_word(&mut self) {
        let end = self.col;
        self.word_left();
        let start = self.col;
        if self.row < self.lines.len() && start <= end && end <= self.lines[self.row].len() {
            self.line().drain(start..end);
        }
    }

    fn visuals(&self, width: usize) -> Vec<Visual> {
        let mut out = Vec::new();
        for (row, line) in self.lines.iter().enumerate() {
            let segs = segments(line, width);
            let n = segs.len();
            out.extend(segs.into_iter().enumerate().map(|(k, (start, end))| Visual { row, start, end, last: k + 1 == n }));
        }
        out
    }

    /// The visual row and column of the cursor.
    fn at(&self, vis: &[Visual]) -> (usize, usize) {
        let i = vis
            .iter()
            .position(|v| v.row == self.row && (self.col < v.end || (v.last && self.col == v.end)))
            .unwrap_or(vis.len().saturating_sub(1));
        let v = vis[i];
        (i, width(&self.lines[v.row][v.start..self.col.max(v.start)]))
    }

    fn go(&mut self, vis: &[Visual], to: usize, x: usize) {
        let v = vis[to];
        let line = &self.lines[v.row];
        // On a row that wraps, its end is where the next row starts.
        let max = if v.last || v.end == v.start { v.end } else { v.end - 1 };
        let (mut c, mut used) = (v.start, 0);
        while c < max {
            let cw = line[c].width().unwrap_or(0);
            if used + cw > x {
                break;
            }
            used += cw;
            c += 1;
        }
        self.row = v.row;
        self.col = c;
    }

    /// One row up on screen. False on the first row: the key belongs to whatever is above.
    pub fn up(&mut self, width: usize) -> bool {
        let vis = self.visuals(width);
        let (i, x) = self.at(&vis);
        if i == 0 {
            return false;
        }
        let x = *self.want.get_or_insert(x);
        self.go(&vis, i - 1, x);
        true
    }

    pub fn down(&mut self, width: usize) -> bool {
        let vis = self.visuals(width);
        let (i, x) = self.at(&vis);
        if i + 1 >= vis.len() {
            return false;
        }
        let x = *self.want.get_or_insert(x);
        self.go(&vis, i + 1, x);
        true
    }

    /// The rows to draw at `width`, and where the cursor is among them. A cursor after a row
    /// that fills the width sits at the start of a row of its own.
    pub fn layout(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let vis = self.visuals(width);
        let mut rows: Vec<String> = vis.iter().map(|v| self.lines[v.row][v.start..v.end].iter().collect()).collect();
        let (mut r, mut x) = self.at(&vis);
        if x >= width.max(1) {
            rows.insert(r + 1, String::new());
            r += 1;
            x = 0;
        }
        (rows, (r, x))
    }

    /// Keeps the cursor's row inside `height` rows from `scroll`.
    pub fn follow(&mut self, width: usize, height: usize) {
        let (_, (r, _)) = self.layout(width);
        let h = height.max(1);
        if r < self.scroll {
            self.scroll = r;
        } else if r >= self.scroll + h {
            self.scroll = r + 1 - h;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(f: &mut Field, s: &str) {
        for c in s.chars() {
            f.insert(c);
        }
    }

    #[test]
    fn lines_wrap_at_words_and_long_words_break() {
        let segs = |s: &str, w| segments(&s.chars().collect::<Vec<_>>(), w);
        assert_eq!(segs("", 10), vec![(0, 0)]);
        assert_eq!(segs("the quick brown fox", 10), vec![(0, 10), (10, 19)], "the quick | brown fox");
        assert_eq!(segs("abcdefghijkl", 5), vec![(0, 5), (5, 10), (10, 12)]);
        assert_eq!(segs("ab 界界界", 4), vec![(0, 3), (3, 5), (5, 6)], "a wide character takes two columns");
    }

    #[test]
    fn the_cursor_moves_over_wrapped_rows_and_keeps_its_column() {
        let mut f = Field::new("", true);
        typed(&mut f, "the quick brown fox\nab\nlast line here");
        let (rows, cursor) = f.layout(10);
        assert_eq!(rows, vec!["the quick ", "brown fox", "ab", "last line ", "here"]);
        assert_eq!(cursor, (4, 4));
        assert!(f.up(10));
        assert_eq!(f.layout(10).1, (3, 4));
        assert!(f.up(10));
        assert_eq!(f.layout(10).1, (2, 2), "a short line holds the cursor at its end");
        assert!(f.up(10));
        assert_eq!(f.layout(10).1, (1, 4), "and the next row gets the column back");
        assert!(f.up(10) && !f.up(10), "the first row is the top");
        f.end();
        assert_eq!(f.layout(10).1, (1, 9));
        assert!(f.down(10) && f.down(10) && f.down(10) && !f.down(10));
    }

    #[test]
    fn editing_joins_splits_and_kills() {
        let mut f = Field::new("hello world", true);
        f.word_left();
        f.newline();
        assert_eq!(f.text(), "hello \nworld");
        f.backspace();
        assert_eq!(f.text(), "hello world");
        f.kill_word();
        assert_eq!(f.text(), "world");
        f.end();
        f.paste("\r\nsecond\tline");
        assert_eq!(f.text(), "world\nsecond    line");
        f.kill_back();
        assert_eq!(f.text(), "world\n");
        f.backspace();
        f.home();
        f.kill_forward();
        assert_eq!(f.text(), "");
        assert!(f.is_blank());
    }

    #[test]
    fn a_title_is_one_line() {
        let mut f = Field::new("a\nb", false);
        assert_eq!(f.text(), "a b");
        f.newline();
        f.paste("c\nd");
        assert_eq!(f.text(), "a bc d");
    }

    #[test]
    fn a_full_row_puts_the_cursor_on_the_next() {
        let mut f = Field::new("", true);
        typed(&mut f, "abcde");
        assert_eq!(f.layout(5), (vec!["abcde".to_string(), String::new()], (1, 0)));
    }
}
