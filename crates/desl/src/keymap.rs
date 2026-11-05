//! The single key registry. The help overlay is generated from this table, never hand-written.
//! Keys come from tmux, lazygit, fzf, Claude Code and Codex, as listed in SPEC.md.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ctx {
    Prompt,
    Transcript,
    Sidebar,
    Approval,
    Workspace,
}

impl Ctx {
    pub fn title(self) -> &'static str {
        match self {
            Ctx::Prompt => "Prompt",
            Ctx::Transcript => "Transcript",
            Ctx::Sidebar => "Sidebar",
            Ctx::Approval => "Approval",
            Ctx::Workspace => "Workspace",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Send,
    Newline,
    Interrupt,
    Back,
    Up,
    Down,
    Top,
    Bottom,
    PageUp,
    PageDown,
    Palette,
    FilePicker,
    History,
    JumpThread,
    NewThread,
    NextPane,
    PrevPane,
    Approve,
    Deny,
    Always,
    OpenFile,
    Expand,
    Help,
    Detach,
    SplitRight,
    SplitBelow,
    ClosePane,
    ToggleSidebar,
    Zoom,
    Switch,
    Fork,
    JumpN,
}

pub struct Key {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

const fn k(code: KeyCode) -> Key {
    Key { code, mods: KeyModifiers::NONE }
}
const fn ctrl(c: char) -> Key {
    Key { code: KeyCode::Char(c), mods: KeyModifiers::CONTROL }
}
const fn shift(code: KeyCode) -> Key {
    Key { code, mods: KeyModifiers::SHIFT }
}

pub struct Binding {
    pub keys: &'static [Key],
    /// True when the key follows the tmux prefix (`ctrl+b`).
    pub prefix: bool,
    pub action: Action,
    pub desc: &'static str,
    pub ctx: Ctx,
    /// False while this prototype does not implement it yet; still listed, shown dim.
    pub built: bool,
    /// A display label when the keys cannot be enumerated (`ctrl+1…9`).
    pub label: Option<&'static str>,
}

pub const PREFIX_LABEL: &str = "ctrl+b";

/// `ctrl+1` … `ctrl+9`, when the terminal can report them.
pub fn ctrl_digit(ev: &KeyEvent) -> Option<usize> {
    if let KeyCode::Char(c) = ev.code
        && ev.modifiers.contains(KeyModifiers::CONTROL)
        && let Some(d) = c.to_digit(10)
        && (1..=9).contains(&d)
    {
        return Some(d as usize);
    }
    None
}

pub fn is_prefix(ev: &KeyEvent) -> bool {
    ev.code == KeyCode::Char('b') && ev.modifiers.contains(KeyModifiers::CONTROL)
}

macro_rules! b {
    ($keys:expr, $prefix:expr, $action:ident, $desc:expr, $ctx:ident, $built:expr) => {
        Binding { keys: $keys, prefix: $prefix, action: Action::$action, desc: $desc, ctx: Ctx::$ctx, built: $built, label: None }
    };
}

pub static KEYMAP: &[Binding] = &[
    // Prompt
    b!(&[k(KeyCode::Enter)], false, Send, "Send the prompt", Prompt, true),
    b!(
        &[shift(KeyCode::Enter), Key { code: KeyCode::Enter, mods: KeyModifiers::ALT }],
        false,
        Newline,
        "Newline in the prompt",
        Prompt,
        true
    ),
    b!(&[ctrl('c')], false, Interrupt, "Interrupt the agent; twice clears the prompt", Prompt, true),
    b!(&[k(KeyCode::Esc)], false, Back, "Dismiss, stop following, back out", Prompt, true),
    b!(&[k(KeyCode::Up)], false, Up, "Scroll the transcript up", Prompt, true),
    b!(&[k(KeyCode::Down)], false, Down, "Scroll the transcript down", Prompt, true),
    b!(&[k(KeyCode::PageUp)], false, PageUp, "Page up", Prompt, true),
    b!(&[k(KeyCode::PageDown)], false, PageDown, "Page down", Prompt, true),
    b!(&[k(KeyCode::Char('/'))], false, Palette, "Command palette (prompt empty)", Prompt, true),
    b!(&[k(KeyCode::Char('@'))], false, FilePicker, "File picker into the prompt", Prompt, true),
    b!(&[ctrl('r')], false, History, "Prompt history across threads", Prompt, true),
    // Sidebar
    b!(&[k(KeyCode::Char('j')), k(KeyCode::Down)], false, Down, "Next thread", Sidebar, true),
    b!(&[k(KeyCode::Char('k')), k(KeyCode::Up)], false, Up, "Previous thread", Sidebar, true),
    b!(&[k(KeyCode::Enter)], false, Send, "Open the thread, focus the prompt", Sidebar, true),
    b!(&[k(KeyCode::Esc)], false, Back, "Back to the prompt", Sidebar, true),
    // Transcript
    b!(&[k(KeyCode::Char('j')), k(KeyCode::Down)], false, Down, "Select the next item", Transcript, true),
    b!(&[k(KeyCode::Char('k')), k(KeyCode::Up)], false, Up, "Select the previous item", Transcript, true),
    b!(&[k(KeyCode::Char('g'))], false, Top, "Top", Transcript, true),
    b!(&[k(KeyCode::Char('G'))], false, Bottom, "Bottom, and follow again", Transcript, true),
    b!(&[k(KeyCode::PageUp), ctrl('u')], false, PageUp, "Page up", Transcript, true),
    b!(&[k(KeyCode::PageDown), ctrl('d')], false, PageDown, "Page down", Transcript, true),
    b!(&[k(KeyCode::Esc)], false, Back, "Follow again, back to the prompt", Transcript, true),
    b!(&[k(KeyCode::Enter)], false, Expand, "Expand or collapse the selected tool call or brief", Transcript, true),
    b!(&[k(KeyCode::Char('o'))], false, OpenFile, "Open the selected file in $EDITOR", Transcript, true),
    // Approval
    b!(&[k(KeyCode::Char('y'))], false, Approve, "Allow this once, or answer yes", Approval, true),
    b!(&[k(KeyCode::Char('n'))], false, Deny, "Deny, or answer no", Approval, true),
    b!(&[k(KeyCode::Char('a'))], false, Always, "Always allow this tool in this thread", Approval, true),
    // Workspace
    b!(&[k(KeyCode::Tab)], false, NextPane, "Next pane", Workspace, true),
    b!(&[k(KeyCode::BackTab), shift(KeyCode::BackTab)], false, PrevPane, "Previous pane", Workspace, true),
    b!(&[k(KeyCode::Char('?'))], false, Help, "Keys for the focused element", Workspace, true),
    b!(&[ctrl('t')], false, JumpThread, "Jump to any thread", Workspace, true),
    b!(&[ctrl('n')], false, NewThread, "New thread", Workspace, true),
    Binding {
        keys: &[],
        prefix: false,
        action: Action::JumpN,
        desc: "Jump to thread n in the sidebar",
        ctx: Ctx::Workspace,
        built: true,
        label: Some("ctrl+1…9"),
    },
    b!(&[k(KeyCode::Char('d'))], true, Detach, "Detach; agents keep running", Workspace, true),
    b!(&[k(KeyCode::Char('n'))], true, NewThread, "New thread", Workspace, true),
    b!(&[k(KeyCode::Char('t'))], true, JumpThread, "Jump to any thread", Workspace, true),
    Binding {
        keys: &[],
        prefix: true,
        action: Action::JumpN,
        desc: "Jump to thread n",
        ctx: Ctx::Workspace,
        built: true,
        label: Some("ctrl+b 1…9"),
    },
    b!(&[k(KeyCode::Tab)], true, NextPane, "Next pane", Workspace, true),
    b!(&[k(KeyCode::Char('b'))], true, ToggleSidebar, "Toggle the sidebar", Workspace, true),
    b!(&[k(KeyCode::Char('%'))], true, SplitRight, "Split pane right", Workspace, false),
    b!(&[k(KeyCode::Char('"'))], true, SplitBelow, "Split pane below", Workspace, false),
    b!(&[k(KeyCode::Char('x'))], true, ClosePane, "Close pane", Workspace, false),
    b!(&[k(KeyCode::Char('z'))], true, Zoom, "Zoom the focused pane", Workspace, false),
    b!(&[k(KeyCode::Char('s'))], true, Switch, "Switch agent (same as /switch)", Workspace, true),
    b!(&[k(KeyCode::Char('f'))], true, Fork, "Fork thread (same as /fork)", Workspace, true),
    b!(&[k(KeyCode::Char('?'))], true, Help, "Keys for the focused element", Workspace, true),
];

fn key_matches(key: &Key, ev: &KeyEvent) -> bool {
    match (&key.code, &ev.code) {
        // Shift is already folded into the character for printable keys.
        (KeyCode::Char(a), KeyCode::Char(b)) => a == b && (ev.modifiers & !KeyModifiers::SHIFT) == (key.mods & !KeyModifiers::SHIFT),
        (a, b) => a == b && ev.modifiers == key.mods,
    }
}

pub fn lookup(ev: &KeyEvent, ctx: Ctx, prefixed: bool) -> Option<&'static Binding> {
    KEYMAP.iter().filter(|b| b.ctx == ctx && b.prefix == prefixed).find(|b| b.keys.iter().any(|key| key_matches(key, ev)))
}

pub fn key_label(key: &Key) -> String {
    let mut s = String::new();
    if key.mods.contains(KeyModifiers::CONTROL) {
        s.push_str("ctrl+");
    }
    if key.mods.contains(KeyModifiers::ALT) {
        s.push_str("alt+");
    }
    if key.mods.contains(KeyModifiers::SHIFT) && !matches!(key.code, KeyCode::Char(_) | KeyCode::BackTab) {
        s.push_str("shift+");
    }
    s.push_str(&match key.code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::BackTab => "shift+tab".into(),
        KeyCode::Up => "↑".into(),
        KeyCode::Down => "↓".into(),
        KeyCode::Left => "←".into(),
        KeyCode::Right => "→".into(),
        KeyCode::PageUp => "pgup".into(),
        KeyCode::PageDown => "pgdn".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        other => format!("{other:?}").to_lowercase(),
    });
    s
}

