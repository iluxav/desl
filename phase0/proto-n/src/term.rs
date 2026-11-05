//! Escape sequences an agent sends to ask the terminal something. While an agent is hidden desl
//! answers them itself; when its output is replayed onto the real terminal they are left out, so
//! the terminal does not answer a second time into the now visible agent's input.

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
            b']' | b'P' => {
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

/// What the real terminal said when desl asked it the same questions at startup.
#[derive(Default)]
pub struct Answers {
    pub fg: Option<Vec<u8>>,
    pub bg: Option<Vec<u8>>,
    pub cursor_color: Option<Vec<u8>>,
    pub da1: Option<Vec<u8>>,
    pub kitty: Option<Vec<u8>>,
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
            let seq = rest[..stop].to_vec();
            if seq.starts_with(b"\x1b]10;") {
                self.fg = Some(seq);
            } else if seq.starts_with(b"\x1b]11;") {
                self.bg = Some(seq);
            } else if seq.starts_with(b"\x1b]12;") {
                self.cursor_color = Some(seq);
            } else if seq.starts_with(b"\x1b[?") && seq.ends_with(b"u") {
                self.kitty = Some(seq);
            } else if seq.starts_with(b"\x1b[?") && seq.ends_with(b"c") {
                self.da1 = Some(seq);
                done = true;
            }
            i += stop;
        }
        done
    }
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
    fn startup_replies_are_kept_for_later() {
        let mut a = Answers::default();
        assert!(a.take(b"\x1b]11;rgb:1a1a/1b1b/2626\x1b\\\x1b[?1u\x1b[?62;22c"));
        assert_eq!(a.bg.as_deref(), Some(&b"\x1b]11;rgb:1a1a/1b1b/2626\x1b\\"[..]));
        assert_eq!(a.kitty.as_deref(), Some(&b"\x1b[?1u"[..]));
    }
}
