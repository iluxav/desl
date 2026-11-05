//! Markdown for an agent's prose, parsed with pulldown-cmark and laid out for one terminal column:
//! headings, emphasis, code spans, links as numbered footnotes, nested lists, quotes, tables, and
//! fenced code with visible boundaries and light syntax colouring.

use crate::theme;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// One laid-out line, without the transcript's left padding.
pub type Row = Vec<Span<'static>>;
/// A run of text in one style. A chunk holding only `"\n"` is a line break.
type Chunk = (String, Style);

const BULLETS: [&str; 3] = ["•", "◦", "▪"];
const MIN_COLUMN: usize = 6;

/// A list item or a quote: what goes in front of its first line and of every later one.
struct Container {
    first: Chunk,
    rest: Chunk,
    used: bool,
}

#[derive(Default)]
struct Table {
    aligns: Vec<Alignment>,
    rows: Vec<Vec<Vec<Chunk>>>,
    head_rows: usize,
}

struct Renderer<'a> {
    src: &'a str,
    width: usize,
    out: Vec<Row>,
    /// Inline content of the block being read.
    cur: Vec<Chunk>,
    heading: Option<HeadingLevel>,
    quotes: usize,
    bold: usize,
    italic: usize,
    strike: usize,
    link: usize,
    containers: Vec<Container>,
    /// Open lists: the next number of an ordered one, `None` for bullets.
    lists: Vec<Option<u64>>,
    /// Link targets, numbered in order of first use.
    links: Vec<String>,
    /// Open links: target and where their text starts in `cur`.
    open_links: Vec<(String, usize)>,
    code: Option<(String, String)>,
    table: Option<Table>,
}

fn width_of(chunks: &[Chunk]) -> usize {
    chunks.iter().map(|(t, _)| t.width()).sum()
}

fn spans(chunks: Vec<Chunk>) -> Row {
    chunks.into_iter().filter(|(t, _)| !t.is_empty()).map(|(t, s)| Span::styled(t, s)).collect()
}

/// True when the line above the one holding byte `at` is blank, and nothing but indentation
/// precedes `at` on its own line: the author left a gap before this block.
fn blank_before(src: &str, at: usize) -> bool {
    let before = &src[..at.min(src.len())];
    let Some(nl) = before.rfind('\n') else { return false };
    if !before[nl + 1..].trim().is_empty() {
        return false;
    }
    let above = &before[..nl];
    let line = above.rsplit('\n').next().unwrap_or("");
    line.trim().is_empty()
}

/// Greedy word wrap over styled text. Words never split across styles; one wider than the line is cut.
fn wrap_chunks(chunks: &[Chunk], width: usize) -> Vec<Vec<Chunk>> {
    let width = width.max(1);
    // Atoms: a word (several styles, no spaces) or a run of spaces.
    enum Atom {
        Word(Vec<Chunk>, usize),
        Space(String, Style),
    }
    let mut atoms: Vec<Atom> = Vec::new();
    for (text, style) in chunks {
        let mut buf = String::new();
        let mut buf_space = false;
        let push = |buf: &mut String, space: bool, atoms: &mut Vec<Atom>| {
            if buf.is_empty() {
                return;
            }
            let t = std::mem::take(buf);
            if space {
                atoms.push(Atom::Space(t, *style));
            } else if let Some(Atom::Word(parts, w)) = atoms.last_mut() {
                *w += t.width();
                parts.push((t, *style));
            } else {
                let w = t.width();
                atoms.push(Atom::Word(vec![(t, *style)], w));
            }
        };
        for c in text.chars() {
            let space = c == ' ';
            if space != buf_space {
                push(&mut buf, buf_space, &mut atoms);
                buf_space = space;
            }
            buf.push(c);
        }
        push(&mut buf, buf_space, &mut atoms);
    }

    let mut lines: Vec<Vec<Chunk>> = Vec::new();
    let mut line: Vec<Chunk> = Vec::new();
    let mut used = 0usize;
    let mut pending: Option<Chunk> = None;
    for atom in atoms {
        match atom {
            Atom::Space(t, s) => {
                if used > 0 {
                    pending = Some((t, s));
                }
            }
            Atom::Word(parts, w) => {
                let gap = pending.as_ref().map(|(t, _)| t.width()).unwrap_or(0);
                if used > 0 && used + gap + w > width {
                    lines.push(std::mem::take(&mut line));
                    used = 0;
                    pending = None;
                }
                if let Some(p) = pending.take() {
                    used += p.0.width();
                    line.push(p);
                }
                if w <= width {
                    used += w;
                    line.extend(parts);
                } else {
                    // A word longer than the line (a URL, a path): cut it where the line ends.
                    for (t, s) in parts {
                        let mut piece = String::new();
                        for c in t.chars() {
                            let cw = c.width().unwrap_or(0);
                            if used + cw > width {
                                line.push((std::mem::take(&mut piece), s));
                                lines.push(std::mem::take(&mut line));
                                used = 0;
                            }
                            piece.push(c);
                            used += cw;
                        }
                        line.push((piece, s));
                    }
                }
            }
        }
    }
    lines.push(line);
    lines
}

