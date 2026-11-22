//! Forms (docs/design/2026-11-21-web-forms.md): what an agent puts in front of the user on desl
//! web's page, described in JSON. This module is the form language: the widgets, the checks a
//! form and an answer pass, and the words desl uses for both, so the daemon, `desl mcp` and the
//! page agree.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The most widgets one form holds.
pub const MAX_WIDGETS: usize = 30;
/// The most a form may weigh, as JSON. The daemon refuses a larger one before reading it.
pub const MAX_BYTES: usize = 256 * 1024;

const KINDS: [&str; 7] = ["markdown", "choice", "checklist", "text", "number", "toggle", "select"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Form {
    pub title: String,
    pub widgets: Vec<Widget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_label: Option<String>,
}

/// One part of a form. The answer to a form is an object keyed by each widget's `id`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Widget {
    /// Text to read; it takes no answer.
    Markdown { id: String, text: String },
    /// One option, or several with `multi`; `other` also takes a typed answer.
    Choice {
        id: String,
        #[serde(default)]
        label: Option<String>,
        options: Vec<Opt>,
        #[serde(default)]
        multi: bool,
        #[serde(default)]
        other: bool,
        /// Where it starts: a label, or a list of them with `multi`.
        #[serde(default)]
        value: Option<Value>,
        #[serde(default)]
        optional: bool,
    },
    /// Steps to untick, reorder and annotate.
    Checklist {
        id: String,
        #[serde(default)]
        label: Option<String>,
        items: Vec<Step>,
        #[serde(default)]
        reorder: bool,
        #[serde(default)]
        optional: bool,
    },
    Text {
        id: String,
        label: String,
        #[serde(default)]
        lines: Option<u8>,
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default)]
        value: Option<String>,
        #[serde(default)]
        optional: bool,
    },
    Number {
        id: String,
        label: String,
        min: f64,
        max: f64,
        #[serde(default)]
        step: Option<f64>,
        #[serde(default)]
        unit: Option<String>,
        #[serde(default)]
        value: Option<f64>,
        #[serde(default)]
        optional: bool,
    },
    Toggle {
        id: String,
        label: String,
        #[serde(default)]
        value: bool,
        #[serde(default)]
        optional: bool,
    },
    /// One option from a list, as `choice` but drawn as a list box.
    Select {
        id: String,
        label: String,
        options: Vec<Opt>,
        #[serde(default)]
        value: Option<String>,
        #[serde(default)]
        optional: bool,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Opt {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default = "yes")]
    pub checked: bool,
}

fn yes() -> bool {
    true
}

impl Widget {
    pub fn id(&self) -> &str {
        match self {
            Widget::Markdown { id, .. }
            | Widget::Choice { id, .. }
            | Widget::Checklist { id, .. }
            | Widget::Text { id, .. }
            | Widget::Number { id, .. }
            | Widget::Toggle { id, .. }
            | Widget::Select { id, .. } => id,
        }
    }

    fn optional(&self) -> bool {
        match self {
            Widget::Markdown { .. } => true,
            Widget::Choice { optional, .. }
            | Widget::Checklist { optional, .. }
            | Widget::Text { optional, .. }
            | Widget::Number { optional, .. }
            | Widget::Toggle { optional, .. }
            | Widget::Select { optional, .. } => *optional,
        }
    }
}

