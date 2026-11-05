//! Word wrapping with display widths, so the transcript knows its own line count.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        let mut lw = 0usize;
        for word in para.split(' ') {
            if word.is_empty() {
                continue;
            }
            let ww = word.width();
            if lw > 0 && lw + 1 + ww <= width {
                line.push(' ');
                line.push_str(word);
                lw += 1 + ww;
            } else if lw == 0 && ww <= width {
                line.push_str(word);
                lw = ww;
            } else {
                if lw > 0 {
                    out.push(std::mem::take(&mut line));
                    lw = 0;
                }
                if ww <= width {
                    line.push_str(word);
                    lw = ww;
                } else {
                    for c in word.chars() {
                        let cw = c.width().unwrap_or(1);
                        if lw + cw > width {
                            out.push(std::mem::take(&mut line));
                            lw = 0;
                        }
                        line.push(c);
                        lw += cw;
                    }
                }
            }
        }
        out.push(line);
    }
    out
}

/// Truncate to `width` columns, keeping the start and the end: `src/auth/…/middleware.ts`.
pub fn truncate_middle(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    if width <= 1 {
        return "…".to_string();
    }
    let keep_end = (width - 1) / 2;
    let keep_start = width - 1 - keep_end;
    let chars: Vec<char> = s.chars().collect();
    let start: String = chars.iter().take(keep_start).collect();
    let end: String = chars[chars.len().saturating_sub(keep_end)..].iter().collect();
    format!("{start}…{end}")
}

pub fn truncate_end(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    if width <= 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(1);
        if w + cw > width - 1 {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// Tabs become spaces up to the next multiple of four columns; a terminal cell cannot hold a tab.
pub fn expand_tabs(s: &str) -> String {
    if !s.contains('\t') {
        return s.to_string();
    }
    let mut out = String::new();
    let mut col = 0;
    for c in s.chars() {
        if c == '\t' {
            let n = 4 - col % 4;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(c);
            col += c.width().unwrap_or(0);
        }
    }
    out
}

/// Wrap a line of code so nothing is cut: at the last space in the second half of the row when
/// there is one, else at the column limit. Indentation and inner spacing are kept as written.
pub fn code_wrap(s: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut line = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > width {
            // Byte offset just past the last space, if it leaves the row at least half full.
            let cut = line.rfind(' ').map(|i| i + 1).filter(|&i| line[..i].width() * 2 >= width && !line[..i].trim().is_empty());
            let rest = cut.map(|i| line.split_off(i)).unwrap_or_default();
            out.push(std::mem::replace(&mut line, rest));
            w = line.width();
        }
        line.push(c);
        w += cw;
    }
    out.push(line);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_wrap_prefers_a_space_and_cuts_nothing() {
        let rows = code_wrap("  const token = header.startsWith(\"Bearer \") ? header.slice(7) : null;", 32);
        assert_eq!(rows.concat(), "  const token = header.startsWith(\"Bearer \") ? header.slice(7) : null;");
        assert!(rows.iter().all(|r| r.width() <= 32));
        assert!(rows[0].ends_with(' '), "{rows:?}");
    }

    #[test]
    fn code_wrap_breaks_a_long_word_at_the_limit() {
        assert_eq!(code_wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn tabs_expand_to_the_next_stop() {
        assert_eq!(expand_tabs("a\tb"), "a   b");
        assert_eq!(expand_tabs("\tx"), "    x");
    }
}
