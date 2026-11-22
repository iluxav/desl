//! A conversation drawn in a pane in place of the agent's screen (SPEC.md "History"): a thread's
//! own, from its first prompt, or one of its subagents'. It is read from the daemon's log of the
//! thread, which holds every prompt, reply and tool call since the thread began, whichever
//! agent ran it and however often that agent compacted its context or was started again. So it
//! shows what the agent's own screen no longer does after a restart.

use crate::native::SKey;
use crate::ui::transcript;
use chrono::{DateTime, Local};
use desl_ir::Event;
use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;

fn local(ms: i64) -> Option<DateTime<Local>> {
    (ms > 0).then(|| DateTime::from_timestamp_millis(ms)).flatten().map(|t| t.with_timezone(&Local))
}

/// A conversation laid out for one width.
pub struct Page {
    pub lines: Vec<Line<'static>>,
    /// The line each prompt starts on and when it was sent (0 when the log does not say),
    /// oldest first.
    prompts: Vec<(usize, i64)>,
    width: usize,
}

impl Page {
    /// `events` as lines `width` wide, with a rule wherever a new day begins. Without `tools`
    /// only what was said is in it: the prompts and the replies.
    pub fn of(events: &[Event], width: usize, tools: bool) -> Page {
        let (m, prompts) = crate::model::conversation(events, tools);
        let laid = transcript::layout(&m.items, width);
        let prompts = prompts.into_iter().map(|(item, at)| (laid.starts.get(item).copied().unwrap_or(0), at)).collect();
        Page { lines: laid.lines, prompts, width }
    }

    /// What the pane says while the daemon has not answered yet.
    fn waiting(width: usize) -> Page {
        Page { lines: vec![Line::from(""), Line::from("   Reading the conversation…")], prompts: Vec::new(), width }
    }

    fn text(&self, line: usize) -> String {
        self.lines[line].spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Line `line` as the pane's columns hold it: one string to a column, empty for the second
    /// column of a wide character.
    fn cells(&self, line: usize) -> Vec<String> {
        let mut cells: Vec<String> = Vec::new();
        for c in self.lines.get(line).map(|_| self.text(line)).unwrap_or_default().chars() {
            match (c.width().unwrap_or(0), cells.last_mut()) {
                // A mark that sits on the character before it.
                (0, Some(last)) => last.push(c),
                (2, _) => cells.extend([c.to_string(), String::new()]),
                _ => cells.push(c.to_string()),
            }
        }
        cells
    }

    /// The first line from `from` on that holds `query`, going down the page or up it, around
    /// its end and back to `from`. Letters match in either case unless `query` has a capital.
    pub fn find(&self, query: &str, from: usize, down: bool) -> Option<usize> {
        let n = self.lines.len();
        if n == 0 || query.is_empty() {
            return None;
        }
        let exact = query.chars().any(char::is_uppercase);
        let needle = if exact { query.to_string() } else { query.to_lowercase() };
        (0..n).map(|k| if down { (from + k) % n } else { (from + n - k % n) % n }).find(|&i| {
            let text = self.text(i);
            if exact { text.contains(&needle) } else { text.to_lowercase().contains(&needle) }
        })
    }

    /// The last prompt that starts above `line`.
    fn prompt_above(&self, line: usize) -> Option<usize> {
        self.prompts.iter().rev().map(|p| p.0).find(|&start| start < line)
    }

    /// The first prompt that starts below `line`.
    fn prompt_below(&self, line: usize) -> Option<usize> {
        self.prompts.iter().map(|p| p.0).find(|&start| start > line)
    }

    /// When the prompt was sent that `line` belongs to: the last one that starts at or above it.
    fn sent(&self, line: usize) -> Option<DateTime<Local>> {
        self.prompts.iter().rev().find(|p| p.0 <= line).or(self.prompts.first()).and_then(|p| local(p.1))
    }
}

/// What a key did to the view.
#[derive(Debug, PartialEq, Eq)]
pub enum After {
    Stay,
    Close,
    /// A sentence for the key line.
    Say(String),
}

/// A conversation being read: its events, the page they make, and where the reader is in it.
pub struct Reading {
    events: Vec<Event>,
    /// The daemon answered: `events` is the conversation, even when it is empty.
    loaded: bool,
    page: Option<Page>,
    /// Lines up from the end; 0 shows the end.
    scroll: usize,
    /// Tool calls are shown, each on a line of its own.
    tools: bool,
    /// `/` or `?` and what is typed after it: the text, and whether the search goes down.
    typing: Option<(String, bool)>,
    /// The last search, for `n` and `N`.
    searched: Option<(String, bool)>,
    /// The line the last search stopped on.
    hit: Option<usize>,
}

impl Reading {
    pub fn new() -> Reading {
        Reading { events: Vec::new(), loaded: false, page: None, scroll: 0, tools: true, typing: None, searched: None, hit: None }
    }