/// A form as an agent sent it, checked: a sentence names the first thing wrong, by the widget's
/// place and id, so the agent can mend it in the same turn.
pub fn check(form: &Value) -> Result<Form, String> {
    let Some(top) = form.as_object() else { return Err("A form is a JSON object with a title and widgets.".to_string()) };
    let title = top.get("title").and_then(Value::as_str).unwrap_or("").trim();
    if title.is_empty() {
        return Err("The form has no title. Give it one: the question it asks, in one line.".to_string());
    }
    if title.contains('\n') || title.chars().count() > 120 {
        return Err("The form's title is one line of at most 120 characters. Put the rest in a markdown widget.".to_string());
    }
    // It is typed into the agent's terminal with the answer.
    if title.chars().any(char::is_control) {
        return Err("The form's title has a control character, such as a tab or an escape. Use plain text.".to_string());
    }
    let Some(widgets) = top.get("widgets").and_then(Value::as_array) else {
        return Err("The form has no widgets. Give it a list of them, at least one.".to_string());
    };
    if widgets.is_empty() {
        return Err("The form has no widgets. Give it at least one.".to_string());
    }
    if widgets.len() > MAX_WIDGETS {
        return Err(format!("The form has {} widgets; desl shows at most {MAX_WIDGETS}. Split it into two forms.", widgets.len()));
    }
    let mut parsed = Vec::with_capacity(widgets.len());
    for (n, w) in widgets.iter().enumerate() {
        let n = n + 1;
        let kind = w.get("kind").and_then(Value::as_str);
        let Some(kind) = kind else { return Err(format!("Widget {n} has no kind. Use one of: {}.", KINDS.join(", "))) };
        if !KINDS.contains(&kind) {
            return Err(format!("Widget {n} has the kind \"{kind}\", which desl does not know. Use one of: {}.", KINDS.join(", ")));
        }
        let id = w.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        if id.trim().is_empty() {
            return Err(format!("Widget {n} has no id. Give it one: the answer is keyed by it."));
        }
        if id.chars().count() > 64 {
            return Err(format!("Widget {n} has an id of {} characters; keep it under 64.", id.chars().count()));
        }
        if let Some(before) = parsed.iter().position(|p: &Widget| p.id() == id) {
            return Err(format!("Widget {n} has the id \"{id}\", which widget {} has already. Give each widget its own id.", before + 1));
        }
        let widget: Widget = serde_json::from_value(w.clone()).map_err(|e| format!("Widget {n} (\"{id}\", {kind}) is not right: {e}."))?;
        sound(&widget, n)?;
        parsed.push(widget);
    }
    let send_label = top.get("send_label").and_then(Value::as_str).map(str::to_string);
    if send_label.as_ref().is_some_and(|l| l.chars().count() > 24) {
        return Err("The form's send_label is at most 24 characters.".to_string());
    }
    // Anything else in the form, such as a misspelt field, is refused by name.
    if let Some(extra) = top.keys().find(|k| !["title", "widgets", "send_label"].contains(&k.as_str())) {
        return Err(format!("The form has \"{extra}\", which desl does not know. A form has a title, widgets and a send_label."));
    }
    Ok(Form { title: title.to_string(), widgets: parsed, send_label })
}

