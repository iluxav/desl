//! Escape sequences an agent sends to ask the terminal something (SPEC.md "PTY host"). While no
//! client shows an agent the daemon answers them, as the user's terminal answered the client; when
//! an agent's output is replayed onto a terminal they are left out, so the terminal does not
//! answer a second time into the agent's input.

pub enum Query {
    CursorPosition,
    PrimaryAttributes,
    SecondaryAttributes,
    Color(u8),
    KittyKeyboard,
    Other,
}

/// Every escape sequence in `b`: (start, end, query if it is one).
pub fn scan(b: &[u8]) -> Vec<(usize, usize, Option<Query>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != 0x1b || i + 1 >= b.len() {
            i += 1;
            continue;
        }
        let start = i;
        match b[i + 1] {
            b'[' => {
                let mut j = i + 2;
                while j < b.len() && (0x30..=0x3f).contains(&b[j]) {
                    j += 1;
                }
                let params = &b[i + 2..j];
                let inter_start = j;
                while j < b.len() && (0x20..=0x2f).contains(&b[j]) {
                    j += 1;
                }
                let inter = &b[inter_start..j];
                if j >= b.len() {
                    break;
                }
                let fin = b[j];
                let q = match (fin, params, inter) {
                    (b'n', b"6" | b"?6", _) => Some(Query::CursorPosition),
                    (b'n', b"5", _) => Some(Query::Other),
                    (b'c', p, []) if p.is_empty() || p == b"0" => Some(Query::PrimaryAttributes),
                    (b'c', p, []) if p.starts_with(b">") => Some(Query::SecondaryAttributes),
                    (b'c', p, []) if p.starts_with(b"=") => Some(Query::Other),
                    (b'u', b"?", []) => Some(Query::KittyKeyboard),
                    (b'q', p, []) if p.starts_with(b">") => Some(Query::Other),
                    (b'p', _, b"$") => Some(Query::Other),
                    (b't', b"14" | b"16" | b"18", []) => Some(Query::Other),
                    _ => None,
                };
                out.push((start, j + 1, q));
                i = j + 1;
            }
            b']' | b'P' | b'_' => {
                let mut j = i + 2;
                let mut end = None;
                while j < b.len() {
                    if b[j] == 0x07 {
                        end = Some((j, j + 1));
                        break;
                    }
                    if b[j] == 0x1b && j + 1 < b.len() && b[j + 1] == b'\\' {
                        end = Some((j, j + 2));
                        break;
                    }
                    j += 1;
                }
                let Some((body_end, stop)) = end else { break };
                let body = &b[i + 2..body_end];
                let q = if b[i + 1] == b']' {
                    if body.ends_with(b";?") {
                        match body {
                            b"10;?" => Some(Query::Color(10)),
                            b"11;?" => Some(Query::Color(11)),
                            b"12;?" => Some(Query::Color(12)),
                            _ => Some(Query::Other),
                        }
                    } else {
                        None
                    }
                } else if b[i + 1] == b'_' {
                    // A kitty graphics query (`ESC _ G … a=q … ESC \`) asks; other APCs draw.
                    (body.starts_with(b"G") && body.windows(3).any(|w| w == b"a=q")).then_some(Query::Other)
                } else if body.starts_with(b"+q") || body.starts_with(b"$q") {
                    Some(Query::Other)
                } else {
                    None
                };
                out.push((start, stop, q));
                i = stop;
            }
            _ => i += 2,
        }
    }
    out
}

/// `b` without the queries in it.
pub fn without_queries(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut at = 0;
    for (s, e, q) in scan(b) {
        if q.is_some() {
            out.extend_from_slice(&b[at..s]);
            at = e;
        }
    }
    out.extend_from_slice(&b[at..]);
    out
}

/// `b` without the agent's own window titles (OSC 0, 1 and 2): the title is desl's.
pub fn without_titles(b: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let titles: Vec<(usize, usize)> = scan(b)
        .into_iter()
        .filter(|(s, e, q)| q.is_none() && *e > s + 3 && b[s + 1] == b']' && matches!(&b[s + 2..s + 4], b"0;" | b"1;" | b"2;"))
        .map(|(s, e, _)| (s, e))
        .collect();
    if titles.is_empty() {
        return std::borrow::Cow::Borrowed(b);
    }
    let mut out = Vec::with_capacity(b.len());
    let mut at = 0;
    for (s, e) in titles {
        out.extend_from_slice(&b[at..s]);
        at = e;
    }
    out.extend_from_slice(&b[at..]);
    std::borrow::Cow::Owned(out)
}