/// Cuts styled text at exactly `width` columns, for code, where spaces carry meaning.
fn hard_wrap(chunks: Vec<Chunk>, width: usize) -> Vec<Vec<Chunk>> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line: Vec<Chunk> = Vec::new();
    let mut used = 0;
    for (t, s) in chunks {
        let mut piece = String::new();
        for c in t.chars() {
            let cw = c.width().unwrap_or(0);
            if used + cw > width {
                line.push((std::mem::take(&mut piece), s));
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            piece.push(c);
            used += cw;
        }
        line.push((piece, s));
    }
    lines.push(line);
    lines
}

enum Comments {
    Hash,
    Slash,
    Dash,
    None,
}

fn comments_of(lang: &str) -> Option<Comments> {
    Some(match lang {
        "py" | "python" | "sh" | "bash" | "zsh" | "fish" | "shell" | "yaml" | "yml" | "toml" | "ruby" | "rb" | "make" | "makefile" | "dockerfile"
        | "r" | "perl" | "nix" | "ini" | "conf" => Comments::Hash,
        "rust" | "rs" | "js" | "javascript" | "ts" | "typescript" | "tsx" | "jsx" | "go" | "c" | "h" | "cpp" | "c++" | "java" | "kotlin" | "kt"
        | "swift" | "cs" | "csharp" | "php" | "scala" | "zig" | "dart" | "css" | "scss" | "proto" => Comments::Slash,
        "sql" | "lua" | "haskell" | "hs" | "elm" => Comments::Dash,
        "json" | "jsonc" | "json5" | "html" | "xml" => Comments::None,
        _ => return None,
    })
}

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "case", "catch", "class", "const", "continue", "crate", "def", "default", "defer", "del", "do", "done", "dyn",
    "elif", "else", "enum", "esac", "except", "export", "extends", "false", "False", "fi", "finally", "fn", "for", "from", "func", "function", "go",
    "if", "impl", "implements", "import", "in", "interface", "is", "lambda", "let", "local", "loop", "match", "mod", "move", "mut", "new", "nil",
    "None", "not", "null", "of", "package", "pass", "private", "protected", "pub", "public", "raise", "ref", "return", "select", "self", "static",
    "struct", "super", "switch", "then", "this", "throw", "trait", "true", "True", "try", "type", "typeof", "undefined", "unsafe", "use", "var",
    "void", "where", "while", "with", "yield", "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "TABLE",
    "JOIN", "ON", "GROUP", "BY", "ORDER", "LIMIT", "AND", "OR", "NOT", "NULL", "AS",
];