/// What only a sound widget has: options to choose from, a range with its value in it.
fn sound(w: &Widget, n: usize) -> Result<(), String> {
    let id = w.id();
    let twice = |labels: Vec<&str>| labels.iter().enumerate().find(|(i, l)| labels[..*i].contains(l)).map(|(_, l)| l.to_string());
    match w {
        Widget::Choice { options, multi, value, .. } => {
            if !(2..=12).contains(&options.len()) {
                return Err(format!("Widget {n} (\"{id}\") has {} options; a choice has 2 to 12.", options.len()));
            }
            if let Some(l) = twice(options.iter().map(|o| o.label.as_str()).collect()) {
                return Err(format!("Widget {n} (\"{id}\") has the option \"{l}\" twice. The answer names an option by its label."));
            }
            let starts: Vec<&Value> = match (value, multi) {
                (None, _) => Vec::new(),
                (Some(Value::Array(list)), true) => list.iter().collect(),
                (Some(_), true) => return Err(format!("Widget {n} (\"{id}\") is multi, so its value is a list of option labels.")),
                (Some(v), false) => vec![v],
            };
            for v in starts {
                match v.as_str() {
                    Some(s) if options.iter().any(|o| o.label == s) => {}
                    Some(s) => return Err(format!("Widget {n} (\"{id}\") starts at \"{s}\", which is not one of its options.")),
                    None => return Err(format!("Widget {n} (\"{id}\") starts at {v}; a value is an option's label.")),
                }
            }
        }
        Widget::Select { options, value, .. } => {
            if !(1..=50).contains(&options.len()) {
                return Err(format!("Widget {n} (\"{id}\") has {} options; a select has 1 to 50.", options.len()));
            }
            if let Some(l) = twice(options.iter().map(|o| o.label.as_str()).collect()) {
                return Err(format!("Widget {n} (\"{id}\") has the option \"{l}\" twice. The answer names an option by its label."));
            }
            if let Some(v) = value
                && !options.iter().any(|o| &o.label == v)
            {
                return Err(format!("Widget {n} (\"{id}\") starts at \"{v}\", which is not one of its options."));
            }
        }
        Widget::Checklist { items, .. } => {
            if !(1..=50).contains(&items.len()) {
                return Err(format!("Widget {n} (\"{id}\") has {} items; a checklist has 1 to 50.", items.len()));
            }
            if let Some(i) = twice(items.iter().map(|s| s.id.as_str()).collect()) {
                return Err(format!("Widget {n} (\"{id}\") has the item id \"{i}\" twice. Give each item its own id."));
            }
        }
        Widget::Number { min, max, value, step, .. } => {
            if min > max {
                return Err(format!("Widget {n} (\"{id}\") has min {min} above max {max}. Swap them."));
            }
            if let Some(v) = value
                && (v < min || v > max)
            {
                return Err(format!("Widget {n} (\"{id}\") starts at {v}, outside {min} to {max}."));
            }
            if step.is_some_and(|s| s <= 0.0) {
                return Err(format!("Widget {n} (\"{id}\") has a step that is not above 0."));
            }
        }
        Widget::Text { lines, .. } => {
            if lines.is_some_and(|l| !(1..=20).contains(&l)) {
                return Err(format!("Widget {n} (\"{id}\") has lines outside 1 to 20."));
            }
        }
        Widget::Markdown { .. } | Widget::Toggle { .. } => {}
    }
    Ok(())
}

/// An answer, checked against its form: an object keyed by widget id, each value the shape its
/// widget answers with.
pub fn check_answer(form: &Form, answer: &Value) -> Result<(), String> {
    let Some(obj) = answer.as_object() else { return Err("The answer is a JSON object keyed by widget id.".to_string()) };
    if let Some(k) = obj.keys().find(|k| !form.widgets.iter().any(|w| w.id() == k.as_str())) {
        return Err(format!("The answer has \"{k}\", which is no widget of the form."));
    }
    for w in &form.widgets {
        let id = w.id();
        let v = obj.get(id).filter(|v| !v.is_null());
        if matches!(w, Widget::Markdown { .. }) {
            if v.is_some() {
                return Err(format!("\"{id}\" is text to read, which takes no answer."));
            }
            continue;
        }
        let Some(v) = v else {
            if w.optional() {
                continue;
            }
            return Err(format!("The answer has nothing for \"{id}\", which is not optional."));
        };
        answers(w, v)?;
    }
    Ok(())
}

