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

const KINDS: [&str; 10] = ["markdown", "choice", "checklist", "text", "number", "toggle", "select", "preview", "table", "images"];

/// The most pictures one `images` widget shows.
pub const MAX_IMAGES: usize = 12;
/// The most one picture may weigh.
pub const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

/// The most a preview's HTML, or its CSS, may weigh.
const PREVIEW_BYTES: usize = 64 * 1024;
/// The most controls one preview has.
const MAX_CONTROLS: usize = 12;

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
    /// HTML and CSS the agent wrote, drawn in a frame where nothing runs, with controls that set
    /// its CSS variables. The one widget whose look comes from the agent: the look is what is
    /// being tuned.
    Preview {
        id: String,
        #[serde(default)]
        label: Option<String>,
        html: String,
        #[serde(default)]
        css: Option<String>,
        #[serde(default)]
        controls: Vec<Control>,
        #[serde(default)]
        optional: bool,
    },
    /// Rows to read, with cells to edit in the columns that say how.
    Table {
        id: String,
        #[serde(default)]
        label: Option<String>,
        columns: Vec<Column>,
        /// Each row an object: its `id`, and a cell per column id.
        rows: Vec<serde_json::Map<String, Value>>,
        #[serde(default)]
        optional: bool,
    },
    /// Pictures of the project to pick from, named by path. desl reads them, inside the thread's
    /// directory only.
    Images {
        id: String,
        #[serde(default)]
        label: Option<String>,
        images: Vec<Pic>,
        #[serde(default)]
        multi: bool,
        /// Where it starts: a path, or a list of them with `multi`.
        #[serde(default)]
        value: Option<Value>,
        #[serde(default)]
        optional: bool,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Pic {
    /// A file inside the thread's directory, relative to it or absolute.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

/// Every picture a form names, by its path as the agent gave it.
pub fn image_paths(form: &Form) -> Vec<&str> {
    form.widgets
        .iter()
        .flat_map(|w| match w {
            Widget::Images { images, .. } => images.iter().map(|p| p.path.as_str()).collect(),
            _ => Vec::new(),
        })
        .collect()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// How its cells are edited; read-only without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<Edit>,
    /// A select column's choices.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Edit {
    Text,
    Select,
    Check,
}

/// Whether `v` fits a cell of column `c`, as given or as edited. A sentence otherwise.
fn cell_fits(c: &Column, v: &Value) -> Result<(), String> {
    match c.edit {
        _ if v.is_null() => Ok(()),
        Some(Edit::Check) if !v.is_boolean() => Err(format!("a cell of \"{}\" is true or false.", c.id)),
        Some(Edit::Select) => match v.as_str() {
            Some(s) if s.is_empty() || c.options.iter().any(|o| o == s) => Ok(()),
            _ => Err(format!("{v} is not one of its options, in column \"{}\".", c.id)),
        },
        Some(Edit::Text) if !v.is_string() => Err(format!("a cell of \"{}\" is text.", c.id)),
        _ if v.is_object() || v.is_array() => Err(format!("a cell of \"{}\" is text, a number, true or false.", c.id)),
        _ => Ok(()),
    }
}

/// One of a preview's controls: it sets the CSS variable `var` on the frame's root.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
    /// A number; the variable gets it with its `unit`.
    Range {
        var: String,
        #[serde(default)]
        label: Option<String>,
        min: f64,
        max: f64,
        #[serde(default)]
        step: Option<f64>,
        #[serde(default)]
        unit: Option<String>,
        #[serde(default)]
        value: Option<f64>,
    },
    /// A colour as `#rrggbb`.
    #[serde(alias = "color")]
    Colour {
        var: String,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        value: Option<String>,
    },
    /// One of a few CSS values.
    Select {
        var: String,
        #[serde(default)]
        label: Option<String>,
        options: Vec<String>,
        #[serde(default)]
        value: Option<String>,
    },
}

impl Control {
    pub fn var(&self) -> &str {
        match self {
            Control::Range { var, .. } | Control::Colour { var, .. } | Control::Select { var, .. } => var,
        }
    }
}

fn is_colour(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
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
            | Widget::Select { id, .. }
            | Widget::Preview { id, .. }
            | Widget::Table { id, .. }
            | Widget::Images { id, .. } => id,
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
            | Widget::Select { optional, .. }
            | Widget::Preview { optional, .. }
            | Widget::Table { optional, .. }
            | Widget::Images { optional, .. } => *optional,
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
        Widget::Preview { html, css, controls, .. } => {
            if html.len() > PREVIEW_BYTES || css.as_ref().is_some_and(|c| c.len() > PREVIEW_BYTES) {
                return Err(format!("Widget {n} (\"{id}\") has HTML or CSS over 64 KB. Show a smaller part of the page."));
            }
            if controls.len() > MAX_CONTROLS {
                return Err(format!("Widget {n} (\"{id}\") has {} controls; a preview has at most 12 controls.", controls.len()));
            }
            if let Some(v) = twice(controls.iter().map(Control::var).collect()) {
                return Err(format!("Widget {n} (\"{id}\") sets \"{v}\" twice. Give each control its own variable."));
            }
            for c in controls {
                control_sound(c).map_err(|why| format!("Widget {n} (\"{id}\"), control \"{}\": {why}", c.var()))?;
            }
        }
        Widget::Table { columns, rows, .. } => {
            if !(1..=12).contains(&columns.len()) {
                return Err(format!("Widget {n} (\"{id}\") has {} columns; a table has 1 to 12 columns.", columns.len()));
            }
            if let Some(c) = twice(columns.iter().map(|c| c.id.as_str()).collect()) {
                return Err(format!("Widget {n} (\"{id}\") has the column \"{c}\" twice. Give each column its own id."));
            }
            for c in columns {
                if c.id.trim().is_empty() {
                    return Err(format!("Widget {n} (\"{id}\") has a column with no id."));
                }
                if c.id == "id" && c.edit.is_some() {
                    return Err(format!("Widget {n} (\"{id}\"): the column \"id\" shows each row's id, which cannot be edited."));
                }
                match c.edit {
                    Some(Edit::Select) if !(1..=50).contains(&c.options.len()) => {
                        return Err(format!("Widget {n} (\"{id}\"): the select column \"{}\" has 1 to 50 options.", c.id));
                    }
                    Some(Edit::Select) => {}
                    _ if !c.options.is_empty() => {
                        return Err(format!("Widget {n} (\"{id}\"): only a select column has options, not \"{}\".", c.id));
                    }
                    _ => {}
                }
            }
            if !(1..=200).contains(&rows.len()) {
                return Err(format!("Widget {n} (\"{id}\") has {} rows; a table has 1 to 200 rows.", rows.len()));
            }
            let mut seen: Vec<&str> = Vec::new();
            for (r, row) in rows.iter().enumerate() {
                let r = r + 1;
                let Some(rid) = row.get("id").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) else {
                    return Err(format!("Widget {n} (\"{id}\"): row {r} has no id. Give each row one: the answer names rows by it."));
                };
                if seen.contains(&rid) {
                    return Err(format!("Widget {n} (\"{id}\") has the row \"{rid}\" twice. Give each row its own id."));
                }
                seen.push(rid);
                for (k, v) in row {
                    if k == "id" {
                        continue;
                    }
                    let Some(c) = columns.iter().find(|c| &c.id == k) else {
                        return Err(format!("Widget {n} (\"{id}\"): row \"{rid}\" has \"{k}\", which is no column."));
                    };
                    cell_fits(c, v).map_err(|why| format!("Widget {n} (\"{id}\"), row \"{rid}\": {why}"))?;
                }
            }
        }
        Widget::Images { images, multi, value, .. } => {
            if !(1..=MAX_IMAGES).contains(&images.len()) {
                return Err(format!("Widget {n} (\"{id}\") has {} images; it shows 1 to 12 images.", images.len()));
            }
            if let Some(p) = twice(images.iter().map(|p| p.path.as_str()).collect()) {
                return Err(format!("Widget {n} (\"{id}\") has \"{p}\" twice. The answer names an image by its path."));
            }
            if images.iter().any(|p| p.path.trim().is_empty() || p.path.chars().any(char::is_control)) {
                return Err(format!("Widget {n} (\"{id}\") has an image with no path, or one with a control character."));
            }
            let starts: Vec<&Value> = match (value, multi) {
                (None, _) => Vec::new(),
                (Some(Value::Array(list)), true) => list.iter().collect(),
                (Some(_), true) => return Err(format!("Widget {n} (\"{id}\") is multi, so its value is a list of paths.")),
                (Some(v), false) => vec![v],
            };
            if let Some(v) = starts.iter().find(|v| !v.as_str().is_some_and(|s| images.iter().any(|p| p.path == s))) {
                return Err(format!("Widget {n} (\"{id}\") starts at {v}, which is not one of its images."));
            }
        }
        Widget::Markdown { .. } | Widget::Toggle { .. } => {}
    }
    Ok(())
}