    /// The daemon's answer. Where the reader is stays as it was, counted from the end.
    pub fn load(&mut self, events: Vec<Event>) {
        self.events = events;
        self.loaded = true;
        self.page = None;
    }

    /// The page at `width`, laid out again only when the width or the events changed.
    pub fn page(&mut self, width: usize) -> &Page {
        if self.page.as_ref().is_none_or(|p| p.width != width) {
            self.page = Some(if self.loaded { Page::of(&self.events, width, self.tools) } else { Page::waiting(width) });
        }
        self.page.get_or_insert_with(|| Page::waiting(width))
    }

    /// The lines of the page laid out last.
    pub fn lines(&self) -> &[Line<'static>] {
        self.page.as_ref().map_or(&[], |p| &p.lines)
    }

    fn len(&self) -> usize {
        self.lines().len()
    }

    /// How far up the reader can be on a pane `height` rows tall.
    fn most(&self, height: usize) -> usize {
        self.len().saturating_sub(height)
    }

    /// Lines up from the end, never past the top.
    pub fn scroll(&self, height: usize) -> usize {
        self.scroll.min(self.most(height))
    }

    /// The line at the top of the pane.
    pub fn top(&self, height: usize) -> usize {
        self.most(height) - self.scroll(height)
    }

    /// Puts `line` at the top of the pane, or as near to it as the end allows.
    fn show(&mut self, line: usize, height: usize) {
        self.scroll = self.most(height).saturating_sub(line);
    }

    pub fn typing(&self) -> Option<(&str, bool)> {
        self.typing.as_ref().map(|(text, down)| (text.as_str(), *down))
    }

    pub fn hit(&self) -> Option<usize> {
        self.hit
    }

    pub fn tools(&self) -> bool {
        self.tools
    }

    /// What the pane shows from one of its cells to another, in reading order and both
    /// included, each cell a (row, column): every row without the blanks at its end, and all
    /// of them without the margin they share, so what is copied starts at its own first column
    /// and keeps the shape it has.
    pub fn text_between(&self, from: (u16, u16), to: (u16, u16), height: usize) -> String {
        let Some(page) = &self.page else { return String::new() };
        let top = self.top(height);
        // Each row's text, and the column its first cell is in.
        let mut rows: Vec<(String, usize)> = Vec::new();
        for row in from.0..=to.0 {
            let cells = page.cells(top + usize::from(row));
            let first = if row == from.0 { usize::from(from.1) } else { 0 };
            let last = if row == to.0 { usize::from(to.1) + 1 } else { cells.len() };
            let text: String = cells.iter().take(last).skip(first).map(String::as_str).collect();
            rows.push((text.trim_end().to_string(), first));
        }
        let blanks = |text: &str| text.len() - text.trim_start().len();
        let margin = rows.iter().filter(|(text, _)| !text.is_empty()).map(|(text, first)| first + blanks(text)).min().unwrap_or(0);
        let rows: Vec<&str> =
            rows.iter().map(|(text, first)| text.get(margin.saturating_sub(*first).min(blanks(text))..).unwrap_or_default()).collect();
        rows.join("\n")
    }