fn answers(w: &Widget, v: &Value) -> Result<(), String> {
    let id = w.id();
    let picked = |options: &[Opt], other: bool, s: &str| -> Result<(), String> {
        if options.iter().any(|o| o.label == s) || (other && !s.trim().is_empty()) {
            Ok(())
        } else {
            Err(format!("\"{s}\" is not one of the options of \"{id}\"."))
        }
    };
    match w {
        Widget::Markdown { .. } => Ok(()),
        Widget::Choice { options, multi: false, other, .. } => match v.as_str() {
            Some(s) => picked(options, *other, s),
            None => Err(format!("The answer for \"{id}\" is one option's label.")),
        },
        Widget::Choice { options, multi: true, other, optional, .. } => {
            let Some(list) = v.as_array() else { return Err(format!("The answer for \"{id}\" is a list of option labels.")) };
            if list.is_empty() && !optional {
                return Err(format!("The answer has nothing for \"{id}\", which is not optional."));
            }
            for item in list {
                let Some(s) = item.as_str() else { return Err(format!("The answer for \"{id}\" is a list of option labels.")) };
                picked(options, *other, s)?;
            }
            Ok(())
        }
        Widget::Checklist { items, .. } => {
            let shape = || format!("The answer for \"{id}\" lists each of its items once, in the user's order, as {{id, checked, note}}.");
            let Some(list) = v.as_array() else { return Err(shape()) };
            let mut seen: Vec<&str> = Vec::new();
            for item in list {
                let iid = item.get("id").and_then(Value::as_str).ok_or_else(shape)?;
                if !items.iter().any(|s| s.id == iid) || seen.contains(&iid) || !item.get("checked").is_some_and(Value::is_boolean) {
                    return Err(shape());
                }
                if item.get("note").is_some_and(|n| !n.is_string() && !n.is_null()) {
                    return Err(shape());
                }
                seen.push(iid);
            }
            if seen.len() != items.len() {
                return Err(shape());
            }
            Ok(())
        }
        Widget::Text { .. } => v.as_str().map(|_| ()).ok_or_else(|| format!("The answer for \"{id}\" is text.")),
        Widget::Number { min, max, .. } => match v.as_f64() {
            Some(n) if n >= *min && n <= *max => Ok(()),
            Some(n) => Err(format!("The answer for \"{id}\" is {n}, outside {min} to {max}.")),
            None => Err(format!("The answer for \"{id}\" is a number.")),
        },
        Widget::Toggle { .. } => v.as_bool().map(|_| ()).ok_or_else(|| format!("The answer for \"{id}\" is true or false.")),
        Widget::Select { options, .. } => match v.as_str() {
            Some(s) => picked(options, false, s),
            None => Err(format!("The answer for \"{id}\" is one option's label.")),
        },
    }
}

const ANSWER_HEAD: &str = "desl: answer to f-";
const NOTE_HEAD: &str = "desl: the user dismissed f-";
const NOTE_TAIL: &str = " without answering.";

/// The prompt that carries the user's answer to the agent.
pub fn answer_prompt(form_id: &str, title: &str, answer: &Value) -> String {
    format!("desl: answer to {form_id} \"{title}\"\n```json\n{answer}\n```")
}

/// Whether a prompt is an answer desl typed: the page shows the form's card in its place.
pub fn is_form_prompt(text: &str) -> bool {
    text.trim_start().starts_with(ANSWER_HEAD)
}

/// The line put before the next prompt once the user dismissed a form.
pub fn dismissed_note(form_id: &str, title: &str) -> String {
    format!("desl: the user dismissed {form_id} \"{title}\"{NOTE_TAIL}")
}

/// A prompt without the dismissal notes desl put before it.
pub fn strip_notes(text: &str) -> &str {
    let mut rest = text;
    loop {
        let t = rest.trim_start();
        match t.split_once('\n') {
            Some((line, after)) if line.starts_with(NOTE_HEAD) && line.trim_end().ends_with(NOTE_TAIL) => rest = after,
            None if t.starts_with(NOTE_HEAD) && t.trim_end().ends_with(NOTE_TAIL) => return "",
            _ => return t,
        }
    }
}

/// What `show_form` says to the agent.
pub fn shown(form_id: &str) -> String {
    format!("Shown to the user as {form_id}. End your turn now; their answer arrives as your next prompt.")
}

/// `show_form`'s description: every agent reads it each turn, so it stays short.
pub const TOOL_DESCRIPTION: &str = "Put a form in front of the user on desl's page in their browser. Use it when the user \
should edit, review, reorder or compare something, or choose with room for a note: options with their trade-offs, a plan \
to approve step by step, settings to set. For a yes or no, ask a quick question in your own way instead.

It returns at once with the form's id. Then end your turn: the answer arrives later as your next prompt, starting \
`desl: answer to f-…`, with a JSON object keyed by widget id. If the user dismisses the form, a line before their next \
prompt says so.