pub fn event_label(ev: &KeyEvent) -> String {
    key_label(&Key { code: ev.code, mods: ev.modifiers })
}

/// "j / k", "ctrl+b d": the label shown in the help overlay and in messages.
pub fn binding_label(b: &Binding) -> String {
    if let Some(l) = b.label {
        return l.to_string();
    }
    let mut keys: Vec<String> = Vec::new();
    for k in b.keys.iter().map(key_label) {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    let joined = keys.join(" / ");
    if b.prefix { format!("{PREFIX_LABEL} {joined}") } else { joined }
}

/// `docs/keys.md`: every key and slash command, from the same tables the help overlay and the
/// palette read. A test keeps the file equal to this.
#[cfg(test)]
pub fn markdown() -> String {
    let mut out = String::from(
        "# Keys and commands\n\nGenerated from the keymap and the command registry in `crates/desl/src`; do not edit by hand.\n\
         `DESL_BLESS=1 cargo test -p desl keys_md` rewrites it. The prefix is `ctrl+b`.\n",
    );
    for ctx in [Ctx::Prompt, Ctx::Transcript, Ctx::Sidebar, Ctx::Approval, Ctx::Workspace] {
        out.push_str(&format!("\n## {}\n\n| Key | Action |\n| --- | --- |\n", ctx.title()));
        for b in KEYMAP.iter().filter(|b| b.ctx == ctx) {
            let label = binding_label(b).replace('|', "\\|");
            let later = if b.built { "" } else { " (not built yet)" };
            out.push_str(&format!("| `{label}` | {}{later} |\n", b.desc));
        }
    }
    out.push_str("\n## Slash commands\n\n| Command | Action |\n| --- | --- |\n");
    for c in crate::commands::COMMANDS {
        let name = if c.args.is_empty() { c.name.to_string() } else { format!("{} {}", c.name, c.args) };
        let later = if c.built { "" } else { " (not built yet)" };
        out.push_str(&format!("| `{}` | {}{later} |\n", name.replace('|', "\\|"), c.desc));
    }
    out.push_str("\nAnything else that starts with `/` is sent to the agent, so its own commands and skills keep working.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn keys_md_is_generated_from_the_keymap() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/keys.md");
        let got = markdown();
        if std::env::var_os("DESL_BLESS").is_some() {
            std::fs::write(&path, &got).unwrap();
        }
        let want = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(got, want, "docs/keys.md is out of date; run DESL_BLESS=1 cargo test -p desl keys_md");
    }

    /// The key table of SPEC.md "Commands, keybindings and CLI": every documented key does what it says.
    #[test]
    fn every_documented_key_is_bound_to_its_action() {
        let none = KeyModifiers::NONE;
        let ctrl = KeyModifiers::CONTROL;
        let cases: &[(KeyCode, KeyModifiers, Ctx, bool, Action)] = &[
            (KeyCode::Enter, none, Ctx::Prompt, false, Action::Send),
            (KeyCode::Enter, none, Ctx::Transcript, false, Action::Expand),
            (KeyCode::Enter, KeyModifiers::SHIFT, Ctx::Prompt, false, Action::Newline),
            (KeyCode::Char('c'), ctrl, Ctx::Prompt, false, Action::Interrupt),
            (KeyCode::Esc, none, Ctx::Prompt, false, Action::Back),
            (KeyCode::Char('j'), none, Ctx::Transcript, false, Action::Down),
            (KeyCode::Char('k'), none, Ctx::Transcript, false, Action::Up),
            (KeyCode::Down, none, Ctx::Sidebar, false, Action::Down),
            (KeyCode::Up, none, Ctx::Sidebar, false, Action::Up),
            (KeyCode::Char('g'), none, Ctx::Transcript, false, Action::Top),
            (KeyCode::Char('G'), KeyModifiers::SHIFT, Ctx::Transcript, false, Action::Bottom),
            (KeyCode::Char('/'), none, Ctx::Prompt, false, Action::Palette),
            (KeyCode::Char('@'), none, Ctx::Prompt, false, Action::FilePicker),
            (KeyCode::Char('r'), ctrl, Ctx::Prompt, false, Action::History),
            (KeyCode::Char('t'), ctrl, Ctx::Workspace, false, Action::JumpThread),
            (KeyCode::Char('n'), ctrl, Ctx::Workspace, false, Action::NewThread),
            (KeyCode::Tab, none, Ctx::Workspace, false, Action::NextPane),
            (KeyCode::BackTab, KeyModifiers::SHIFT, Ctx::Workspace, false, Action::PrevPane),
            (KeyCode::Char('y'), none, Ctx::Approval, false, Action::Approve),
            (KeyCode::Char('n'), none, Ctx::Approval, false, Action::Deny),
            (KeyCode::Char('a'), none, Ctx::Approval, false, Action::Always),
            (KeyCode::Char('o'), none, Ctx::Transcript, false, Action::OpenFile),
            (KeyCode::Char('?'), none, Ctx::Workspace, false, Action::Help),
            (KeyCode::Char('d'), none, Ctx::Workspace, true, Action::Detach),
            (KeyCode::Char('%'), none, Ctx::Workspace, true, Action::SplitRight),
            (KeyCode::Char('"'), none, Ctx::Workspace, true, Action::SplitBelow),
            (KeyCode::Char('x'), none, Ctx::Workspace, true, Action::ClosePane),
            (KeyCode::Char('b'), none, Ctx::Workspace, true, Action::ToggleSidebar),
            (KeyCode::Char('z'), none, Ctx::Workspace, true, Action::Zoom),
            (KeyCode::Char('s'), none, Ctx::Workspace, true, Action::Switch),
            (KeyCode::Char('f'), none, Ctx::Workspace, true, Action::Fork),
        ];
        for (code, mods, ctx, prefixed, action) in cases {
            let found = lookup(&key(*code, *mods), *ctx, *prefixed).map(|b| b.action);
            assert_eq!(found, Some(*action), "{code:?} with {mods:?} in {ctx:?} (prefixed: {prefixed})");
        }
        assert!(is_prefix(&key(KeyCode::Char('b'), ctrl)));
        assert_eq!(ctrl_digit(&key(KeyCode::Char('3'), ctrl)), Some(3));
    }

    #[test]
    fn every_binding_has_a_label_and_a_description_for_the_help_overlay() {
        for b in KEYMAP {
            assert!(!binding_label(b).is_empty() && !b.desc.is_empty(), "{:?}", b.action);
        }
    }
}
