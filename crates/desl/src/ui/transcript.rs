//! The transcript pane: the agent is the hero. Pre-wrapped lines, auto-follow, the `↓ n new` pill,
//! a selection gutter while the transcript is focused, expandable tool calls and briefs.

use super::markdown;
use super::text::{code_wrap, expand_tabs, truncate_end, truncate_middle, wrap};
use crate::model::{Item, Open, PREVIEW_LINES};
use crate::theme;
use desl_ir::diff::{self, Kind};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

const INDENT: &str = "  ";
/// Prose is capped at this many columns on wide terminals; longer lines are hard to read.
const MAX_TEXT_WIDTH: usize = 100;
/// Expanded tool output shows at most this many lines before `… n more lines`.
const MAX_DETAIL_LINES: usize = 40;
/// Columns kept free at the right edge, so text never touches the pane border.
const RIGHT_MARGIN: usize = 1;

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

/// A conversation's items as lines at most `width` columns wide.
pub fn layout(items: &[Item], width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let width = width.min(MAX_TEXT_WIDTH);
    let width = width.saturating_sub(RIGHT_MARGIN);
    let body = width.saturating_sub(1 + INDENT.len()).max(8);
    if items.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(format!(" {INDENT}Nothing yet."), theme::muted())]));
        return lines;
    }
    for item in items {
        match item {
            Item::User { text } => user_lines(text, body, &mut lines),
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
                    (None, None) => String::new(),
                    (Some(d), None) => d.to_string(),
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
    }
    lines
}

/// A prompt: `›` in the accent colour, the text wrapped under it.
fn user_lines(text: &str, body: usize, lines: &mut Vec<Line<'static>>) {
    lines.push(Line::from(""));
    for (i, l) in wrap(text, body).into_iter().enumerate() {
        let lead = if i == 0 { Span::styled(" › ".to_string(), theme::accent()) } else { Span::raw(format!(" {INDENT}")) };
        lines.push(Line::from(vec![lead, Span::styled(l, theme::normal())]));
    }
}