/// Colours one line of code: comments, strings and keywords. Deliberately small; no grammar, no state
/// across lines. A language it does not know stays plain.
fn highlight(lang: &str, line: &str) -> Vec<Chunk> {
    if lang == "diff" || lang == "patch" {
        let style = match line.chars().next() {
            Some('+') => theme::code_string(),
            Some('-') => theme::critical(),
            Some('@') => theme::accent(),
            _ => theme::dim(),
        };
        return vec![(line.to_string(), style)];
    }
    let Some(comments) = comments_of(lang) else {
        return vec![(line.to_string(), theme::dim())];
    };
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<Chunk> = Vec::new();
    let mut plain = String::new();
    let flush = |plain: &mut String, out: &mut Vec<Chunk>| {
        if !plain.is_empty() {
            out.push((std::mem::take(plain), theme::normal()));
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let prev = if i > 0 { Some(chars[i - 1]) } else { None };
        let next = chars.get(i + 1).copied();
        let starts_comment = match comments {
            Comments::Hash => c == '#' && prev.is_none_or(|p| p.is_whitespace()),
            Comments::Slash => c == '/' && next == Some('/') && prev != Some(':'),
            Comments::Dash => c == '-' && next == Some('-'),
            Comments::None => false,
        };
        if starts_comment {
            flush(&mut plain, &mut out);
            out.push((chars[i..].iter().collect(), theme::muted().add_modifier(Modifier::ITALIC)));
            return out;
        }
        if c == '"' || c == '\'' || c == '`' {
            // A quote directly after a letter is an apostrophe or a lifetime, not a string.
            if c == '\'' && (prev.is_some_and(|p| p.is_alphanumeric()) || !chars[i + 1..].contains(&'\'')) {
                plain.push(c);
                i += 1;
                continue;
            }
            flush(&mut plain, &mut out);
            let mut j = i + 1;
            while j < chars.len() && chars[j] != c {
                if chars[j] == '\\' {
                    j += 1;
                }
                j += 1;
            }
            let end = (j + 1).min(chars.len());
            out.push((chars[i..end].iter().collect(), theme::code_string()));
            i = end;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let mut j = i;
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            if KEYWORDS.contains(&word.as_str()) && prev != Some('.') {
                flush(&mut plain, &mut out);
                out.push((word, theme::accent()));
            } else {
                plain.push_str(&word);
            }
            i = j;
            continue;
        }
        plain.push(c);
        i += 1;
    }
    flush(&mut plain, &mut out);
    out
}

impl<'a> Renderer<'a> {
    fn style(&self) -> Style {
        let mut s = match self.heading {
            Some(HeadingLevel::H1) => theme::accent().add_modifier(Modifier::BOLD),
            Some(_) => theme::bold(),
            None if self.quotes > 0 => theme::dim(),
            None => theme::normal(),
        };
        if self.bold > 0 {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.italic > 0 {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if self.strike > 0 {
            s = s.add_modifier(Modifier::CROSSED_OUT);
        }
        if self.link > 0 {
            s = s.add_modifier(Modifier::UNDERLINED);
        }
        s
    }

    /// Columns left for content inside the open lists and quotes.
    fn avail(&self) -> usize {
        let taken: usize = self.containers.iter().map(|c| c.rest.0.width()).sum();
        self.width.saturating_sub(taken).max(MIN_COLUMN)
    }

    fn emit(&mut self, content: Vec<Chunk>) {
        let mut row: Vec<Chunk> = Vec::new();
        for c in &mut self.containers {
            row.push(if c.used { c.rest.clone() } else { c.first.clone() });
            c.used = true;
        }
        row.extend(content);
        self.out.push(spans(row));
    }

    fn gap(&mut self) {
        if self.out.last().is_some_and(|r| !r.is_empty()) {
            self.out.push(Vec::new());
        }
    }

    /// A block begins at byte `at`: leave a blank line if the author did.
    fn block(&mut self, at: usize) {
        self.flush();
        if blank_before(self.src, at) {
            self.gap();
        }
    }

    /// Lays out the inline content gathered so far as wrapped lines.
    fn flush(&mut self) {
        if self.cur.is_empty() {
            return;
        }
        let chunks = std::mem::take(&mut self.cur);
        let avail = self.avail();
        for segment in chunks.split(|(t, _)| t == "\n") {
            for line in wrap_chunks(segment, avail) {
                self.emit(line);
            }
        }
    }

    fn text(&mut self, t: &str) {
        let style = self.style();
        for (i, part) in t.split('\n').enumerate() {
            if i > 0 {
                self.cur.push(("\n".into(), style));
            }
            if !part.is_empty() {
                self.cur.push((part.to_string(), style));
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>, at: usize) {
        match tag {
            Tag::Paragraph => self.block(at),
            Tag::Heading { level, .. } => {
                self.flush();
                self.gap();
                self.heading = Some(level);
            }
            Tag::BlockQuote(_) => {
                self.block(at);
                let bar = ("▎ ".to_string(), theme::muted());
                self.containers.push(Container { first: bar.clone(), rest: bar, used: false });
                self.quotes += 1;
            }
            Tag::CodeBlock(kind) => {
                self.block(at);
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_lowercase(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::List(first) => {
                self.flush();
                self.lists.push(first);
            }
            Tag::Item => {
                self.block(at);
                let depth = self.lists.len().saturating_sub(1);
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => format!("{} ", BULLETS[depth % BULLETS.len()]),
                };
                let rest = " ".repeat(marker.width());
                self.containers.push(Container { first: (marker, theme::muted()), rest: (rest, theme::muted()), used: false });
            }
            Tag::Table(aligns) => {
                self.block(at);
                self.table = Some(Table { aligns, ..Table::default() });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(t) = &mut self.table {
                    t.rows.push(Vec::new());
                }
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.open_links.push((dest_url.to_string(), self.cur.len()));
                self.link += 1;
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush(),
            TagEnd::Heading(_) => {
                self.flush();
                self.heading = None;
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.containers.pop();
                self.quotes = self.quotes.saturating_sub(1);
            }
            TagEnd::CodeBlock => self.code_block(),
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
            }
            TagEnd::Item => {
                self.flush();
                // An item with no text still shows its marker.
                if self.containers.last().is_some_and(|c| !c.used) {
                    self.emit(Vec::new());
                }
                self.containers.pop();
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.cur);
                if let Some(row) = self.table.as_mut().and_then(|t| t.rows.last_mut()) {
                    row.push(cell);
                }
            }
            TagEnd::TableHead => {
                if let Some(t) = &mut self.table {
                    t.head_rows = t.rows.len();
                }
            }
            TagEnd::Table => self.table_block(),
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link | TagEnd::Image => {
                self.link = self.link.saturating_sub(1);
                let Some((dest, from)) = self.open_links.pop() else { return };
                let shown: String = self.cur[from.min(self.cur.len())..].iter().map(|(t, _)| t.as_str()).collect();
                let bare = dest.trim_start_matches("mailto:");
                // A link that shows its own address needs no footnote; neither does one that goes nowhere.
                if dest.is_empty() || dest.starts_with('#') || shown.trim() == dest || shown.trim() == bare {
                    return;
                }
                let n = match self.links.iter().position(|l| *l == dest) {
                    Some(i) => i + 1,
                    None => {
                        self.links.push(dest);
                        self.links.len()
                    }
                };
                self.cur.push((format!("[{n}]"), theme::muted()));
            }
            _ => {}
        }
    }

    fn code_block(&mut self) {
        let Some((lang, body)) = self.code.take() else { return };
        let avail = self.avail();
        let label = if lang.is_empty() { "┄┄".to_string() } else { format!("┄┄ {lang} ") };
        let fill = avail.saturating_sub(label.width());
        self.emit(vec![(format!("{label}{}", "┄".repeat(fill)), theme::muted())]);
        for line in body.strip_suffix('\n').unwrap_or(&body).split('\n') {
            let line = line.replace('\t', "    ");
            for part in hard_wrap(highlight(&lang, &line), avail.saturating_sub(2)) {
                let mut row = vec![("│ ".to_string(), theme::muted())];
                row.extend(part);
                self.emit(row);
            }
        }
        self.emit(vec![("┄".repeat(avail), theme::muted())]);
    }

    fn table_block(&mut self) {
        let Some(table) = self.table.take() else { return };
        let cols = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        // Natural widths, then the widest columns give way until the table fits.
        let mut widths = vec![1usize; cols];
        for row in &table.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(width_of(cell));
            }
        }
        let room = self.avail().saturating_sub(3 * cols + 1).max(cols);
        while widths.iter().sum::<usize>() > room {
            let Some((i, _)) = widths.iter().enumerate().filter(|(_, w)| **w > MIN_COLUMN.min(room / cols).max(1)).max_by_key(|(_, w)| **w) else {
                break;
            };
            widths[i] -= 1;
        }
        let rule = |l: &str, m: &str, r: &str| -> Vec<Chunk> {
            let body: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
            vec![(format!("{l}{}{r}", body.join(m)), theme::muted())]
        };
        let laid: Vec<Vec<Vec<Vec<Chunk>>>> =
            table.rows.iter().map(|row| (0..cols).map(|i| wrap_chunks(row.get(i).map(Vec::as_slice).unwrap_or(&[]), widths[i])).collect()).collect();
        let wrapped = laid.iter().any(|row| row.iter().any(|cell| cell.len() > 1));
        self.emit(rule("┌", "┬", "┐"));
        for (r, row) in laid.iter().enumerate() {
            let height = row.iter().map(Vec::len).max().unwrap_or(1);
            for line in 0..height {
                let mut out: Vec<Chunk> = vec![("│".to_string(), theme::muted())];
                for (i, cell) in row.iter().enumerate() {
                    let content: Vec<Chunk> = cell.get(line).cloned().unwrap_or_default();
                    let pad = widths[i].saturating_sub(width_of(&content));
                    let (left, right) = match table.aligns.get(i) {
                        Some(Alignment::Right) => (pad, 0),
                        Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                        _ => (0, pad),
                    };
                    out.push((" ".repeat(left + 1), theme::normal()));
                    for (t, s) in content {
                        out.push((t, if r < table.head_rows { s.add_modifier(Modifier::BOLD) } else { s }));
                    }
                    out.push((" ".repeat(right + 1), theme::normal()));
                    out.push(("│".to_string(), theme::muted()));
                }
                self.emit(out);
            }
            let last = r + 1 == laid.len();
            // A rule under the header always; between rows only when a cell needed more than one line.
            if !last && (r + 1 == table.head_rows || wrapped) {
                self.emit(rule("├", "┼", "┤"));
            }
        }
        self.emit(rule("└", "┴", "┘"));
    }

    fn footnotes(&mut self) {
        if self.links.is_empty() {
            return;
        }
        self.gap();
        let links = std::mem::take(&mut self.links);
        for (i, url) in links.iter().enumerate() {
            let label = format!("[{}] ", i + 1);
            let hang = " ".repeat(label.width());
            let room = self.width.saturating_sub(label.width());
            for (n, part) in hard_wrap(vec![(url.clone(), theme::muted())], room).into_iter().enumerate() {
                let mut row = vec![(if n == 0 { label.clone() } else { hang.clone() }, theme::muted())];
                row.extend(part);
                self.emit(row);
            }
        }
    }
}

/// Lays `text` out in `width` columns.
pub fn render(text: &str, width: usize) -> Vec<Row> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let mut r = Renderer {
        src: text,
        width: width.max(MIN_COLUMN),
        out: Vec::new(),
        cur: Vec::new(),
        heading: None,
        quotes: 0,
        bold: 0,
        italic: 0,
        strike: 0,
        link: 0,
        containers: Vec::new(),
        lists: Vec::new(),
        links: Vec::new(),
        open_links: Vec::new(),
        code: None,
        table: None,
    };
    for (event, range) in Parser::new_ext(text, opts).into_offset_iter() {
        match event {
            Event::Start(tag) => r.start(tag, range.start),
            Event::End(tag) => r.end(tag),
            Event::Text(t) => match &mut r.code {
                Some((_, body)) => body.push_str(&t),
                None => r.text(&t),
            },
            Event::Code(t) => r.cur.push((t.to_string(), theme::accent())),
            // A single newline in the source stays a line break: agents write short lines on purpose.
            Event::SoftBreak | Event::HardBreak => r.cur.push(("\n".into(), theme::normal())),
            Event::Rule => {
                r.block(range.start);
                let w = r.avail().min(40);
                r.emit(vec![("─".repeat(w), theme::muted())]);
            }
            Event::TaskListMarker(done) => r.cur.push((if done { "☑ " } else { "☐ " }.to_string(), theme::muted())),
            Event::Html(t) | Event::InlineHtml(t) | Event::InlineMath(t) | Event::DisplayMath(t) => r.text(t.trim_end_matches('\n')),
            Event::FootnoteReference(t) => r.cur.push((format!("[{t}]"), theme::muted())),
        }
    }
    r.flush();
    r.footnotes();
    while r.out.last().is_some_and(Vec::is_empty) {
        r.out.pop();
    }
    r.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str, width: usize) -> Vec<String> {
        render(text, width).into_iter().map(|row| row.iter().map(|s| s.content.as_ref()).collect::<String>()).collect()
    }

    #[test]
    fn links_become_footnotes_and_prose_stays_readable() {
        let out = plain("See [the report](https://example.com/a/very/long/path) and [again](https://example.com/a/very/long/path).", 60);
        assert_eq!(out[0], "See the report[1] and again[1].");
        assert_eq!(out.last().unwrap(), "[1] https://example.com/a/very/long/path");
        assert_eq!(plain("<https://example.com>", 60), vec!["https://example.com"]);
    }

    #[test]
    fn nested_lists_indent_under_their_parent_and_wrap_with_a_hang() {
        let out = plain("- one\n  - inner item that is long enough to wrap\n- two\n\n1. first\n2. second", 24);
        assert_eq!(out, vec!["• one", "  ◦ inner item that is", "    long enough to wrap", "• two", "", "1. first", "2. second"]);
    }

    #[test]
    fn blank_lines_follow_the_source() {
        assert_eq!(plain("Sources:\n- a\n- b", 40), vec!["Sources:", "• a", "• b"]);
        assert_eq!(plain("One.\n\nTwo.\nThree.", 40), vec!["One.", "", "Two.", "Three."]);
        assert_eq!(plain("Intro\n## Title\nBody", 40), vec!["Intro", "", "Title", "Body"]);
    }

    #[test]
    fn a_table_is_boxed_and_never_wider_than_the_column() {
        let md = "| Name | Notes |\n| --- | ---: |\n| alpha | a rather long note that has to wrap somewhere |\n| b | 2 |";
        let out = plain(md, 30);
        assert!(out.iter().all(|l| l.width() <= 30), "{out:#?}");
        assert!(out[0].starts_with('┌') && out.last().unwrap().starts_with('└'));
        assert!(out[1].contains("Name") && out[1].contains("Notes"));
        assert!(out[2].starts_with('├'));
        let wide = plain("| a | b |\n|---|---|\n| 1 | 2 |", 40);
        assert_eq!(wide, vec!["┌───┬───┐", "│ a │ b │", "├───┼───┤", "│ 1 │ 2 │", "└───┴───┘"]);
    }

    #[test]
    fn code_keeps_visible_boundaries_and_every_character() {
        let out = plain("```rust\nfn main() { println!(\"hi\"); }\n```", 20);
        assert!(out[0].starts_with("┄┄ rust ┄"));
        assert_eq!(out[1], "│ fn main() { printl");
        assert_eq!(out[2], "│ n!(\"hi\"); }");
        assert!(out[3].chars().all(|c| c == '┄'));
        // An unclosed fence, as seen mid-stream, is still a code block.
        assert!(plain("```\nls -la", 20)[1].starts_with("│ ls"));
    }

    #[test]
    fn code_colours_keywords_strings_and_comments() {
        let row = highlight("rust", "let s = \"hi\"; // greet");
        let kinds: Vec<(&str, Style)> = row.iter().map(|(t, s)| (t.as_str(), *s)).collect();
        assert!(kinds.contains(&("let", theme::accent())));
        assert!(kinds.contains(&("\"hi\"", theme::code_string())));
        assert!(kinds.iter().any(|(t, _)| *t == "// greet"));
        assert_eq!(highlight("text", "let x").len(), 1);
    }

    #[test]
    fn no_line_is_wider_than_asked() {
        let md = "# Heading that is quite long indeed\n\nSome **bold and long** prose with `code` and a https://example.com/some/really/long/url/that/cannot/break/anywhere link.\n\n> quoted text that also has to wrap around\n\n- [ ] todo item number one which wraps\n- [x] done";
        for width in [12, 20, 37, 80] {
            for line in plain(md, width) {
                assert!(line.width() <= width, "{width}: {line:?}");
            }
        }
    }
}
