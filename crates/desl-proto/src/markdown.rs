//! Sections of a Markdown file (docs/design/2026-11-21-web-forms.md, the `document` widget): a
//! heading and everything under it, up to the next heading of the same or a higher level.

use std::ops::Range;

/// An ATX heading: its level and its text.
struct Heading<'a> {
    at: usize,
    level: usize,
    text: &'a str,
}

/// The headings of `text` outside fenced code, with the byte each line starts at.
fn headings(text: &str) -> Vec<Heading<'_>> {
    let mut out = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let body = line.trim_end_matches(['\n', '\r']);
        let lead = body.len() - body.trim_start_matches(' ').len();
        let t = &body[lead..];
        let mark = t.chars().next().filter(|c| *c == '`' || *c == '~');
        if lead < 4
            && let Some(c) = mark
        {
            let run = t.chars().take_while(|x| *x == c).count();
            if run >= 3 {
                match fence {
                    None => fence = Some((c, run)),
                    Some((f, n)) if f == c && run >= n && t[run..].trim().is_empty() => fence = None,
                    Some(_) => {}
                }
                continue;
            }
        }
        if fence.is_some() || lead >= 4 {
            continue;
        }
        let level = t.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&level) && (t.len() == level || t[level..].starts_with([' ', '\t'])) {
            // The closing #s, as in `## Install ##`, are not part of the text.
            let words = t[level..].trim().trim_end_matches('#').trim_end();
            out.push(Heading { at: start, level, text: words });
        }
    }
    out
}

/// The bytes of `text` the section under `heading` takes: the heading's line and everything up
/// to the next heading of the same or a higher level. `heading` is the heading's text, with or
/// without its `#`s; None is the whole text. A sentence when it is missing or there twice.
pub fn section(text: &str, heading: Option<&str>) -> Result<Range<usize>, String> {
    let Some(heading) = heading else { return Ok(0..text.len()) };
    let want = heading.trim().trim_start_matches('#').trim();
    let all = headings(text);
    let found: Vec<usize> = all.iter().enumerate().filter(|(_, h)| h.text == want).map(|(i, _)| i).collect();
    match found.as_slice() {
        [i] => {
            let h = &all[*i];
            let end = all[i + 1..].iter().find(|n| n.level <= h.level).map_or(text.len(), |n| n.at);
            Ok(h.at..end)
        }
        [] => {
            let names: Vec<String> = all.iter().take(12).map(|h| format!("\"{}\"", h.text)).collect();
            let there = if names.is_empty() { "It has no headings.".to_string() } else { format!("Its headings: {}.", names.join(", ")) };
            Err(format!("The file has no heading \"{want}\". {there}"))
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
    fn a_heading_missing_or_twice_is_refused() {
        let err = cut(Some("Uninstall")).unwrap_err();
        assert!(err.contains("no heading \"Uninstall\""), "{err}");
        assert!(err.contains("Install") && err.contains("Use"), "it names the headings there are: {err}");
        let twice = "## A\n\nx\n\n## A\n\ny\n";
        assert!(section(twice, Some("A")).unwrap_err().contains("twice"));
        assert!(section(DOC, Some("not a heading")).is_err(), "inside a fence");
    }
}
