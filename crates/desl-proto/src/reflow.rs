//! A screen at a new size (SPEC.md "Rendering"). The lines that wrapped at the old width are
//! joined and wrapped again at the new one, history included, as the user's own terminal does;
//! the emulator alone cuts each row short or pads it. The daemon and the client each keep a
//! screen of what a thread drew, and both resize it here, so the two stay alike.

use std::fmt::Write;
use std::num::NonZeroU16;
use vt100::{Color, CursorStyle, Parser, Screen};

fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n.max(1)).unwrap_or(NonZeroU16::MIN)
}

/// How one cell is drawn: all the emulator keeps about it.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct Style {
    fg: Color,
    bg: Color,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

impl Style {
    fn of(cell: &vt100::Cell) -> Style {
        Style {
            fg: cell.fgcolor(),
            bg: cell.bgcolor(),
            bold: cell.bold(),
            dim: cell.dim(),
            italic: cell.italic(),
            underline: cell.underline(),
            inverse: cell.inverse(),
        }
    }

    /// The sequence that sets this style and nothing else.
    fn write(&self, out: &mut String) {
        out.push_str("\x1b[0");
        for (on, code) in [(self.bold, 1), (self.dim, 2), (self.italic, 3), (self.underline, 4), (self.inverse, 7)] {
            if on {
                let _ = write!(out, ";{code}");
            }
        }
        for (color, code) in [(self.fg, 38), (self.bg, 48)] {
            match color {
                Color::Default => {}
                Color::Idx(i) => drop(write!(out, ";{code};5;{i}")),
                Color::Rgb(r, g, b) => drop(write!(out, ";{code};2;{r};{g};{b}")),
            }
        }
        out.push('m');
    }
}

/// Nothing was ever drawn in this cell.
fn blank(cell: &vt100::Cell) -> bool {
    !cell.has_contents() && !cell.is_wide_continuation() && Style::of(cell) == Style::default()
}

/// How many cells of `row` are drawn: one past the last that is not blank.
fn drawn(screen: &Screen, row: u16, width: u16) -> u16 {
    (0..width).rev().find(|&c| screen.cell(row, c).is_some_and(|cell| !blank(cell))).map_or(0, |c| c + 1)
}

/// The main screen and its history as one stream that draws them again on a screen `cols` wide.
struct Rewrapped {
    text: String,
    /// How many rows the stream takes at the new width.
    rows: usize,
    /// Where the cursor belongs, (row, column) from the top of the stream. `None` when it is
    /// where the stream ends, which leaves a wrap that is about to happen as it is.
    cursor: Option<(usize, u16)>,
}

/// `history` is how many rows above the screen go into the stream at most: the newest ones.
fn rewrapped(screen: &mut Screen, cols: u16, history: usize) -> Rewrapped {
    screen.set_scrollback(0);
    let (height, width) = screen.size();
    let (height, width) = (height.get(), width.get());
    let keep = history;
    let history = screen.scrollback_len();
    let (cursor_row, cursor_col) = screen.cursor_position();
    let cursor_at = history + usize::from(cursor_row);
    // The blank rows under the cursor and under what is drawn belong to no line.
    let last_drawn = (0..height).rev().find(|&r| drawn(screen, r, width) > 0).unwrap_or(0);
    let end = history + usize::from(last_drawn.max(cursor_row)) + 1;

    let mut text = String::new();
    let mut style = Style::default();
    // Where the next cell goes at the new width.
    let (mut row, mut col) = (0usize, 0u16);
    let mut cursor = None;
    let mut at = history.saturating_sub(keep);
    while at < end {
        // The screen shows `height` rows of the history at a time, oldest first.
        let offset = history.saturating_sub(at);
        screen.set_scrollback(offset);
        let first = at - (history - offset);
        for r in first..usize::from(height) {
            if at >= end {
                break;
            }
            let r = u16::try_from(r).unwrap_or(u16::MAX);
            let wrapped = screen.row_wrapped(r);
            let n = if wrapped { width } else { drawn(screen, r, width) };
            for c in 0..n {
                let Some(cell) = screen.cell(r, c) else { break };
                let here = at == cursor_at && c == cursor_col;
                if cell.is_wide_continuation() {
                    if here {
                        cursor = Some((row, col.saturating_sub(1)));
                    }
                    continue;
                }
                let w = if cell.is_wide() { 2 } else { 1 };
                if col + w > cols {
                    (row, col) = (row + 1, 0);
                }
                if here {
                    cursor = Some((row, col));
                }
                let s = Style::of(cell);
                if s != style {
                    s.write(&mut text);
                    style = s;
                }
                text.push_str(if cell.has_contents() { cell.contents() } else { " " });
                col += w;
            }
            let last = at + 1 == end;
            if at == cursor_at && cursor_col >= n {
                // Past what the row holds. At the very end of the stream it is already there.
                let past = cursor_col - n;
                if past > 0 || !last {
                    cursor = Some((row, col.saturating_add(past).min(cols.saturating_sub(1))));
                }
            }
            if !wrapped && !last {
                text.push_str("\r\n");
                (row, col) = (row + 1, 0);
            }
            at += 1;
        }
    }
    screen.set_scrollback(0);
    Rewrapped { text, rows: row + 1, cursor }
}

