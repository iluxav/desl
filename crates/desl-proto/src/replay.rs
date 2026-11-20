//! What a terminal drew, kept to be drawn again (SPEC.md "Process hygiene"). The daemon keeps
//! each terminal's output for the client that attaches later, and marks in it every size the
//! terminal was given: what was drawn at one size is drawn again at that size and wrapped at the
//! next, as it was while it happened. Past its cap, the oldest part of the output gives way to
//! what it left behind (the screen, the history above it and the modes), so however long a
//! program has run, what is kept still draws the screen it has now.

use crate::reflow;
use crate::term::KeyModes;
use std::num::NonZeroU16;
use vt100::Parser;

/// How many rows above the screen a pane keeps.
pub const SCROLLBACK: usize = 10_000;

/// A mark starts like this: an application program command, which a terminal ignores.
const MARK: &[u8] = b"\x1b_desl;size;";

/// No mark is longer.
const MARK_MAX: usize = MARK.len() + 5 + 1 + 5 + 2;

/// The mark for "from here on the terminal is `rows` by `cols`".
pub fn size(rows: u16, cols: u16) -> Vec<u8> {
    format!("\x1b_desl;size;{rows};{cols}\x1b\\").into_bytes()
}

/// The mark that starts at `b[0]`: (its length, rows, columns).
fn mark_at(b: &[u8]) -> Option<(usize, u16, u16)> {
    let rest = b.strip_prefix(MARK)?;
    let end = rest.iter().take(MARK_MAX).position(|c| *c == 0x1b)?;
    if rest.get(end + 1) != Some(&b'\\') {
        return None;
    }
    let (rows, cols) = std::str::from_utf8(&rest[..end]).ok()?.split_once(';')?;
    Some((MARK.len() + end + 2, rows.parse().ok()?, cols.parse().ok()?))
}

/// Where the mark that `b` ends with starts, when it ends with one.
pub fn last_mark(b: &[u8]) -> Option<usize> {
    let from = b.len().saturating_sub(MARK_MAX);
    (from..b.len()).rev().find(|&i| b[i] == 0x1b && mark_at(&b[i..]).is_some_and(|(len, _, _)| i + len == b.len()))
}

/// Draws `b` on `vt`, giving `vt` each size the terminal had while `b` was drawn.
pub fn play(vt: &mut Parser, b: &[u8]) {
    let mut from = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] == 0x1b
            && let Some((len, rows, cols)) = mark_at(&b[i..])
        {
            vt.process(&b[from..i]);
            reflow::resize(vt, rows, cols);
            i += len;
            from = i;
        } else {
            i += 1;
        }
    }
    vt.process(&b[from..]);
}

/// How far back from its end [`settled`] looks for a place to cut.
const REACH: usize = 64 * 1024;

/// How much of `b`, from its start, is finished drawing: the rest may be the first half of a
/// sequence or of a character whose other half has not been drawn yet. The cut is in front of
/// the last escape that starts a sequence; with no escape in reach it is after the last end of
/// a line, and with none of those either, after the last whole character.
pub fn settled(b: &[u8]) -> usize {
    let from = b.len().saturating_sub(REACH);
    // `ESC \` ends a string that began earlier; every other escape begins something.
    let starts = |i: usize| b[i] == 0x1b && b.get(i + 1) != Some(&b'\\');
    if let Some(i) = (from..b.len()).rev().find(|&i| starts(i)) {
        return i;
    }
    if let Some(i) = (from..b.len()).rev().find(|&i| b[i] == b'\n') {
        return i + 1;
    }
    // At most the last three bytes can be a character that is not all there.
    let lead = (b.len().saturating_sub(3)..b.len()).rev().find(|&i| b[i] & 0xc0 != 0x80);
    match lead {
        Some(i) if b[i] >= 0xc0 && std::str::from_utf8(&b[i..]).is_err() => i,
        _ => b.len(),
    }
}