Widgets by kind, with the answer for each:
- markdown {text}: text to read. No answer.
- choice {label?, options[{label, detail?}], multi?, other?, value?}: the label, or a list with multi; other allows typed text.
- checklist {label?, items[{id, label, detail?, checked?}], reorder?}: [{id, checked, note?}] in the user's order.
- text {label, lines?, placeholder?, value?}: the text.
- number {label, min, max, step?, unit?, value?}: the number.
- toggle {label, value?}: true or false.
- select {label, options[{label, detail?}], value?}: the label.
Every widget has an id, unique in the form, and may be optional. At most 30 widgets. Set value to what is true now \
or what you recommend (choice takes a label, or a list with multi), so the user changes only what differs.";

/// `close_form`'s description.
pub const CLOSE_DESCRIPTION: &str =
    "Withdraw a form you showed with show_form that is no longer needed, by its id. Nothing comes back from the user.";

/// `show_form`'s arguments. Flat, with no `oneOf`, since not every agent reads one.
pub fn input_schema() -> Value {
    let s = |d: &str| json!({ "type": "string", "description": d });
    let options = json!({ "type": "array", "items": { "type": "object", "properties": { "label": { "type": "string" }, "detail": { "type": "string" } }, "required": ["label"] } });
    json!({
        "type": "object",
        "properties": {
            "title": s("The question or task, one line."),
            "send_label": s("The Send button's label."),
            "widgets": { "type": "array", "items": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": KINDS },
                    "id": s("Unique in the form; the answer is keyed by it."),
                    "label": { "type": "string" },
                    "text": s("markdown: the text."),
                    "options": options,
                    "items": { "type": "array", "items": { "type": "object", "properties": {
                        "id": { "type": "string" }, "label": { "type": "string" }, "detail": { "type": "string" }, "checked": { "type": "boolean" } },
                        "required": ["id", "label"] } },
                    "multi": { "type": "boolean" },
                    "other": { "type": "boolean" },
                    "reorder": { "type": "boolean" },
                    "optional": { "type": "boolean" },
                    "lines": { "type": "integer" },
                    "placeholder": { "type": "string" },
                    "min": { "type": "number" },
                    "max": { "type": "number" },
                    "step": { "type": "number" },
                    "unit": { "type": "string" },
                    "value": { "description": "Where it starts: what is true now or what you recommend." }
                },
                "required": ["kind", "id"]
            } }
        },
        "required": ["title", "widgets"]
    })
}

