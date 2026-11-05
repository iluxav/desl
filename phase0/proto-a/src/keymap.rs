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
    Sidebar,
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
    b!(&[shift(KeyCode::Enter), Key { code: KeyCode::Enter, mods: KeyModifiers::ALT }], false, Newline, "Newline in the prompt", Prompt, true),
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
    b!(&[k(KeyCode::Char('o'))], false, OpenFile, "Open the selected file in $EDITOR", Transcript, false),
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
    Binding { keys: &[], prefix: false, action: Action::JumpN, desc: "Jump to thread n in the sidebar", ctx: Ctx::Workspace, built: true, label: Some("ctrl+1…9") },
    b!(&[k(KeyCode::Char('d'))], true, Detach, "Detach; agents keep running", Workspace, true),
    b!(&[k(KeyCode::Char('n'))], true, NewThread, "New thread", Workspace, true),
    b!(&[k(KeyCode::Char('t'))], true, JumpThread, "Jump to any thread", Workspace, true),
    Binding { keys: &[], prefix: true, action: Action::JumpN, desc: "Jump to thread n", ctx: Ctx::Workspace, built: true, label: Some("ctrl+b 1…9") },
    b!(&[k(KeyCode::Tab)], true, NextPane, "Next pane", Workspace, true),
    b!(&[k(KeyCode::Char('b'))], true, ToggleSidebar, "Toggle the sidebar", Workspace, true),
    b!(&[k(KeyCode::Char('%'))], true, SplitRight, "Split pane right", Workspace, false),
    b!(&[k(KeyCode::Char('"'))], true, SplitBelow, "Split pane below", Workspace, false),
    b!(&[k(KeyCode::Char('x'))], true, ClosePane, "Close pane", Workspace, false),
    b!(&[k(KeyCode::Char('z'))], true, Zoom, "Zoom the focused pane", Workspace, false),
    b!(&[k(KeyCode::Char('s'))], true, Switch, "Switch agent (same as /switch)", Workspace, true),
    b!(&[k(KeyCode::Char('f'))], true, Fork, "Fork thread", Workspace, false),
    b!(&[k(KeyCode::Char('?'))], true, Help, "Keys for the focused element", Workspace, true),
];

fn key_matches(key: &Key, ev: &KeyEvent) -> bool {
    match (&key.code, &ev.code) {
        // Shift is already folded into the character for printable keys.
        (KeyCode::Char(a), KeyCode::Char(b)) => {
            a == b && (ev.modifiers & !KeyModifiers::SHIFT) == (key.mods & !KeyModifiers::SHIFT)
        }
        (a, b) => a == b && ev.modifiers == key.mods,
    }
}

pub fn lookup(ev: &KeyEvent, ctx: Ctx, prefixed: bool) -> Option<&'static Binding> {
    KEYMAP
        .iter()
        .filter(|b| b.ctx == ctx && b.prefix == prefixed)
        .find(|b| b.keys.iter().any(|key| key_matches(key, ev)))
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