    /// The run of non-blank cells around a cell of the pane, for a double click: its first and
    /// last column. None on a blank cell.
    pub fn word(&self, row: u16, col: u16, height: usize) -> Option<(u16, u16)> {
        let cells = self.page.as_ref()?.cells(self.top(height) + usize::from(row));
        // The second column of a wide character belongs to the word the character is in.
        let blank = |c: usize| cells.get(c).is_none_or(|s| s.trim().is_empty() && !(s.is_empty() && c > 0));
        let col = usize::from(col);
        if blank(col) {
            return None;
        }
        let from = (0..=col).rev().take_while(|&c| !blank(c)).last()?;
        let to = (col..cells.len()).take_while(|&c| !blank(c)).last()?;
        Some((u16::try_from(from).ok()?, u16::try_from(to).ok()?))
    }

    /// Where the reader is, for the pane's header: when the prompt at the top of the pane was
    /// sent, then the line at the top and how many there are, as tmux writes its history.
    pub fn place(&self, height: usize) -> String {
        let top = self.top(height);
        let when = self.page.as_ref().and_then(|p| p.sent(top)).map(|t| format!("{} · ", t.format("%a %-d %b %H:%M")));
        format!("{}[{}/{}]", when.unwrap_or_default(), self.scroll(height), self.most(height))
    }

    pub fn wheel(&mut self, up: bool, height: usize) {
        let at = self.scroll(height);
        self.scroll = if up { at + 3 } else { at.saturating_sub(3) };
    }

    /// Searches from where the last search stopped, or from the edge of the pane the search
    /// starts at, and shows what it finds a third of the way down the pane.
    fn search(&mut self, down: bool, height: usize) -> After {
        let Some((query, _)) = self.searched.clone() else {
            return After::Say("Nothing was searched for yet. Press / and type what to find.".to_string());
        };
        let Some(page) = &self.page else { return After::Stay };
        let n = page.lines.len().max(1);
        let from = match self.hit {
            Some(hit) if down => (hit + 1) % n,
            Some(hit) => (hit + n - 1) % n,
            None if down => self.top(height),
            None => (self.top(height) + height).min(n) - 1,
        };
        match page.find(&query, from, down) {
            Some(line) => {
                self.hit = Some(line);
                self.show(line.saturating_sub(height / 3), height);
                After::Stay
            }
            None => {
                self.hit = None;
                After::Say(format!("\"{query}\" is not in this conversation."))
            }
        }
    }