/// `close_form`'s arguments.
pub fn close_schema() -> Value {
    json!({ "type": "object", "properties": { "form_id": { "type": "string" } }, "required": ["form_id"] })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_form_is_checked_widget_by_widget() {
        let ok = json!({"title": "Which milestone next?", "widgets": [
            {"kind": "choice", "id": "next", "options": [{"label": "M4"}, {"label": "M5", "detail": "the biggest"}]},
            {"kind": "text", "id": "note", "label": "Note", "optional": true}]});
        assert_eq!(check(&ok).unwrap().widgets.len(), 2);
        let twice =
            json!({"title": "t", "widgets": [{"kind": "toggle", "id": "a", "label": "A"}, {"kind": "toggle", "id": "a", "label": "B"}]});
        assert_eq!(check(&twice).unwrap_err(), "Widget 2 has the id \"a\", which widget 1 has already. Give each widget its own id.");
        let unknown = json!({"title": "t", "widgets": [{"kind": "slider", "id": "a"}]});
        assert!(
            check(&unknown)
                .unwrap_err()
                .starts_with("Widget 1 has the kind \"slider\", which desl does not know. Use one of: markdown, choice, checklist, text, number, toggle, select."),
            "{}",
            check(&unknown).unwrap_err()
        );
        let many = json!({"title": "t", "widgets": (0..31).map(|i| json!({"kind": "toggle", "id": format!("w{i}"), "label": "x"})).collect::<Vec<_>>()});
        assert_eq!(check(&many).unwrap_err(), "The form has 31 widgets; desl shows at most 30. Split it into two forms.");
        let range = json!({"title": "t", "widgets": [{"kind": "number", "id": "n", "label": "N", "min": 5, "max": 1}]});
        assert_eq!(check(&range).unwrap_err(), "Widget 1 (\"n\") has min 5 above max 1. Swap them.");
        assert!(check(&json!({"title": "", "widgets": []})).unwrap_err().contains("title"));
    }

    #[test]
    fn a_choice_starts_on_its_value() {
        let one = |value: Value| json!({"title": "t", "widgets": [{"kind": "choice", "id": "a", "options": [{"label": "X"}, {"label": "Y"}], "value": value}]});
        assert!(check(&one(json!("Y"))).is_ok());
        let err = check(&one(json!("Z"))).unwrap_err();
        assert!(err.contains("starts at \"Z\", which is not one of its options"), "{err}");
        let many = |value: Value| json!({"title": "t", "widgets": [{"kind": "choice", "id": "a", "multi": true, "options": [{"label": "X"}, {"label": "Y"}], "value": value}]});
        assert!(check(&many(json!(["X", "Y"]))).is_ok());
        let err = check(&many(json!("X"))).unwrap_err();
        assert!(err.contains("a list"), "{err}");
        // The author's note on the first live form: what is true now comes pre-set.
        assert!(TOOL_DESCRIPTION.contains("Set value to what is true now"), "{TOOL_DESCRIPTION}");
    }

    #[test]
    fn a_title_holds_no_control_characters() {
        // The title is typed into the agent's terminal with the answer: a carriage return would
        // send it early, and an escape could end the paste it goes in.
        let err = check(&json!({"title": "a\rb\u{1b}[201~c", "widgets": [{"kind": "toggle", "id": "a", "label": "A"}]})).unwrap_err();
        assert!(err.contains("control"), "{err}");
    }

    #[test]
    fn an_answer_is_checked_against_its_form() {
        let f = check(&json!({"title": "t", "widgets": [
            {"kind": "choice", "id": "next", "options": [{"label": "M4"}, {"label": "M5"}]},
            {"kind": "markdown", "id": "why", "text": "Pick one."},
            {"kind": "text", "id": "note", "label": "Note", "optional": true}]}))
        .unwrap();
        assert!(check_answer(&f, &json!({"next": "M5"})).is_ok(), "an optional widget may be left out, a markdown one has no answer");
        assert_eq!(check_answer(&f, &json!({})).unwrap_err(), "The answer has nothing for \"next\", which is not optional.");
        assert_eq!(check_answer(&f, &json!({"next": "M6"})).unwrap_err(), "\"M6\" is not one of the options of \"next\".");
        assert_eq!(check_answer(&f, &json!({"next": "M5", "x": 1})).unwrap_err(), "The answer has \"x\", which is no widget of the form.");
    }

    #[test]
    fn the_prompts_are_worded_once() {
        let p = answer_prompt("f-4b1e", "Which milestone next?", &json!({"next": "M5"}));
        assert_eq!(p, "desl: answer to f-4b1e \"Which milestone next?\"\n```json\n{\"next\":\"M5\"}\n```");
        assert!(is_form_prompt(&p));
        let note = dismissed_note("f-4b1e", "Which milestone next?");
        assert_eq!(note, "desl: the user dismissed f-4b1e \"Which milestone next?\" without answering.");
        assert_eq!(strip_notes(&format!("{note}\n\nand now the tests")), "and now the tests");
        assert_eq!(shown("f-4b1e"), "Shown to the user as f-4b1e. End your turn now; their answer arrives as your next prompt.");
    }

    #[test]
    fn the_tool_costs_at_most_1500_tokens_a_turn() {
        let size = TOOL_DESCRIPTION.len() + input_schema().to_string().len() + CLOSE_DESCRIPTION.len() + close_schema().to_string().len();
        assert!(size <= 6_000, "{size} bytes");
        assert!(TOOL_DESCRIPTION.contains("ask a quick question in your own way"), "design: quick choices stay in the agent's own dialog");
    }
}