fn control_sound(c: &Control) -> Result<(), String> {
    let var = c.var();
    if !var.starts_with("--") || var.len() < 3 {
        return Err("a variable's name starts with --, as in --accent.".to_string());
    }
    if var.len() > 64 || !var[2..].chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_') {
        return Err("a variable's name holds only letters, digits, - and _, at most 64 of them.".to_string());
    }
    match c {
        Control::Range { min, max, step, unit, value, .. } => {
            if min > max {
                return Err(format!("min {min} is above max {max}. Swap them."));
            }
            if let Some(v) = value
                && (v < min || v > max)
            {
                return Err(format!("it starts at {v}, outside {min} to {max}."));
            }
            if step.is_some_and(|s| s <= 0.0) {
                return Err("its step is not above 0.".to_string());
            }
            if unit.as_ref().is_some_and(|u| u.len() > 8 || !u.chars().all(|ch| ch.is_ascii_alphabetic() || ch == '%')) {
                return Err("its unit is a CSS unit, such as px, rem or %.".to_string());
            }
        }
        Control::Colour { value, .. } => {
            if let Some(v) = value
                && !is_colour(v)
            {
                return Err(format!("it starts at \"{v}\"; a colour is #rrggbb."));
            }
        }
        Control::Select { options, value, .. } => {
            if !(1..=20).contains(&options.len()) {
                return Err(format!("it has {} options; a select has 1 to 20 options.", options.len()));
            }
            if options.iter().any(|o| o.len() > 200 || o.chars().any(char::is_control)) {
                return Err("an option is a CSS value of at most 200 characters on one line.".to_string());
            }
            if let Some(v) = value
                && !options.contains(v)
            {
                return Err(format!("it starts at \"{v}\", which is not one of its options."));
            }
        }
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
        Widget::Preview { controls, .. } => {
            let Some(obj) = v.as_object() else {
                return Err(format!("The answer for \"{id}\" is an object keyed by each control's variable."));
            };
            if let Some(k) = obj.keys().find(|k| !controls.iter().any(|c| c.var() == k.as_str())) {
                return Err(format!("The answer for \"{id}\" has \"{k}\", which is no control of it."));
            }
            for c in controls {
                let var = c.var();
                let Some(given) = obj.get(var) else { return Err(format!("The answer for \"{id}\" has nothing for \"{var}\".")) };
                match c {
                    Control::Range { min, max, .. } => match given.as_f64() {
                        Some(x) if x >= *min && x <= *max => {}
                        Some(x) => return Err(format!("\"{var}\" of \"{id}\" is {x}, outside {min} to {max}.")),
                        None => return Err(format!("\"{var}\" of \"{id}\" is a number.")),
                    },
                    Control::Colour { .. } => {
                        if !given.as_str().is_some_and(is_colour) {
                            return Err(format!("\"{var}\" of \"{id}\" is a colour as #rrggbb."));
                        }
                    }
                    Control::Select { options, .. } => {
                        if !given.as_str().is_some_and(|s| options.iter().any(|o| o == s)) {
                            return Err(format!("\"{var}\" of \"{id}\" is not one of the options it has."));
                        }
                    }
                }
            }
            Ok(())
        }
        Widget::Images { images, multi, optional, .. } => {
            let one = |s: &str| {
                if images.iter().any(|p| p.path == s) { Ok(()) } else { Err(format!("\"{s}\" is not one of the images of \"{id}\".")) }
            };
            if !multi {
                return v.as_str().map_or_else(|| Err(format!("The answer for \"{id}\" is one image's path.")), one);
            }
            let Some(list) = v.as_array() else { return Err(format!("The answer for \"{id}\" is a list of image paths.")) };
            if list.is_empty() && !optional {
                return Err(format!("The answer has nothing for \"{id}\", which is not optional."));
            }
            for item in list {
                item.as_str().map_or_else(|| Err(format!("The answer for \"{id}\" is a list of image paths.")), one)?;
            }
            Ok(())
        }
        Widget::Table { columns, rows, .. } => {
            let shape = || format!("The answer for \"{id}\" lists the changed rows, each as {{id, <column>: value}}.");
            let Some(list) = v.as_array() else { return Err(shape()) };
            let mut seen: Vec<&str> = Vec::new();
            for item in list {
                let Some(obj) = item.as_object() else { return Err(shape()) };
                let Some(rid) = obj.get("id").and_then(Value::as_str) else { return Err(shape()) };
                if !rows.iter().any(|r| r.get("id").and_then(Value::as_str) == Some(rid)) {
                    return Err(format!("The answer for \"{id}\" changes the row \"{rid}\", which the table does not have."));
                }
                if seen.contains(&rid) {
                    return Err(format!("The answer for \"{id}\" has the row \"{rid}\" twice."));
                }
                seen.push(rid);
                for (k, cell) in obj {
                    if k == "id" {
                        continue;
                    }
                    let Some(c) = columns.iter().find(|c| &c.id == k && c.edit.is_some()) else {
                        return Err(format!("The answer for \"{id}\" sets \"{k}\" of row \"{rid}\", which cannot be edited."));
                    };
                    if cell.is_null() {
                        return Err(format!("The answer for \"{id}\" sets \"{k}\" of row \"{rid}\" to nothing."));
                    }
                    cell_fits(c, cell).map_err(|why| format!("The answer for \"{id}\", row \"{rid}\": {why}"))?;
                }
            }
            Ok(())
        }
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
- preview {label?, html, css?, controls[{kind: range|colour|select, var --x, label?, min, max, step?, unit?, \
options[css values], value?}]}: your HTML and CSS drawn without scripts; controls set CSS variables on its root. \
{var: value}.
- table {label?, columns[{id, label?, edit?: text|select|check, options?}], rows[{id, <column id>: value}]}: the \
changed rows as [{id, <column>: new value}], only the changed cells.
- images {label?, images[{path, caption?}], multi?, value?}: pictures of the project (PNG, JPEG, GIF, WebP, SVG; \
inside your working directory) to pick from. The path, or a list with multi.
Every widget has an id, unique in the form, and may be optional. At most 30 widgets. Set value to what is true now \
or what you recommend (choice takes a label, or a list with multi), so the user changes only what differs.";

/// `close_form`'s description.
pub const CLOSE_DESCRIPTION: &str =
    "Withdraw a form you showed with show_form that is no longer needed, by its id. Nothing comes back from the user.";

/// `show_form`'s arguments. Flat, with no `oneOf`, since not every agent reads one.
pub fn input_schema() -> Value {
    let s = |d: &str| json!({ "type": "string", "description": d });
    let options = json!({ "type": "array", "items": { "type": "object", "properties": { "label": { "type": "string" }, "detail": { "type": "string" } }, "required": ["label"] } });
    let controls = json!({ "type": "array", "items": { "type": "object", "properties": {
        "kind": { "type": "string", "enum": ["range", "colour", "select"] }, "var": { "type": "string" }, "label": { "type": "string" },
        "min": { "type": "number" }, "max": { "type": "number" }, "step": { "type": "number" }, "unit": { "type": "string" },
        "options": { "type": "array", "items": { "type": "string" } }, "value": {} },
        "required": ["kind", "var"] } });
    let columns = json!({ "type": "array", "items": { "type": "object", "properties": {
        "id": { "type": "string" }, "label": { "type": "string" }, "edit": { "type": "string", "enum": ["text", "select", "check"] },
        "options": { "type": "array", "items": { "type": "string" } } }, "required": ["id"] } });
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
                    "value": { "description": "Where it starts: what is true now or what you recommend." },
                    "html": s("preview: the HTML to show; no scripts run."),
                    "css": s("preview: its CSS; var(--x) for each control."),
                    "controls": controls,
                    "columns": columns,
                    "images": { "type": "array", "items": { "type": "object", "properties": {
                        "path": { "type": "string" }, "caption": { "type": "string" } }, "required": ["path"] } },
                    "rows": { "type": "array", "items": { "type": "object", "description": "table: {id, <column id>: value}" } }
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
                .starts_with("Widget 1 has the kind \"slider\", which desl does not know. Use one of: markdown, choice, checklist, text, number, toggle, select, "),
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

    fn preview(controls: Value) -> Value {
        json!({"title": "t", "widgets": [{"kind": "preview", "id": "look", "html": "<button>Go</button>", "css": "button { background: var(--accent); }", "controls": controls}]})
    }

    #[test]
    fn a_preview_is_checked_control_by_control() {
        let all = preview(json!([
            {"kind": "range", "var": "--radius", "label": "Corners", "min": 0, "max": 24, "unit": "px", "value": 8},
            {"kind": "colour", "var": "--accent", "value": "#3355ff"},
            {"kind": "color", "var": "--ink", "value": "#000000"},
            {"kind": "select", "var": "--font", "options": ["system-ui", "Georgia, serif"], "value": "system-ui"}
        ]));
        let f = check(&all).unwrap();
        assert!(matches!(&f.widgets[0], Widget::Preview { controls, .. } if controls.len() == 4));
        let refused = |c: Value, says: &str| {
            let err = check(&preview(json!([c]))).unwrap_err();
            assert!(err.contains(says), "{err}");
        };
        refused(json!({"kind": "colour", "var": "accent"}), "starts with --");
        refused(json!({"kind": "colour", "var": "--a;b"}), "letters, digits");
        refused(json!({"kind": "range", "var": "--r", "min": 0, "max": 10, "value": 11}), "outside 0 to 10");
        refused(json!({"kind": "colour", "var": "--c", "value": "blue"}), "#rrggbb");
        refused(json!({"kind": "select", "var": "--f", "options": ["a", "b"], "value": "c"}), "not one of its options");
        refused(json!({"kind": "select", "var": "--f", "options": []}), "1 to 20 options");
        let twice = preview(json!([{"kind": "colour", "var": "--c"}, {"kind": "colour", "var": "--c"}]));
        assert!(check(&twice).unwrap_err().contains("--c\" twice"));
        let many = preview(Value::Array((0..13).map(|i| json!({"kind": "colour", "var": format!("--c{i}")})).collect()));
        assert!(check(&many).unwrap_err().contains("at most 12 controls"));
        let big = json!({"title": "t", "widgets": [{"kind": "preview", "id": "look", "html": "x".repeat(70_000)}]});
        assert!(check(&big).unwrap_err().contains("64 KB"));
    }

    #[test]
    fn a_preview_answer_holds_each_control() {
        let f = check(&preview(json!([
            {"kind": "range", "var": "--radius", "min": 0, "max": 24, "unit": "px"},
            {"kind": "colour", "var": "--accent"},
            {"kind": "select", "var": "--font", "options": ["serif", "sans-serif"]}
        ])))
        .unwrap();
        let answer = |v: Value| check_answer(&f, &json!({ "look": v }));
        assert!(answer(json!({"--radius": 8, "--accent": "#3355FF", "--font": "serif"})).is_ok());
        assert!(answer(json!({"--radius": 8, "--accent": "#3355ff", "--font": "serif", "--x": 1})).unwrap_err().contains("--x"));
        assert!(answer(json!({"--radius": 30, "--accent": "#3355ff", "--font": "serif"})).unwrap_err().contains("outside 0 to 24"));
        assert!(answer(json!({"--radius": 8, "--accent": "red", "--font": "serif"})).unwrap_err().contains("#rrggbb"));
        assert!(answer(json!({"--radius": 8, "--accent": "#3355ff", "--font": "mono"})).unwrap_err().contains("not one of the options"));
        assert!(answer(json!({"--radius": 8, "--accent": "#3355ff"})).unwrap_err().contains("--font"));
    }

    fn table(columns: Value, rows: Value) -> Value {
        json!({"title": "t", "widgets": [{"kind": "table", "id": "tasks", "columns": columns, "rows": rows}]})
    }

    fn columns() -> Value {
        json!([
            {"id": "title", "label": "Task"},
            {"id": "prio", "label": "Priority", "edit": "select", "options": ["high", "low"]},
            {"id": "done", "label": "Done", "edit": "check"},
            {"id": "note", "label": "Note", "edit": "text"}
        ])
    }

    #[test]
    fn a_table_is_checked_row_by_row() {
        let rows =
            json!([{"id": "r1", "title": "Fix login", "prio": "high", "done": false}, {"id": "r2", "title": "Docs", "note": "later"}]);
        assert!(check(&table(columns(), rows)).is_ok());
        let refused = |cols: Value, rows: Value, says: &str| {
            let err = check(&table(cols, rows)).unwrap_err();
            assert!(err.contains(says), "{err}");
        };
        let one = json!([{"id": "r1"}]);
        refused(json!([]), one.clone(), "1 to 12 columns");
        refused(Value::Array((0..13).map(|i| json!({"id": format!("c{i}")})).collect()), one.clone(), "1 to 12 columns");
        refused(json!([{"id": "a"}, {"id": "a"}]), one.clone(), "column \"a\" twice");
        refused(json!([{"id": "p", "edit": "select"}]), one.clone(), "1 to 50 options");
        refused(json!([{"id": "id", "edit": "text"}]), one.clone(), "row's id");
        refused(columns(), json!([]), "1 to 200 rows");
        refused(columns(), Value::Array((0..201).map(|i| json!({"id": format!("r{i}")})).collect()), "1 to 200 rows");
        refused(columns(), json!([{"title": "no id"}]), "has no id");
        refused(columns(), json!([{"id": "r1"}, {"id": "r1"}]), "\"r1\" twice");
        refused(columns(), json!([{"id": "r1", "owner": "me"}]), "\"owner\", which is no column");
        refused(columns(), json!([{"id": "r1", "title": {"a": 1}}]), "text, a number, true or false");
        refused(columns(), json!([{"id": "r1", "done": "yes"}]), "true or false");
        refused(columns(), json!([{"id": "r1", "prio": "mid"}]), "not one of its options");
    }

    #[test]
    fn a_table_answer_lists_the_changed_cells() {
        let f =
            check(&table(columns(), json!([{"id": "r1", "title": "Fix login", "prio": "high"}, {"id": "r2", "title": "Docs"}]))).unwrap();
        let answer = |v: Value| check_answer(&f, &json!({ "tasks": v }));
        assert!(answer(json!([{"id": "r1", "prio": "low", "done": true}, {"id": "r2", "note": "today"}])).is_ok());
        assert!(answer(json!([])).is_ok(), "nothing changed");
        assert!(answer(json!([{"id": "r9", "done": true}])).unwrap_err().contains("\"r9\""));
        assert!(answer(json!([{"id": "r1", "done": true}, {"id": "r1", "done": false}])).unwrap_err().contains("twice"));
        assert!(answer(json!([{"id": "r1", "title": "Renamed"}])).unwrap_err().contains("cannot be edited"));
        assert!(answer(json!([{"id": "r1", "done": "yes"}])).unwrap_err().contains("true or false"));
        assert!(answer(json!([{"id": "r1", "prio": "mid"}])).unwrap_err().contains("not one of its options"));
        assert!(answer(json!([{"id": "r1", "note": 3}])).unwrap_err().contains("text"));
    }

    #[test]
    fn images_are_checked_and_answered_by_path() {
        let pics = |multi: bool, images: Value| json!({"title": "t", "widgets": [{"kind": "images", "id": "logo", "multi": multi, "images": images}]});
        let two = json!([{"path": "art/a.png", "caption": "Round"}, {"path": "art/b.svg"}]);
        let f = check(&pics(false, two.clone())).unwrap();
        assert_eq!(image_paths(&f), ["art/a.png", "art/b.svg"]);
        assert!(check(&pics(false, json!([]))).unwrap_err().contains("1 to 12 images"));
        assert!(
            check(&pics(false, Value::Array((0..13).map(|i| json!({"path": format!("{i}.png")})).collect())))
                .unwrap_err()
                .contains("1 to 12 images")
        );
        assert!(check(&pics(false, json!([{"path": "a.png"}, {"path": "a.png"}]))).unwrap_err().contains("\"a.png\" twice"));
        assert!(check_answer(&f, &json!({"logo": "art/b.svg"})).is_ok());
        assert!(check_answer(&f, &json!({"logo": "art/c.png"})).unwrap_err().contains("not one of the images"));
        let many = check(&pics(true, two)).unwrap();
        assert!(check_answer(&many, &json!({"logo": ["art/a.png", "art/b.svg"]})).is_ok());
        assert!(check_answer(&many, &json!({"logo": "art/a.png"})).unwrap_err().contains("a list"));
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
