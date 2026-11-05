//! The transcript pane: the agent is the hero. Pre-wrapped lines, auto-follow, the `↓ n new` pill,
//! a selection gutter while the transcript is focused, expandable tool calls and briefs.

use super::markdown;
use super::text::{code_wrap, expand_tabs, truncate_end, truncate_middle, wrap};
use crate::app::{App, Focus};
use crate::diff::{self, Kind};
use crate::model::{Item, Open, PREVIEW_LINES, Status, Thread};
use crate::theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

const INDENT: &str = "  ";
/// Prose is capped at this many columns on wide terminals; longer lines are hard to read.
const MAX_TEXT_WIDTH: usize = 100;
/// Expanded tool output shows at most this many lines before `… n more lines`.
const MAX_DETAIL_LINES: usize = 40;
/// A fully opened diff shows at most this many rows.
const MAX_DIFF_LINES: usize = 400;
/// Columns kept free at the right edge, so text never touches the pane border.
const RIGHT_MARGIN: usize = 1;

pub struct Laid {
    pub lines: Vec<Line<'static>>,
    /// First line of each item, by item index.
    pub starts: Vec<usize>,
}

fn detail_lines(text: &str, body: usize, lines: &mut Vec<Line<'static>>) {
    let all: Vec<&str> = text.lines().collect();
    for l in all.iter().take(MAX_DETAIL_LINES) {
        lines.push(Line::from(vec![
            Span::raw(format!(" {INDENT}")),
            Span::styled("│ ", theme::muted()),
            Span::styled(truncate_end(&expand_tabs(l), body.saturating_sub(2)), theme::dim()),
        ]));
    }
    if all.len() > MAX_DETAIL_LINES {
        lines.push(Line::from(vec![
            Span::raw(format!(" {INDENT}")),
            Span::styled(format!("│ … {} more lines", all.len() - MAX_DETAIL_LINES), theme::muted()),
        ]));
    }
}

/// A unified diff under its edit: line numbers and signs in the gutter, green for added and red
/// for removed, context dim. Long lines wrap so the changed end of a line is never cut off.
fn diff_lines(unified: &str, body: usize, cap: usize, lines: &mut Vec<Line<'static>>) {
    let rows = diff::parse(unified);
    let digits = rows.iter().filter_map(|r| r.number).max().map(|n| n.to_string().len()).unwrap_or(0);
    // "│ " + number + " " + sign + " "
    let gutter = 2 + digits + 3;
    let text_width = body.saturating_sub(gutter).max(8);
    let bar = || vec![Span::raw(format!(" {INDENT}")), Span::styled("│ ", theme::muted())];
    for row in rows.iter().take(cap) {
        let (sign, mark, text) = match row.kind {
            Kind::Added => ("+", theme::added(), theme::added()),
            Kind::Removed => ("-", theme::removed(), theme::removed()),
            Kind::Context => (" ", theme::muted(), theme::dim()),
            Kind::Gap => {
                let mut spans = bar();
                spans.push(Span::styled(format!("{:>digits$} ⋯", ""), theme::muted()));
                lines.push(Line::from(spans));
                continue;
            }
        };
        let number = row.number.map(|n| n.to_string()).unwrap_or_default();
        for (i, part) in code_wrap(&expand_tabs(&row.text), text_width).into_iter().enumerate() {
            let mut spans = bar();
            if i == 0 {
                spans.push(Span::styled(format!("{number:>digits$} {sign} "), mark));
            } else {
                spans.push(Span::raw(" ".repeat(digits + 3)));
            }
            spans.push(Span::styled(part, text));
            lines.push(Line::from(spans));
        }
    }
    if rows.len() > cap {
        let mut spans = bar();
        spans.push(Span::styled(format!("… {} more lines", rows.len() - cap), theme::muted()));
        lines.push(Line::from(spans));
    }
}

