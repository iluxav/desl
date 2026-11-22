//! Sections of a Markdown file (docs/design/2026-11-21-web-forms.md, the `document` widget): a
//! heading and everything under it, up to the next heading of the same or a higher level.

use std::ops::Range;

/// An ATX heading: its level and its text.
struct Heading<'a> {
    at: usize,
    level: usize,
    text: &'a str,
}

/// The headings of `text` outside fenced code, front matter and HTML comments, with the byte
/// each line starts at.
fn headings(text: &str) -> Vec<Heading<'_>> {
    let mut out = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut comment = false;
    let mut front = false;
    let mut at = 0;
    for (n, line) in text.split_inclusive('\n').enumerate() {
        let start = at;
        at += line.len();
        let mut body = line.trim_end_matches(['\n', '\r']);
        if n == 0 {
            body = body.trim_start_matches('\u{feff}');
            // Front matter: a first line of ---, up to the next --- or ...
            if body == "---" {
                front = true;
                continue;
            }
        }
        if front {
            front = !(body == "---" || body == "...");
            continue;
        }
        if comment {
            comment = !body.contains("-->");
            continue;
        }
        let lead = body.len() - body.trim_start_matches(' ').len();
        let t = &body[lead..];
        let mark = t.chars().next().filter(|c| *c == '`' || *c == '~');
        if lead < 4
            && let Some(c) = mark
        {
            let run = t.chars().take_while(|x| *x == c).count();
            // A backtick run with a backtick after it is inline code, not a fence.
            let opens = run >= 3 && !(c == '`' && t[run..].contains('`'));
            if opens || (fence.is_some() && run >= 3) {
                match fence {
                    None => fence = Some((c, run)),
                    Some((f, k)) if f == c && run >= k && t[run..].trim().is_empty() => fence = None,
                    Some(_) => {}
                }
                continue;
            }
        }
        if fence.is_some() || lead >= 4 {
            continue;
        }
        if t.starts_with("<!--") {
            comment = !t.contains("-->");
            continue;
        }
        let level = t.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&level) && (t.len() == level || t[level..].starts_with([' ', '\t'])) {
            out.push(Heading { at: start, level, text: heading_text(&t[level..]) });
        }
    }
    out
}

/// A heading's text: without the closing #s, as in `## Install ##`, which need a space before
/// them, so `## C#` is the heading "C#".
fn heading_text(rest: &str) -> &str {
    let rest = rest.trim();
    let bare = rest.trim_end_matches('#');
    if bare.is_empty() {
        return "";
    }
    if bare.len() < rest.len() && bare.ends_with([' ', '\t']) { bare.trim_end() } else { rest }
}

/// What the agent named: a heading's text, with or without its level's #s and the space after.
fn wanted(heading: &str) -> &str {
    let h = heading.trim();
    let level = h.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&level) && h[level..].starts_with([' ', '\t']) { heading_text(&h[level..]) } else { h }
}

/// The bytes of `text` the section under `heading` takes: the heading's line and everything up
/// to the next heading of the same or a higher level. `heading` is the heading's text, with or
/// without its `#`s; None is the whole text. A sentence when it is missing or there twice.
pub fn section(text: &str, heading: Option<&str>) -> Result<Range<usize>, String> {
    let Some(heading) = heading else { return Ok(0..text.len()) };
    let want = wanted(heading);
    match find(text, Some(heading))? {
        Some(range) => Ok(range),
        None => {
            let all = headings(text);
            let names: Vec<String> = all.iter().take(12).map(|h| format!("\"{}\"", h.text)).collect();
            let there = if names.is_empty() { "It has no headings.".to_string() } else { format!("Its headings: {}.", names.join(", ")) };
            Err(format!("The file has no heading \"{want}\". {there}"))
        }
    }
}

/// As [`section`], with a missing heading as None, and a sentence only for one there twice.
pub fn find(text: &str, heading: Option<&str>) -> Result<Option<Range<usize>>, String> {
    let Some(heading) = heading else { return Ok(Some(0..text.len())) };
    let want = wanted(heading);
    let all = headings(text);
    let found: Vec<usize> = all.iter().enumerate().filter(|(_, h)| h.text == want).map(|(i, _)| i).collect();
    match found.as_slice() {
        [] => Ok(None),
        [i] => {
            let h = &all[*i];
            let end = all[i + 1..].iter().find(|n| n.level <= h.level).map_or(text.len(), |n| n.at);
            Ok(Some(h.at..end))
        }
        _ => Err(format!("The heading \"{want}\" is in the file twice, so desl cannot tell which section you mean.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str =
        "# Title\n\nIntro.\n\n## Install\n\nRun it.\n\n### From source\n\nBuild it.\n\n## Use\n\n```sh\n# not a heading\n```\n\nThen go.\n";

    fn cut(heading: Option<&str>) -> Result<&'static str, String> {
        section(DOC, heading).map(|r| &DOC[r])
    }

    #[test]
    fn a_section_runs_to_the_next_heading_of_its_level_or_higher() {
        assert_eq!(cut(Some("Install")).unwrap(), "## Install\n\nRun it.\n\n### From source\n\nBuild it.\n\n");
        assert_eq!(cut(Some("## Install")).unwrap(), cut(Some("Install")).unwrap(), "with or without its #s");
        assert_eq!(cut(Some("From source")).unwrap(), "### From source\n\nBuild it.\n\n");
        assert_eq!(cut(Some("Use")).unwrap(), "## Use\n\n```sh\n# not a heading\n```\n\nThen go.\n", "a fence hides a heading");
        assert_eq!(cut(Some("Title")).unwrap(), DOC);
        assert_eq!(cut(None).unwrap(), DOC, "no heading: the whole file");
    }

    #[test]
    fn real_files_are_read_as_markdown_reads_them() {
        // A closing run of #s needs a space before it: `## C#` is the heading "C#".
        let sharp = "## C#\n\nx\n\n## Go\n";
        assert_eq!(&sharp[section(sharp, Some("C#")).unwrap()], "## C#\n\nx\n\n");
        // A heading's text may start with #; only the #s and the space before it are its level.
        let hash = "# #1 priority\n\nx\n";
        assert_eq!(&hash[section(hash, Some("#1 priority")).unwrap()], hash);
        // Backticks in what follows a backtick run make it inline code, not a fence.
        let inline = "```x``` is code\n\n## After\n\ny\n";
        assert!(section(inline, Some("After")).is_ok());
        // Front matter and HTML comments hold no headings.
        let front = "---\ntitle: x\n# a yaml comment\n---\n\n# Real\n\n<!--\n# old\n-->\n\n## Old\n";
        assert!(section(front, Some("a yaml comment")).is_err());
        assert!(section(front, Some("old")).is_err());
        assert!(section(front, Some("Old")).is_ok());
        // A byte order mark does not hide the first heading.
        let bom = "\u{feff}# First\n\nx\n";
        assert_eq!(section(bom, Some("First")).unwrap(), 0..bom.len());
    }

    #[test]
    fn a_heading_missing_or_twice_is_refused() {
        let err = cut(Some("Uninstall")).unwrap_err();
        assert!(err.contains("no heading \"Uninstall\""), "{err}");
        assert!(err.contains("Install") && err.contains("Use"), "it names the headings there are: {err}");
        let twice = "## A\n\nx\n\n## A\n\ny\n";
        assert!(section(twice, Some("A")).unwrap_err().contains("twice"));
        assert!(section(DOC, Some("not a heading")).is_err(), "inside a fence");
    }
}