/// What the alternate screen shows, drawn again on a screen of `rows` by `cols`. A program that
/// draws there (an editor, a pager, an agent's full-screen view) draws it afresh for the new
/// size, so its rows are cut, not wrapped.
fn alternate(screen: &Screen, rows: u16, cols: u16) -> String {
    let (_, width) = screen.size();
    let mut out = String::from("\x1b[?1049h");
    for (i, line) in screen.rows_formatted(0, width.get().min(cols)).take(usize::from(rows)).enumerate() {
        let _ = write!(out, "\x1b[m\x1b[{};1H{line}", i + 1);
    }
    let (r, c) = screen.cursor_position();
    let _ = write!(out, "\x1b[m\x1b[{};{}H", r.min(rows - 1) + 1, c.min(cols - 1) + 1);
    out
}

/// Everything `old` shows and has set, as one stream that draws it on an empty screen of `rows`
/// by `cols`: its lines wrapped at that width, history first, then the alternate screen when
/// it is on one, the cursor and the modes. At most `history` rows above the screen are in it.
/// `old` is left on its main screen.
fn redrawn(old: &mut Parser, rows: u16, cols: u16, history: usize) -> String {
    let screen = old.screen();
    let mut state = screen.attributes_formatted();
    state.push_str(&screen.input_mode_formatted());
    if screen.hide_cursor() {
        state.push_str("\x1b[?25l");
    }
    let shape = match screen.cursor_style() {
        CursorStyle::Default => 0,
        CursorStyle::BlinkBlock => 1,
        CursorStyle::SteadyBlock => 2,
        CursorStyle::BlinkUnderline => 3,
        CursorStyle::SteadyUnderline => 4,
        CursorStyle::BlinkVerticalBar => 5,
        CursorStyle::SteadyVerticalBar => 6,
    };
    if shape != 0 {
        let _ = write!(state, "\x1b[{shape} q");
    }
    let over = screen.alternate_screen().then(|| alternate(screen, rows, cols));
    if over.is_some() {
        // The main screen behind it is the one with lines to wrap.
        old.process(b"\x1b[?47l");
    }

    let lines = rewrapped(old.screen_mut(), cols, history);
    let mut out = lines.text;
    out.push_str("\x1b[m");
    if let Some((row, col)) = lines.cursor {
        // Rows that do not fit went to the history, from the top.
        let top = lines.rows.saturating_sub(usize::from(rows));
        let _ = write!(out, "\x1b[{};{}H", row.saturating_sub(top) + 1, col + 1);
    }
    out.push_str(over.as_deref().unwrap_or(""));
    out.push_str(&state);
    out
}

/// Gives `vt` the size `rows` by `cols`. When the width changes, every line of the main screen
/// and its history is wrapped at the new width and the cursor stays on the character it was on.
pub fn resize(vt: &mut Parser, rows: u16, cols: u16) {
    let (rows, cols) = (nz(rows), nz(cols));
    if vt.screen().size().1 == cols {
        // No line wraps differently: rows go to the history or come back from it.
        vt.screen_mut().set_size(rows, cols);
        return;
    }
    // `vt` may be in the middle of an escape sequence, so it keeps its parser and only its
    // screen is taken out and replaced.
    let mut old = Parser::new(NonZeroU16::MIN, NonZeroU16::MIN, 0);
    std::mem::swap(vt.screen_mut(), old.screen_mut());
    let history = old.screen().scrollback_capacity();
    let stream = redrawn(&mut old, rows.get(), cols.get(), usize::MAX);
    let mut new = Parser::new(rows, cols, history);
    new.process(stream.as_bytes());
    std::mem::swap(vt.screen_mut(), new.screen_mut());
}