/// `spin` is the current spinner frame while the thread's agent is working: a tool call still
/// waiting for its result shows it where the outcome will land.
pub fn layout(items: &[Item], width: usize, selected: Option<usize>, spin: Option<&'static str>) -> Laid {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut starts: Vec<usize> = Vec::with_capacity(items.len());
    let width = width.min(MAX_TEXT_WIDTH);
    let width = width.saturating_sub(RIGHT_MARGIN);
    let body = width.saturating_sub(1 + INDENT.len()).max(8);
    if items.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            format!(" {INDENT}Type a prompt and press enter. ? shows every key."),
            theme::muted(),
        )]));
        return Laid { lines, starts };
    }
    for (idx, item) in items.iter().enumerate() {
        let first = lines.len();
        match item {
            Item::User { text, queued } => {
                lines.push(Line::from(""));
                for (i, l) in wrap(text, body).into_iter().enumerate() {
                    let mut spans = vec![if i == 0 {
                        Span::styled(" › ".to_string(), theme::accent())
                    } else {
                        Span::raw(format!(" {INDENT}"))
                    }];
                    spans.push(Span::styled(l, theme::normal()));
                    lines.push(Line::from(spans));
                }
                if *queued {
                    lines.push(Line::from(Span::styled(format!(" {INDENT}⏳ queued"), theme::muted())));
                }
            }
            Item::Assistant { text, streaming } => {
                lines.push(Line::from(""));
                let pad = format!(" {INDENT}");
                let rows = markdown::render(text, body);
                let emitted = !rows.is_empty();
                for row in rows {
                    let mut spans = vec![Span::raw(pad.clone())];
                    spans.extend(row);
                    lines.push(Line::from(spans));
                }
                if *streaming {
                    let cursor = Span::styled("▍", theme::dim());
                    if emitted && !text.ends_with('\n') {
                        if let Some(last) = lines.last_mut() {
                            last.push_span(cursor);
                        }
                    } else {
                        lines.push(Line::from(vec![Span::raw(format!(" {INDENT}")), cursor]));
                    }
                }
            }
            Item::Tool { verb, target, outcome, decision, detail, diff, open, .. } => {
                let right = match (decision, outcome) {
                    (None, None) => spin.unwrap_or("").to_string(),
                    (Some(d), None) => spin.map(|f| format!("{d} {f}")).unwrap_or_else(|| d.to_string()),
                    (None, Some(o)) => o.summary.clone(),
                    (Some(d), Some(o)) if *d == "denied" => format!("{d} ✕"),
                    (Some(d), Some(o)) => format!("{d} {}", o.summary),
                };
                let right_w = right.width();
                let avail = width.saturating_sub(1 + INDENT.len() + 2 + verb.len() + 1 + if right_w > 0 { right_w + 2 } else { 0 });
                let target = truncate_middle(target, avail.max(4));
                let left_w = 1 + INDENT.len() + 2 + verb.len() + 1 + target.width();
                let pad = width.saturating_sub(left_w + right_w);
                let glyph = if *open == Open::Closed { "▸ " } else { "▾ " };
                let mut spans = vec![
                    Span::raw(format!(" {INDENT}")),
                    Span::styled(glyph, theme::muted()),
                    Span::styled(verb.clone(), theme::dim()),
                    Span::raw(" "),
                    Span::styled(target, theme::normal()),
                ];
                if right_w > 0 {
                    spans.push(Span::raw(" ".repeat(pad)));
                    let style = match outcome {
                        None => theme::dim(),
                        Some(o) if o.ok => theme::dim(),
                        Some(_) => theme::critical(),
                    };
                    spans.push(Span::styled(right, style));
                }
                lines.push(Line::from(spans));
                match (open, diff) {
                    (Open::Closed, _) => {}
                    (Open::Preview, Some(d)) => diff_lines(d, body, PREVIEW_LINES, &mut lines),
                    (_, Some(d)) => diff_lines(d, body, MAX_DIFF_LINES, &mut lines),
                    (_, None) if detail.trim().is_empty() => {
                        let note = if outcome.is_some() { "│ (no output)" } else { "│ (no output yet)" };
                        lines.push(Line::from(vec![Span::raw(format!(" {INDENT}")), Span::styled(note, theme::muted())]));
                    }
                    (_, None) => detail_lines(detail, body, &mut lines),
                }
            }
            Item::Divider(label) => {
                lines.push(Line::from(""));
                let text = format!("── {label} ");
                let fill = width.saturating_sub(1 + INDENT.len() + text.width());
                lines.push(Line::from(vec![
                    Span::raw(format!(" {INDENT}")),
                    Span::styled(format!("{text}{}", "─".repeat(fill)), theme::muted()),
                ]));
            }
            Item::Brief { label, text, tokens, expanded } => {
                let glyph = if *expanded { "▾ " } else { "▸ " };
                let right = format!("~{tokens} tokens");
                let left = format!(" {INDENT}{glyph}{label}");
                let pad = width.saturating_sub(left.width() + right.width());
                lines.push(Line::from(vec![
                    Span::raw(format!(" {INDENT}")),
                    Span::styled(glyph, theme::muted()),
                    Span::styled(label.clone(), theme::dim()),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(right, theme::muted()),
                ]));
                if *expanded {
                    detail_lines(text, body, &mut lines);
                }
            }
            Item::Note(text) => {
                for l in wrap(text, body) {
                    lines.push(Line::from(vec![
                        Span::raw(format!(" {INDENT}")),
                        Span::styled(l, theme::muted().add_modifier(Modifier::ITALIC)),
                    ]));
                }
            }
            Item::Error(text) => {
                for (i, l) in wrap(text, body.saturating_sub(2)).into_iter().enumerate() {
                    lines.push(Line::from(vec![
                        Span::raw(format!(" {INDENT}")),
                        Span::styled(if i == 0 { "✕ " } else { "  " }, theme::critical()),
                        Span::styled(l, theme::critical()),
                    ]));
                }
            }
        }
        // Items open with a blank spacer line; the selection gutter starts below it.
        let content_start = if lines.get(first).is_some_and(|l| l.width() == 0) { first + 1 } else { first };
        starts.push(content_start);
        if selected == Some(idx) {
            for l in lines.iter_mut().skip(content_start) {
                if let Some(s) = l.spans.first_mut() {
                    let rest: String = s.content.chars().skip(1).collect();
                    *s = Span::styled(format!("┃{rest}"), theme::accent());
                }
            }
        }
    }
    Laid { lines, starts }
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn spinner_frame(t: &Thread) -> &'static str {
    let elapsed = t.working_since.map(|s| s.elapsed()).unwrap_or_default();
    SPINNER[(elapsed.as_millis() / 100) as usize % SPINNER.len()]
}

