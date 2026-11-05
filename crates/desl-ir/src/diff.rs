//! Line diffs in unified form, and the rows the transcript draws from them.
//! Neutral ground: each adapter turns whatever its agent reports about an edit into this text.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Context,
    Added,
    Removed,
    /// Lines skipped between two hunks.
    Gap,
}

pub struct Line {
    pub kind: Kind,
    /// The line's number in the new file for context and added lines, in the old file for removed ones.
    pub number: Option<usize>,
    pub text: String,
}

/// Above this many table cells the middle of a change is shown as "all removed, all added".
const MAX_CELLS: usize = 4_000_000;

fn ops<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(Kind, &'a str)> {
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf] {
        suf += 1;
    }
    let (ma, mb) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let (n, m) = (ma.len(), mb.len());
    let mut out: Vec<(Kind, &str)> = a[..pre].iter().map(|l| (Kind::Context, *l)).collect();
    if n.saturating_mul(m) > MAX_CELLS {
        out.extend(ma.iter().map(|l| (Kind::Removed, *l)));
        out.extend(mb.iter().map(|l| (Kind::Added, *l)));
    } else {
        // Longest common subsequence, filled from the end so the walk below reads forwards.
        let w = m + 1;
        let mut t = vec![0u32; (n + 1) * w];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                t[i * w + j] = if ma[i] == mb[j] { t[(i + 1) * w + j + 1] + 1 } else { t[(i + 1) * w + j].max(t[i * w + j + 1]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if ma[i] == mb[j] {
                out.push((Kind::Context, ma[i]));
                i += 1;
                j += 1;
            } else if t[(i + 1) * w + j] >= t[i * w + j + 1] {
                out.push((Kind::Removed, ma[i]));
                i += 1;
            } else {
                out.push((Kind::Added, mb[j]));
                j += 1;
            }
        }
        out.extend(ma[i..].iter().map(|l| (Kind::Removed, *l)));
        out.extend(mb[j..].iter().map(|l| (Kind::Added, *l)));
    }
    out.extend(a[a.len() - suf..].iter().map(|l| (Kind::Context, *l)));
    out
}

/// The change from `old` to `new` as a unified diff with `context` unchanged lines around each
/// hunk. Empty when nothing changed.
pub fn unified(old: &str, new: &str, context: usize) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let ops = ops(&a, &b);
    // Hunks as ranges of ops: every change with its context, merged where they touch.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (i, _) in ops.iter().enumerate().filter(|(_, o)| o.0 != Kind::Context) {
        let start = i.saturating_sub(context);
        let end = (i + context + 1).min(ops.len());
        match ranges.last_mut() {
            Some(last) if start <= last.1 => last.1 = end,
            _ => ranges.push((start, end)),
        }
    }
    // The old and new line numbers each op starts at.
    let mut at: Vec<(usize, usize)> = Vec::with_capacity(ops.len());
    let (mut o, mut n) = (1, 1);
    for op in &ops {
        at.push((o, n));
        match op.0 {
            Kind::Context => {
                o += 1;
                n += 1;
            }
            Kind::Removed => o += 1,
            Kind::Added => n += 1,
            Kind::Gap => {}
        }
    }
    let mut out = String::new();
    for (start, end) in ranges {
        let old_len = ops[start..end].iter().filter(|op| op.0 != Kind::Added).count();
        let new_len = ops[start..end].iter().filter(|op| op.0 != Kind::Removed).count();
        let (old_at, new_at) = at[start];
        // By convention an empty side names the line before it.
        let old_at = if old_len == 0 { old_at - 1 } else { old_at };
        let new_at = if new_len == 0 { new_at - 1 } else { new_at };
        out.push_str(&format!("@@ -{old_at},{old_len} +{new_at},{new_len} @@\n"));
        for (kind, text) in &ops[start..end] {
            out.push(match kind {
                Kind::Added => '+',
                Kind::Removed => '-',
                _ => ' ',
            });
            out.push_str(text);
            out.push('\n');
        }
    }
    out
}

/// `@@ -2,6 +2,7 @@` gives (2, 2): where the old and the new side of the hunk start.
fn hunk_start(header: &str) -> Option<(usize, usize)> {
    let mut parts = header.trim_start_matches('@').split_whitespace();
    let first = |s: &str| s[1..].split(',').next().and_then(|n| n.parse::<usize>().ok());
    let old = parts.next().filter(|p| p.starts_with('-')).and_then(first)?;
    let new = parts.next().filter(|p| p.starts_with('+')).and_then(first)?;
    Some((old, new))
}

/// The rows of a unified diff. File headers are dropped; a gap row separates hunks.
pub fn parse(unified: &str) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    let mut at: Option<(usize, usize)> = None;
    for l in unified.lines() {
        if l.starts_with("@@") {
            at = hunk_start(l);
            if !out.is_empty() {
                out.push(Line { kind: Kind::Gap, number: None, text: String::new() });
            }
            continue;
        }
        if l.starts_with("+++") || l.starts_with("---") || l.starts_with("diff ") || l.starts_with("index ") || l.starts_with('\\') {
            continue;
        }
        let (kind, text) = match l.chars().next() {
            Some('+') => (Kind::Added, &l[1..]),
            Some('-') => (Kind::Removed, &l[1..]),
            Some(' ') => (Kind::Context, &l[1..]),
            _ => (Kind::Context, l),
        };
        let number = at.map(|(o, n)| if kind == Kind::Removed { o } else { n });
        if let Some((o, n)) = at.as_mut() {
            match kind {
                Kind::Removed => *o += 1,
                Kind::Added => *n += 1,
                _ => {
                    *o += 1;
                    *n += 1;
                }
            }
        }
        out.push(Line { kind, number, text: text.to_string() });
    }
    out
}

/// How many lines the diff adds and removes.
pub fn stat(unified: &str) -> (usize, usize) {
    let rows = parse(unified);
    (rows.iter().filter(|l| l.kind == Kind::Added).count(), rows.iter().filter(|l| l.kind == Kind::Removed).count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_keeps_context_and_numbers_both_sides() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let new = "a\nb\nc\nD\ne\nf\ng\nh\nI\n";
        let d = unified(old, new, 1);
        assert_eq!(d, "@@ -3,3 +3,3 @@\n c\n-d\n+D\n e\n@@ -8,1 +8,2 @@\n h\n+I\n");
        assert_eq!(stat(&d), (2, 1));
    }

    #[test]
    fn a_new_file_is_all_additions() {
        let d = unified("", "one\ntwo\n", 3);
        assert_eq!(d, "@@ -0,0 +1,2 @@\n+one\n+two\n");
        let rows = parse(&d);
        assert_eq!(rows.iter().map(|l| l.number).collect::<Vec<_>>(), vec![Some(1), Some(2)]);
    }

    #[test]
    fn no_change_is_empty() {
        assert_eq!(unified("same\n", "same\n", 3), "");
    }

    #[test]
    fn parse_numbers_removed_lines_on_the_old_side_and_marks_gaps() {
        let rows = parse("@@ -10,2 +10,2 @@\n keep\n-old\n+new\n@@ -40,1 +40,2 @@\n tail\n+more\n");
        let got: Vec<(Kind, Option<usize>)> = rows.iter().map(|l| (l.kind, l.number)).collect();
        assert_eq!(
            got,
            vec![
                (Kind::Context, Some(10)),
                (Kind::Removed, Some(11)),
                (Kind::Added, Some(11)),
                (Kind::Gap, None),
                (Kind::Context, Some(40)),
                (Kind::Added, Some(41)),
            ]
        );
    }
}