/// What `b` leaves behind, as a short stream that starts on an empty terminal: the screen, the
/// newest `history` rows above it, and the modes. `rows` by `cols` is the size the terminal had
/// when `b` began, for output that does not say so itself. A stream longer than `most` bytes
/// (rows upon rows of cells each in a colour of its own) keeps fewer rows of history, a quarter
/// as many at each try, down to the screen alone.
pub fn fold(b: &[u8], rows: u16, cols: u16, history: usize, most: usize) -> Vec<u8> {
    let size_of = |n: u16| NonZeroU16::new(n.max(1)).unwrap_or(NonZeroU16::MIN);
    let mut vt = Parser::new(size_of(rows), size_of(cols), history);
    play(&mut vt, b);
    let mut keys = KeyModes::default();
    keys.track(b);
    let (rows, cols) = vt.screen().size();
    let mut keep = history;
    loop {
        let mut out = size(rows.get(), cols.get());
        out.extend_from_slice(&reflow::snapshot(&vt, keep));
        out.extend_from_slice(&keys.bytes());
        if out.len() <= most || keep == 0 {
            return out;
        }
        keep /= 4;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vt100::MouseProtocolMode;

    fn fresh(rows: u16, cols: u16) -> Parser {
        Parser::new(NonZeroU16::new(rows).unwrap(), NonZeroU16::new(cols).unwrap(), SCROLLBACK)
    }

    /// Every row the screen has, history first, without the blank ones at the end.
    fn all(vt: &mut Parser) -> Vec<String> {
        let (rows, cols) = vt.screen().size();
        let mut out = Vec::new();
        let mut offset = vt.screen().scrollback_len();
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

    #[test]
    fn a_mark_draws_nothing_and_gives_the_screen_its_size() {
        let mut vt = fresh(24, 80);
        let mut b = size(5, 20);
        b.extend_from_slice(b"hello");
        play(&mut vt, &b);
        assert_eq!(vt.screen().size(), (NonZeroU16::new(5).unwrap(), NonZeroU16::new(20).unwrap()));
        assert_eq!(vt.screen().contents(), "hello");
        // A terminal that is given the mark itself shows nothing of it.
        let mut raw = fresh(5, 20);
        raw.process(&b);
        assert_eq!(raw.screen().contents(), "hello");
    }

    #[test]
    fn what_was_drawn_at_one_width_is_wrapped_again_at_the_next() {
        let mut b = size(4, 10);
        b.extend_from_slice(b"aaaaaaaaaabbbbbbbbbb\r\n$ ");
        b.extend_from_slice(&size(4, 20));
        b.extend_from_slice(b"ls");
        let mut vt = fresh(4, 20);
        play(&mut vt, &b);
        assert_eq!(all(&mut vt), ["aaaaaaaaaabbbbbbbbbb", "$ ls"]);
        // A program that places its text by column drew it for the width it had.
        let mut b = size(4, 10);
        b.extend_from_slice(b"\x1b[1;15Hx\x1b[2;1H$ ");
        b.extend_from_slice(&size(4, 20));
        let mut vt = fresh(4, 20);
        play(&mut vt, &b);
        assert_eq!(all(&mut vt), ["         x", "$ "], "column 15 of 10 is the last one");
    }

    #[test]
    fn the_last_mark_is_found_only_at_the_very_end() {
        let mut b = b"drawn".to_vec();
        assert_eq!(last_mark(&b), None);
        b.extend_from_slice(&size(30, 100));
        assert_eq!(last_mark(&b), Some(5));
        b.extend_from_slice(b"x");
        assert_eq!(last_mark(&b), None);
        assert_eq!(last_mark(&size(65535, 65535)), Some(0));
    }

    #[test]
    fn a_cut_never_falls_inside_a_sequence_or_a_character() {
        let b = b"one\x1b[31mtwo\x1b]0;title\x1b\\three\x1b[0";
        assert_eq!(settled(b), 27, "in front of the sequence that is not finished");
        assert_eq!(settled(&b[..27]), 11, "the escape that ends the title begins nothing: in front of the title");
        assert_eq!(settled(&b[..9]), 3, "a finished sequence stays whole too");
        assert_eq!(settled(b"plain\nlines\nonly"), 12, "after the end of a line");
        assert_eq!(settled("no line ends, caf\u{e9}".as_bytes()), 19);
        let cut = "no line ends, caf\u{e9}".as_bytes();
        assert_eq!(settled(&cut[..cut.len() - 1]), 17, "half a character waits for its other half");
        assert_eq!(settled(b""), 0);
        // What is cut off, drawn after the rest, draws what the whole would have.
        let whole = b"\x1b[2;3Hhere\x1b[1;1H\x1b[1;31mred";
        for end in 1..whole.len() {
            let at = settled(&whole[..end]);
            let mut vt = fresh(4, 20);
            play(&mut vt, &fold(&whole[..at], 4, 20, SCROLLBACK, usize::MAX));
            vt.process(&whole[at..]);
            let mut straight = fresh(4, 20);
            straight.process(whole);
            let (a, z) = (straight.screen(), vt.screen());
            assert_eq!(z.contents(), a.contents(), "cut at {at} of the first {end}");
            assert_eq!(z.cursor_position(), a.cursor_position(), "cut at {at} of the first {end}");
            assert_eq!(z.cell(0, 0).map(|c| (c.fgcolor(), c.bold())), a.cell(0, 0).map(|c| (c.fgcolor(), c.bold())));
        }
    }

    #[test]
    fn folded_output_draws_the_screen_the_output_drew() {
        // A full-screen program as Claude Code is one: the alternate screen, the mouse, its
        // key modes, then what it draws, all of it set once and long ago.
        let mut b = size(6, 30);
        b.extend_from_slice(b"$ claude\r\n\x1b[?2004h\x1b[?1004h\x1b[>5u\x1b[>4;2m\x1b[?1049h\x1b[2J\x1b[H");
        b.extend_from_slice(b"\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h\x1b[?25l");
        b.extend_from_slice(b"\x1b[1;1Hthe conversation\x1b[3;1H\x1b[1;32mgreen\x1b[m and plain\x1b[6;3Hprompt");
        let mut whole = fresh(6, 30);
        play(&mut whole, &b);

        let folded = fold(&b, 6, 30, SCROLLBACK, usize::MAX);
        assert!(folded.len() < 600, "{} bytes", folded.len());
        let mut vt = fresh(24, 80);
        play(&mut vt, &folded);
        let (a, z) = (whole.screen(), vt.screen());
        assert_eq!(z.size(), a.size());
        assert!(z.alternate_screen());
        assert_eq!(z.mouse_protocol_mode(), MouseProtocolMode::AnyMotion);
        assert_eq!(z.mouse_protocol_encoding(), a.mouse_protocol_encoding());
        assert!(z.hide_cursor() && z.bracketed_paste());
        assert_eq!(z.cursor_position(), a.cursor_position());
        assert_eq!(z.contents_formatted(), a.contents_formatted(), "colours and all");
        let mut keys = KeyModes::default();
        keys.track(&folded);
        assert_eq!(keys.bytes(), b"\x1b[>5u\x1b[>4;2m\x1b[?2004h\x1b[?1004h");

        // What the program draws next lands where it would have.
        let next = b"\x1b[3;1H\x1b[2Kchanged\x1b[?1049l";
        whole.process(next);
        vt.process(next);
        assert_eq!(vt.screen().contents_formatted(), whole.screen().contents_formatted());
        assert_eq!(vt.screen().contents(), "$ claude", "the shell's screen was behind it all along");
    }

    #[test]
    fn a_fold_keeps_the_history_above_the_screen() {
        let mut b = size(4, 20);
        for n in 1..=50 {
            b.extend_from_slice(format!("line {n}\r\n").as_bytes());
        }
        b.extend_from_slice(b"$ ");
        let mut whole = fresh(4, 20);
        play(&mut whole, &b);
        let mut vt = fresh(4, 20);
        play(&mut vt, &fold(&b, 4, 20, SCROLLBACK, usize::MAX));
        assert_eq!(all(&mut vt), all(&mut whole));
        assert_eq!(vt.screen().cursor_position(), whole.screen().cursor_position());

        // With less room, the newest rows of it; with none, the screen alone.
        let forty = fold(&b, 4, 20, 40, usize::MAX).len();
        let less = fold(&b, 4, 20, 40, forty - 1);
        let mut vt = fresh(4, 20);
        play(&mut vt, &less);
        let rows = all(&mut vt);
        assert_eq!(
            (rows.len(), rows[0].as_str(), rows[rows.len() - 2].as_str()),
            (14, "line 38", "line 50"),
            "a quarter of forty rows, and the screen"
        );
        assert_eq!(vt.screen().cursor_position(), whole.screen().cursor_position());
        let mut vt = fresh(4, 20);
        play(&mut vt, &fold(&b, 4, 20, SCROLLBACK, 1));
        assert_eq!(all(&mut vt), ["line 48", "line 49", "line 50", "$ "]);

        // Folded again with what came after, as the daemon does each time the cap is passed.
        let mut again = fold(&b, 4, 20, SCROLLBACK, usize::MAX);
        again.extend_from_slice(b"make\r\nline 51\r\n$ ");
        whole.process(b"make\r\nline 51\r\n$ ");
        let mut vt = fresh(4, 20);
        play(&mut vt, &fold(&again, 4, 20, SCROLLBACK, usize::MAX));
        assert_eq!(all(&mut vt), all(&mut whole));
    }
}