/// What `vt` shows and has set, as a stream that draws it again on an empty screen of the same
/// size: the screen, the newest `history` rows above it at most, the alternate screen when it
/// is on one, the cursor and the modes.
pub fn snapshot(vt: &Parser, history: usize) -> Vec<u8> {
    let (rows, cols) = vt.screen().size();
    let mut old = Parser::new(NonZeroU16::MIN, NonZeroU16::MIN, 0);
    *old.screen_mut() = vt.screen().clone();
    redrawn(&mut old, rows.get(), cols.get(), history).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(rows: u16, cols: u16, drawn: &[u8]) -> Parser {
        let mut vt = Parser::new(nz(rows), nz(cols), 100);
        vt.process(drawn);
        vt
    }

    /// Every row the screen has, history first, without the blank ones at the end.
    fn all(vt: &mut Parser) -> Vec<String> {
        let (rows, cols) = vt.screen().size();
        let history = vt.screen().scrollback_len();
        let mut out = Vec::new();
        let mut offset = history;
        loop {
            vt.screen_mut().set_scrollback(offset);
            let take = if offset == 0 { usize::from(rows.get()) } else { offset.min(usize::from(rows.get())) };
            out.extend(vt.screen().rows(0, cols.get()).take(take));
            if offset == 0 {
                break;
            }
            offset = offset.saturating_sub(usize::from(rows.get()));
        }
        while out.last().is_some_and(|l| l.is_empty()) {
            out.pop();
        }
        out
    }

    /// The rows on the screen now, without the blank ones at the end.
    fn shown(vt: &Parser) -> Vec<String> {
        let mut out: Vec<String> = vt.screen().rows(0, vt.screen().size().1.get()).collect();
        while out.last().is_some_and(|l| l.is_empty()) {
            out.pop();
        }
        out
    }

    #[test]
    fn a_long_line_wraps_again_at_each_width() {
        let mut vt = screen(6, 20, b"The quick brown fox jumps over the lazy dog\r\nnext\r\n$ ");
        assert_eq!(all(&mut vt), ["The quick brown fox ", "jumps over the lazy ", "dog", "next", "$ "]);
        resize(&mut vt, 6, 11);
        assert_eq!(all(&mut vt), ["The quick b", "rown fox ju", "mps over th", "e lazy dog", "next", "$ "]);
        assert_eq!(vt.screen().cursor_position(), (5, 2), "the cursor is still after the prompt");
        resize(&mut vt, 6, 50);
        assert_eq!(all(&mut vt), ["The quick brown fox jumps over the lazy dog", "next", "$ "]);
        assert_eq!(vt.screen().cursor_position(), (2, 2));
        vt.process(b"ls");
        assert_eq!(all(&mut vt)[2], "$ ls", "what is drawn next lands after the prompt");
    }

    #[test]
    fn lines_that_no_longer_fit_go_to_the_history_and_come_back() {
        let mut vt = screen(4, 20, b"one one one one one\r\ntwo two two two two\r\nthree\r\n$ ");
        resize(&mut vt, 4, 10);
        assert_eq!(shown(&vt), ["two two tw", "o two two", "three", "$ "]);
        assert_eq!(vt.screen().scrollback_len(), 2);
        assert_eq!(vt.screen().cursor_position(), (3, 2));
        resize(&mut vt, 4, 20);
        assert_eq!(shown(&vt), ["one one one one one", "two two two two two", "three", "$ "]);
        assert_eq!(vt.screen().scrollback_len(), 0);
    }

    #[test]
    fn a_prompt_at_the_top_stays_at_the_top() {
        let mut vt = screen(10, 40, b"$ ");
        resize(&mut vt, 10, 15);
        assert_eq!(vt.screen().cursor_position(), (0, 2));
        assert_eq!(vt.screen().scrollback_len(), 0, "the blank rows under it are not lines");
    }

    #[test]
    fn colours_and_weights_are_kept() {
        let mut vt = screen(4, 20, b"\x1b[1;31mred and bold\x1b[m plain \x1b[4;38;5;40;48;2;1;2;3mdecked out\x1b[m end");
        resize(&mut vt, 6, 9);
        let s = vt.screen();
        let c = s.cell(0, 0).unwrap();
        assert!(c.bold() && c.fgcolor() == Color::Idx(1) && c.bgcolor() == Color::Default);
        let plain = s.cell(1, 5).unwrap();
        assert_eq!((plain.contents(), plain.bold(), plain.fgcolor()), ("l", false, Color::Default));
        let decked = s.cell(2, 2).unwrap();
        assert_eq!(decked.contents(), "e");
        assert!(decked.underline() && !decked.bold());
        assert_eq!((decked.fgcolor(), decked.bgcolor()), (Color::Idx(40), Color::Rgb(1, 2, 3)));
        assert_eq!(all(&mut vt), ["red and b", "old plain", " decked o", "ut end"]);
    }

    #[test]
    fn what_the_program_set_is_kept() {
        let mut vt = screen(4, 20, b"\x1b[?2004h\x1b[?1h\x1b[?1002h\x1b[?1006h\x1b[?25l\x1b[32mgreen");
        resize(&mut vt, 4, 10);
        let s = vt.screen();
        assert!(s.bracketed_paste() && s.application_cursor() && s.hide_cursor());
        assert_eq!(s.mouse_protocol_mode(), vt100::MouseProtocolMode::ButtonMotion);
        assert_eq!(s.mouse_protocol_encoding(), vt100::MouseProtocolEncoding::Sgr);
        vt.process(b"!");
        assert_eq!(vt.screen().cell(0, 5).unwrap().fgcolor(), Color::Idx(2), "the colour in use is still in use");
    }

    #[test]
    fn a_sequence_cut_in_two_by_the_resize_still_reads() {
        let mut vt = screen(4, 20, b"plain \x1b[3");
        resize(&mut vt, 4, 10);
        vt.process(b"1mred");
        assert_eq!(all(&mut vt), ["plain red"]);
        assert_eq!(vt.screen().cell(0, 6).unwrap().fgcolor(), Color::Idx(1));
    }

    #[test]
    fn the_main_screen_wraps_behind_a_full_screen_program() {
        let mut vt = screen(5, 20, b"a line that is long enough to wrap\r\n$ vi");
        vt.process(b"\x1b[?1049h\x1b[H~ editor\x1b[2;1H~ the second row");
        resize(&mut vt, 5, 12);
        assert!(vt.screen().alternate_screen());
        assert_eq!(shown(&vt), ["~ editor", "~ the second"], "its rows are cut until it draws again");
        vt.process(b"\x1b[?1049l");
        assert_eq!(shown(&vt), ["a line that ", "is long enou", "gh to wrap", "$ vi"]);
        assert_eq!(vt.screen().cursor_position(), (3, 4), "the cursor is back after what was typed");
    }

    #[test]
    fn the_cursor_keeps_its_place_inside_a_line() {
        // A line being edited, with the cursor moved back into it.
        let mut vt = screen(5, 20, b"$ echo one two three four five\x1b[1;8H");
        assert_eq!(vt.screen().cursor_position(), (0, 7));
        resize(&mut vt, 5, 6);
        assert_eq!(vt.screen().cursor_position(), (1, 1), "on the same character: the o of one");
        assert_eq!(vt.screen().cell(1, 1).unwrap().contents(), "o");
        resize(&mut vt, 5, 40);
        assert_eq!(vt.screen().cursor_position(), (0, 7));
    }

    #[test]
    fn a_wide_character_is_not_split() {
        let mut vt = screen(4, 10, "ab漢字cd漢字ef".as_bytes());
        resize(&mut vt, 4, 5);
        assert_eq!(all(&mut vt), ["ab漢", "字cd", "漢字e", "f"]);
        assert_eq!(vt.screen().cursor_position(), (3, 1));
    }

    #[test]
    fn only_a_new_height_moves_rows_and_wraps_nothing() {
        let mut vt = screen(4, 10, b"1\r\n2\r\n3\r\n4\r\n5\r\n$ ");
        resize(&mut vt, 6, 10);
        assert_eq!(shown(&vt), ["1", "2", "3", "4", "5", "$ "]);
        resize(&mut vt, 2, 10);
        assert_eq!(shown(&vt), ["5", "$ "]);
    }
}