/// `⠹ working 12s · ctrl+c to interrupt` while the thread's agent is working, else an empty line.
fn working_line(t: &Thread) -> Line<'static> {
    if t.status != Status::Working {
        return Line::from("");
    }
    let secs = t.working_since.map(|s| s.elapsed()).unwrap_or_default().as_secs();
    let time = if secs >= 60 { format!("{}m {:02}s", secs / 60, secs % 60) } else { format!("{secs}s") };
    let frame = spinner_frame(t);
    Line::from(vec![
        Span::raw(format!(" {INDENT}")),
        Span::styled(frame, theme::accent()),
        Span::styled(" working ", theme::dim()),
        Span::styled(format!("{time} · ctrl+c to interrupt"), theme::muted()),
    ])
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let selected = if app.focus == Focus::Transcript { app.thread().cursor } else { None };
    let spin = (app.thread().status == Status::Working).then(|| spinner_frame(app.thread()));
    let mut laid = layout(&app.thread().items, area.width as usize, selected, spin);
    // Two lines are always kept free under the last item. While the agent works the second one says
    // so, exactly where its next words will appear; when it stops, nothing above has to move.
    laid.lines.push(Line::from(""));
    laid.lines.push(working_line(app.thread()));
    let total = laid.lines.len();
    let height = area.height as usize;
    app.view.transcript_height = height;
    app.view.transcript_lines = total;

    let max = total.saturating_sub(height);
    let t = app.thread_mut();
    if let Some(i) = selected
        && let Some(&start) = laid.starts.get(i)
    {
        // Keep the selected item in view.
        let end = laid.starts.get(i + 1).copied().unwrap_or(total);
        if start < t.scroll || t.follow {
            if !t.follow {
                t.scroll = start.saturating_sub(1).min(max);
            }
        } else if end > t.scroll + height {
            t.scroll = (end.saturating_sub(height)).min(max);
        }
    }
    if t.scroll >= max {
        t.follow = true;
    }
    let offset = if t.follow { max } else { t.scroll.min(max) };
    if t.follow {
        t.scroll = max;
    }
    let hidden_below = total.saturating_sub(offset + height);

    let visible: Vec<Line<'static>> = laid.lines.into_iter().skip(offset).take(height).collect();
    f.render_widget(Paragraph::new(visible), area);

    if hidden_below > 0 {
        let label = format!(" ↓ {hidden_below} new ");
        let w = label.width() as u16;
        if w + 2 <= area.width {
            let pill = Rect::new(area.right() - w - 1, area.bottom() - 1, w, 1);
            f.render_widget(Line::from(Span::styled(label, theme::pill())), pill);
        }
    }
}