/// What the user's terminal said when the client asked it the questions agents ask. Each is the
/// terminal's reply exactly, escape sequence included.
#[derive(Default, Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Answers {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub da1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kitty: Option<String>,
}

pub const PROBE: &[u8] = b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b]12;?\x1b\\\x1b[?u\x1b[c";

impl Answers {
    /// Splits the terminal's replies to [`PROBE`]; true once the closing DA1 reply arrived.
    pub fn take(&mut self, b: &[u8]) -> bool {
        let mut done = false;
        let mut i = 0;
        while i < b.len() {
            if b[i] != 0x1b {
                i += 1;
                continue;
            }
            // Replies look like requests to the scanner: OSC 1x;rgb:..., CSI ? flags u, CSI ? ... c
            let rest = &b[i..];
            let stop = rest
                .iter()
                .enumerate()
                .skip(2)
                .find(|(k, c)| **c == 0x07 || (**c == b'\\' && rest[k - 1] == 0x1b) || (rest[1] == b'[' && (0x40..=0x7e).contains(*c)))
                .map(|(k, _)| k + 1)
                .unwrap_or(rest.len());
            let seq = String::from_utf8_lossy(&rest[..stop]).into_owned();
            if seq.starts_with("\x1b]10;") {
                self.fg = Some(seq);
            } else if seq.starts_with("\x1b]11;") {
                self.bg = Some(seq);
            } else if seq.starts_with("\x1b]12;") {
                self.cursor_color = Some(seq);
            } else if seq.starts_with("\x1b[?") && seq.ends_with('u') {
                self.kitty = Some(seq);
            } else if seq.starts_with("\x1b[?") && seq.ends_with('c') {
                self.da1 = Some(seq);
                done = true;
            }
            i += stop;
        }
        done
    }
}

/// What an agent asked in `chunk`, answered as the user's terminal would; `cursor` is the agent's
/// cursor (row, column, from 0) on its own screen.
pub fn replies(chunk: &[u8], answers: &Answers, cursor: (u16, u16)) -> Vec<u8> {
    let mut out = Vec::new();
    for (_, _, q) in scan(chunk) {
        let reply: Option<String> = match q {
            Some(Query::CursorPosition) => Some(format!("\x1b[{};{}R", cursor.0 + 1, cursor.1 + 1)),
            Some(Query::PrimaryAttributes) => Some(answers.da1.clone().unwrap_or_else(|| "\x1b[?62;22c".to_string())),
            Some(Query::SecondaryAttributes) => Some("\x1b[>1;10;0c".to_string()),
            Some(Query::Color(10)) => answers.fg.clone(),
            Some(Query::Color(11)) => answers.bg.clone(),
            Some(Query::Color(_)) => answers.cursor_color.clone(),
            Some(Query::KittyKeyboard) => answers.kitty.clone(),
            _ => None,
        };
        if let Some(r) = reply {
            out.extend_from_slice(r.as_bytes());
        }
    }
    out
}

/// Puts the terminal back the way an agent may have changed it: main screen, mouse, paste, focus
/// and keyboard modes off, cursor shown, colours reset.
pub const RESET: &[u8] =
    b"\x1b[?1049l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1015l\x1b[?2004l\x1b[?1004l\x1b[<10u\x1b[=0u\x1b[>4;0m\x1b[0m\x1b[?25h";

/// A whole new screen in one synchronized frame: the terminal reset and cleared, scrollback
/// included, then `output` without its queries.
pub fn replace_screen(output: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(output.len() + 128);
    frame.extend_from_slice(b"\x1b[?2026h");
    frame.extend_from_slice(RESET);
    frame.extend_from_slice(b"\x1b[H\x1b[2J\x1b[3J");
    frame.extend_from_slice(&without_titles(&without_queries(output)));
    frame.extend_from_slice(b"\x1b[?2026l");
    frame
}

/// The terminal modes an agent has set that an overlay must put back: whether it draws on the
/// alternate screen, and whether it hid the cursor. Read from what it drew, oldest first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub alternate: bool,
    pub cursor_hidden: bool,
}

impl Modes {
    /// Follows the private-mode sets and resets (`CSI ? n h` / `CSI ? n l`) in `b`.
    pub fn track(&mut self, b: &[u8]) {
        let mut i = 0;
        while i + 2 < b.len() {
            if b[i] != 0x1b || b[i + 1] != b'[' || b[i + 2] != b'?' {
                i += 1;
                continue;
            }
            let mut j = i + 3;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == b';') {
                j += 1;
            }
            if j < b.len() && (b[j] == b'h' || b[j] == b'l') {
                let on = b[j] == b'h';
                for n in std::str::from_utf8(&b[i + 3..j]).unwrap_or("").split(';') {
                    match n {
                        "1049" | "1047" | "47" => self.alternate = on,
                        "25" => self.cursor_hidden = !on,
                        _ => {}
                    }
                }
            }
            i = j;
        }
    }
}