    /// A key while the conversation is on screen, in a pane `height` rows tall.
    pub fn key(&mut self, key: SKey, height: usize) -> After {
        if let Some((text, down)) = &mut self.typing {
            match key {
                SKey::Close => self.typing = None,
                SKey::Back => {
                    if text.pop().is_none() {
                        self.typing = None;
                    }
                }
                SKey::Char(c) if !c.is_control() => text.push(c),
                SKey::Enter => {
                    let (text, down) = (std::mem::take(text), *down);
                    self.typing = None;
                    if !text.is_empty() {
                        self.searched = Some((text, down));
                        self.hit = None;
                        return self.search(down, height);
                    }
                }
                _ => {}
            }
            return After::Stay;
        }
        let at = self.scroll(height);
        let (half, whole) = ((height / 2).max(1), height.saturating_sub(2).max(1));
        match key {
            SKey::Close | SKey::Char('q') => return After::Close,
            SKey::Up | SKey::Char('k') => self.scroll = at + 1,
            SKey::Down | SKey::Char('j') | SKey::Enter => self.scroll = at.saturating_sub(1),
            // ctrl+u and ctrl+d, ctrl+b and ctrl+f, as in less and in tmux's copy mode.
            SKey::Char('\x15') => self.scroll = at + half,
            SKey::Char('\x04') => self.scroll = at.saturating_sub(half),
            SKey::PageUp | SKey::Char('\x02' | 'b') => self.scroll = at + whole,
            SKey::PageDown | SKey::Char('\x06' | ' ') => self.scroll = at.saturating_sub(whole),
            SKey::Char('g') => self.scroll = self.most(height),
            SKey::Char('G') => self.scroll = 0,
            SKey::Char('{') => match self.page.as_ref().and_then(|p| p.prompt_above(self.top(height))) {
                Some(line) => self.show(line, height),
                None => self.scroll = self.most(height),
            },
            SKey::Char('}') => match self.page.as_ref().and_then(|p| p.prompt_below(self.top(height))) {
                Some(line) => self.show(line, height),
                None => self.scroll = 0,
            },
            SKey::Char('/') => self.typing = Some((String::new(), true)),
            SKey::Char('?') => self.typing = Some((String::new(), false)),
            SKey::Char('n') => return self.search(self.searched.as_ref().is_none_or(|s| s.1), height),
            SKey::Char('N') => return self.search(!self.searched.as_ref().is_none_or(|s| s.1), height),
            SKey::Char('t') => {
                // The same place in the conversation, by the prompt on screen, not by the line.
                let prompt = self.page.as_ref().map(|p| p.prompts.iter().filter(|q| q.0 <= self.top(height)).count());
                let width = self.page.as_ref().map_or(0, |p| p.width);
                self.tools = !self.tools;
                self.page = None;
                self.hit = None;
                let start = prompt.and_then(|n| n.checked_sub(1)).and_then(|n| self.page(width).prompts.get(n).map(|q| q.0));
                match start {
                    Some(line) => self.show(line, height),
                    None => self.scroll = self.most(height),
                }
            }
            _ => {}
        }
        self.scroll = self.scroll.min(self.most(height));
        After::Stay
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desl_ir::EventKind;
    use serde_json::json;

    /// Noon on 2026-11-19 and on the day after, local time, in milliseconds.
    fn noon(day: u32) -> i64 {
        use chrono::TimeZone;
        Local.with_ymd_and_hms(2026, 11, day, 12, 0, 0).unwrap().timestamp_millis()
    }

    fn ev(index: u64, timestamp: i64, kind: EventKind) -> Event {
        Event { index, thread_id: "t-7f3a".into(), timestamp, agent: "claude".into(), kind }
    }

    fn said(index: u64, at: i64, text: &str) -> Event {
        ev(index, at, EventKind::UserMessage { text: text.into(), attachments: vec![] })
    }

    fn replied(index: u64, at: i64, text: &str) -> Event {
        ev(index, at, EventKind::AssistantMessage { text: text.into(), model: None })
    }

    /// Two days of a conversation: two prompts on the first, one on the second.
    fn two_days() -> Vec<Event> {
        let (a, b) = (noon(19), noon(20));
        vec![
            said(1, a, "why is the sidebar blank"),
            ev(2, a, EventKind::ToolCall { id: "c1".into(), name: "bash".into(), args: json!({ "command": "cargo test" }) }),
            ev(
                3,
                a,
                EventKind::ToolResult {
                    call_id: "c1".into(),
                    output: "ok".into(),
                    exit_code: Some(0),
                    is_error: false,
                    truncated: false,
                    blob: None,
                },
            ),
            replied(4, a, "The Sidebar read the wrong column."),
            said(5, a + 60_000, "fix it"),
            replied(6, a + 90_000, "Fixed."),
            said(7, b, "now the header"),
            replied(8, b, "Done: the header shows the branch."),
        ]
    }

    /// The lines that hold something, each with its runs of blanks as one.
    fn texts(page: &Page) -> Vec<String> {
        let words = |i| page.text(i).split_whitespace().collect::<Vec<_>>().join(" ");
        (0..page.lines.len()).map(words).filter(|l| !l.is_empty()).collect()
    }

    #[test]
    fn a_conversation_reads_from_its_first_prompt_with_a_rule_at_each_new_day() {
        let page = Page::of(&two_days(), 60, true);
        let rows = texts(&page);
        assert!(rows[0].starts_with("── Thursday 19 November 2026 ─"), "{rows:?}");
        assert_eq!(
            rows[1..6],
            ["› why is the sidebar blank", "▸ run cargo test ✓", "The Sidebar read the wrong column.", "› fix it", "Fixed."]
        );
        assert!(rows[6].starts_with("── Friday 20 November 2026 ─"), "{rows:?}");
        assert_eq!(rows[7..], ["› now the header", "Done: the header shows the branch."]);
        // Each prompt is known by the line it starts on: the blank one above it.
        let starts: Vec<String> = page.prompts.iter().map(|p| page.text(p.0 + 1).trim().to_string()).collect();
        assert_eq!(starts, ["› why is the sidebar blank", "› fix it", "› now the header"]);
    }

    #[test]
    fn without_the_tool_calls_only_what_was_said_is_left() {
        let rows = texts(&Page::of(&two_days(), 60, false));
        assert!(!rows.iter().any(|r| r.contains("cargo test")), "{rows:?}");
        assert_eq!(rows.iter().filter(|r| r.starts_with('›')).count(), 3);
    }

    #[test]
    fn a_log_without_times_has_no_rules() {
        // A subagent's events carry no time.
        let rows = texts(&Page::of(&[said(1, 0, "look into it"), replied(2, 0, "Looked.")], 60, true));
        assert_eq!(rows, ["› look into it", "Looked."]);
    }

    #[test]
    fn a_search_goes_down_or_up_and_around_and_minds_capitals_only_when_asked() {
        let page = Page::of(&two_days(), 60, true);
        let line = |needle: &str| (0..page.lines.len()).find(|&i| page.text(i).contains(needle)).unwrap();
        let (asked, told) = (line("why is the sidebar"), line("The Sidebar read"));
        assert_eq!(page.find("sidebar", 0, true), Some(asked));
        assert_eq!(page.find("sidebar", asked + 1, true), Some(told));
        assert_eq!(page.find("sidebar", told + 1, true), Some(asked), "around the end to the top");
        assert_eq!(page.find("sidebar", page.lines.len() - 1, false), Some(told), "up from the end: the newest first");
        assert_eq!(page.find("Sidebar", 0, true), Some(told), "a capital asks for that capital");
        assert_eq!(page.find("no such words", 0, true), None);
    }

    #[test]
    fn the_reader_opens_at_the_end_and_moves_by_line_page_and_prompt() {
        let mut r = Reading::new();
        r.load(two_days());
        let total = r.page(60).lines.len();
        let h = 6;
        assert_eq!((r.scroll(h), r.top(h)), (0, total - h), "it opens on the end");
        assert_eq!(r.key(SKey::Up, h), After::Stay);
        assert_eq!(r.scroll(h), 1);
        r.key(SKey::Char('g'), h);
        assert_eq!(r.top(h), 0);
        r.key(SKey::Up, h);
        assert_eq!(r.top(h), 0, "never past the top");
        r.key(SKey::PageDown, h);
        assert_eq!(r.top(h), 4, "a page is the pane less two rows");
        r.key(SKey::Char('G'), h);
        assert_eq!(r.scroll(h), 0);
        // { goes to the prompt above, } to the one below.
        let prompts: Vec<usize> = r.page(60).prompts.iter().map(|p| p.0).collect();
        // The last prompt is on screen already: the first { shows the one before it.
        r.key(SKey::Char('{'), h);
        assert_eq!(r.top(h), prompts[1]);
        r.key(SKey::Char('{'), h);
        assert_eq!(r.top(h), prompts[0]);
        r.key(SKey::Char('}'), h);
        assert_eq!(r.top(h), prompts[1]);
        r.key(SKey::Char('}'), h);
        assert_eq!(r.top(h), prompts[2].min(total - h), "as near the top as the end allows");
        assert_eq!(r.key(SKey::Char('q'), h), After::Close);
    }

    #[test]
    fn a_search_is_typed_run_with_enter_and_repeated_with_n() {
        let mut r = Reading::new();
        r.load(two_days());
        let h = 6;
        let page = r.page(60);
        let line = |needle: &str| (0..page.lines.len()).find(|&i| page.text(i).contains(needle)).unwrap();
        let (asked, told) = (line("why is the sidebar"), line("The Sidebar read"));
        assert_eq!(r.key(SKey::Char('n'), h), After::Say("Nothing was searched for yet. Press / and type what to find.".into()));
        // ? looks up from the bottom of the pane: the newest match first.
        r.key(SKey::Char('?'), h);
        for c in "sidebr".chars() {
            r.key(SKey::Char(c), h);
        }
        r.key(SKey::Back, h);
        r.key(SKey::Char('a'), h);
        r.key(SKey::Char('r'), h);
        assert_eq!(r.typing(), Some(("sidebar", false)));
        assert_eq!(r.key(SKey::Char('q'), h), After::Stay, "a q typed is a letter of the search");
        r.key(SKey::Back, h);
        assert_eq!(r.key(SKey::Enter, h), After::Stay);
        assert_eq!((r.typing(), r.hit()), (None, Some(told)));
        assert!(r.top(h) <= told && told < r.top(h) + h, "what was found is on screen");
        r.key(SKey::Char('n'), h);
        assert_eq!(r.hit(), Some(asked), "n goes on the same way: up");
        r.key(SKey::Char('N'), h);
        assert_eq!(r.hit(), Some(told), "N goes the other way");
        r.key(SKey::Char('/'), h);
        for c in "zebra".chars() {
            r.key(SKey::Char(c), h);
        }
        assert_eq!(r.key(SKey::Enter, h), After::Say("\"zebra\" is not in this conversation.".into()));
        // esc while typing ends the typing, not the view.
        r.key(SKey::Char('/'), h);
        assert_eq!(r.key(SKey::Close, h), After::Stay);
        assert_eq!(r.key(SKey::Close, h), After::Close);
    }

    #[test]
    fn hiding_the_tool_calls_keeps_the_prompt_on_screen() {
        let mut r = Reading::new();
        r.load(two_days());
        let h = 4;
        r.page(60);
        r.key(SKey::Char('g'), h);
        r.key(SKey::Char('}'), h);
        r.key(SKey::Char('}'), h);
        let top = r.top(h);
        assert_eq!(r.page(60).text(top + 1).trim(), "› fix it");
        r.key(SKey::Char('t'), h);
        assert!(!r.tools());
        let top = r.top(h);
        assert_eq!(r.page(60).text(top + 1).trim(), "› fix it", "the same prompt, though the lines above it are fewer");
    }

    #[test]
    fn marked_text_is_copied_without_the_margin_and_a_double_click_takes_a_word() {
        let mut r = Reading::new();
        r.load(vec![
            said(1, 0, "where is the config"),
            replied(2, 0, "It is in `~/.config/desl/config.toml`, read at start.\n\nRestart to apply: 日本語 ok."),
        ]);
        let h = 20;
        let page = r.page(60);
        let line = |needle: &str| (0..page.lines.len()).find(|&i| page.text(i).contains(needle)).unwrap();
        let (first, second) = (u16::try_from(line("It is in")).unwrap(), u16::try_from(line("Restart to apply")).unwrap());
        // The page is shorter than the pane, so its lines are the pane's rows.
        assert_eq!(r.top(h), 0);
        let col = |row: u16, needle: &str| u16::try_from(r.page.as_ref().unwrap().text(usize::from(row)).find(needle).unwrap()).unwrap();
        let path = col(first, "~/.config");
        assert_eq!(r.word(first, path + 3, h), Some((path, path + 26)), "a path is one word, with the comma that touches it");
        assert_eq!(r.text_between((first, path), (first, path + 25), h), "~/.config/desl/config.toml");
        assert_eq!(r.word(first, 0, h), None, "the margin is blank");
        // From the middle of one line to the end of another: the second has no margin left.
        let from = col(first, "read at start");
        assert_eq!(r.text_between((first, from), (second, 59), h), "read at start.\n\nRestart to apply: 日本語 ok.");
        // A whole row, as a triple click marks it, starts at its text and not at the margin.
        assert_eq!(r.text_between((second, 0), (second, 59), h), "Restart to apply: 日本語 ok.");
        // A wide character is two columns and one character.
        let wide = col(second, "日");
        assert_eq!(r.text_between((second, wide), (second, wide + 3), h), "日本");
        assert_eq!(r.word(second, wide + 1, h), Some((wide, wide + 5)));
    }

    #[test]
    fn until_the_daemon_answers_the_pane_says_it_is_reading() {
        let mut r = Reading::new();
        assert!(texts(r.page(60))[0].starts_with("Reading the conversation"));
        r.load(Vec::new());
        assert_eq!(texts(r.page(60)), ["Nothing yet."]);
        assert_eq!(r.place(10), "[0/0]");
    }
}