/// The sequences in `b` that set how the terminal sends keys and pastes: the kitty keyboard
/// protocol's push, pop and set (`CSI > u`, `CSI < u`, `CSI = u`), modifyOtherKeys
/// (`CSI > 4 ; n m`), bracketed paste and focus reports. desl draws the agent through its own
/// screen, so these are passed to the real terminal for the agent's keys to arrive as it asked.
pub fn key_modes(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 < b.len() {
        if b[i] != 0x1b || b[i + 1] != b'[' {
            i += 1;
            continue;
        }
        let mut j = i + 2;
        while j < b.len() && (0x20..=0x3f).contains(&b[j]) {
            j += 1;
        }
        if j >= b.len() {
            break;
        }
        let params = &b[i + 2..j];
        let keep = match b[j] {
            b'u' => matches!(params.first(), Some(b'>' | b'<' | b'=')),
            b'm' => params.starts_with(b">4"),
            b'h' | b'l' => params == b"?2004" || params == b"?1004",
            _ => false,
        };
        if keep {
            out.extend_from_slice(&b[i..=j]);
        }
        i = j + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_found_and_dropped_but_drawing_is_kept() {
        let b = b"a\x1b[6nb\x1b]11;?\x1b\\c\x1b[31md\x1b[?u\x1b[c\x1b]0;title\x07e";
        assert_eq!(without_queries(b), b"abc\x1b[31md\x1b]0;title\x07e".to_vec());
    }

    #[test]
    fn graphics_queries_and_titles_are_recognised() {
        assert_eq!(without_queries(b"a\x1b_Gi=31,s=1,a=q;AAAA\x1b\\b\x1b_Gf=24;AAAA\x1b\\"), b"ab\x1b_Gf=24;AAAA\x1b\\".to_vec());
        assert_eq!(&*without_titles(b"x\x1b]0;claude\x07y\x1b]8;;http://a\x1b\\z"), b"xy\x1b]8;;http://a\x1b\\z");
    }

    #[test]
    fn startup_replies_are_kept_for_later() {
        let mut a = Answers::default();
        assert!(a.take(b"\x1b]11;rgb:1a1a/1b1b/2626\x1b\\\x1b[?1u\x1b[?62;22c"));
        assert_eq!(a.bg.as_deref(), Some("\x1b]11;rgb:1a1a/1b1b/2626\x1b\\"));
        assert_eq!(a.kitty.as_deref(), Some("\x1b[?1u"));
    }

    #[test]
    fn modes_follow_the_last_set_or_reset() {
        let mut m = Modes::default();
        m.track(b"x\x1b[?1049h\x1b[?25l\x1b[?2026h");
        assert_eq!(m, Modes { alternate: true, cursor_hidden: true });
        m.track(b"\x1b[?1049;25h");
        assert_eq!(m, Modes { alternate: true, cursor_hidden: false });
        m.track(b"\x1b[?1049l");
        assert!(!m.alternate);
    }

    #[test]
    fn key_modes_are_picked_out_of_drawing() {
        let b = b"\x1b[31mhi\x1b[>1u\x1b[?u\x1b[?2004h\x1b[>4;2m\x1b[2J\x1b[<u";
        assert_eq!(key_modes(b), b"\x1b[>1u\x1b[?2004h\x1b[>4;2m\x1b[<u");
    }
}
