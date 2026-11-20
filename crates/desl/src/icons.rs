//! The marks desl draws besides the agents' logos (SPEC.md "Visual language"). With a Nerd Font,
//! its Material Design icons, one family throughout the screen; without, plain Unicode, as
//! before. A Nerd Font's icon can spill into the cell after it, so every one is followed by a
//! blank cell wherever it is drawn.

use crate::theme;
use desl_ir::Status;

fn pick(nerd: &'static str, text: &'static str) -> &'static str {
    if theme::nerd() { nerd } else { text }
}

/// A branch, and a thread working on one of its own (a worktree).
pub fn branch() -> &'static str {
    pick("\u{f062c}", "⎇")
}

/// Uncommitted paths, before their count.
pub fn changes() -> &'static str {
    pick("\u{f14c9}", "±")
}

/// Before a project's name, in the project's own colour: a repository, or a folder that is not
/// one; without a Nerd Font, a dot.
pub fn project(repo: bool) -> &'static str {
    match (theme::nerd(), repo) {
        (true, true) => "\u{f0ccf}",
        (true, false) => "\u{f024b}",
        (false, _) => "●",
    }
}

/// Which pane of two a thread is in, on its row in the sidebar: the left or right half of the
/// screen, or with the panes stacked, the top or bottom.
pub fn pane(second: bool, stacked: bool) -> &'static str {
    match (theme::nerd(), stacked, second) {
        (true, false, false) => "\u{f10aa}",
        (true, false, true) => "\u{f10ab}",
        (true, true, false) => "\u{f1513}",
        (true, true, true) => "\u{f10a9}",
        (false, false, false) => "◧",
        (false, false, true) => "◨",
        (false, true, false) => "⬒",
        (false, true, true) => "⬓",
    }
}

/// A thread's status, where it is written out: the panel's title and the info box.
pub fn status(s: Status) -> &'static str {
    match (s, theme::nerd()) {
        (Status::Working, true) => "\u{f0996}",
        (Status::Waiting, true) => "\u{f009e}",
        (Status::Idle, true) => "\u{f05e1}",
        (Status::Failed, true) => "\u{f0159}",
        (Status::Detached, true) => "\u{f0338}",
        (s, false) => s.glyph(),
    }
}

/// The threads waiting for the user, in the sidebar's title. Without a Nerd Font, the words.
pub fn waiting() -> Option<&'static str> {
    theme::nerd().then_some("\u{f009e}")
}

/// `±4` without a Nerd Font; with one, its icon and a blank cell before the count.
pub fn changed(n: usize) -> String {
    if theme::nerd() { format!("{} {n}", changes()) } else { format!("{}{n}", changes()) }
}

/// A subagent: running, done, or left unfinished by a stopped turn.
pub fn sub_running() -> &'static str {
    pick("\u{f0996}", "●")
}
pub fn sub_done() -> &'static str {
    pick("\u{f05e0}", "✓")
}
pub fn sub_unfinished() -> &'static str {
    pick("\u{f0130}", "○")
}

/// Before the model in a panel's title.
pub fn model() -> Option<&'static str> {
    theme::nerd().then_some("\u{f061a}")
}

/// A notice in the key line.
pub fn notice() -> &'static str {
    pick("\u{f02fd}", "●")
}

/// A row of the directory finder, by what it found: a project, a recent directory, a repository,
/// a folder, or the path typed.
pub fn finder(note: &str) -> Option<&'static str> {
    if !theme::nerd() {
        return None;
    }
    Some(match note {
        "project" => "\u{f069d}",
        "recent" => "\u{f02da}",
        "git" => "\u{f0ccf}",
        "folder" => "\u{f024b}",
        _ => "\u{f0256}",
    })
}

/// Before a label of the thread's info box (`prefix i`).
pub fn fact(label: &str) -> Option<&'static str> {
    if !theme::nerd() {
        return None;
    }
    Some(match label {
        "Thread" => "\u{f0423}",
        "Title" => "\u{f05f4}",
        "Directory" => "\u{f024b}",
        "Branch" => "\u{f062c}",
        "Worktree" => "\u{f0645}",
        "Forked from" => "\u{f04c1}",
        "Agent" => "\u{f167a}",
        "Permissions" => "\u{f0499}",
        "Subagents" => "\u{f160e}",
        l if l.ends_with(" session") => "\u{f0efe}",
        l if l.ends_with(" model") => "\u{f061a}",
        _ => "\u{f01d8}",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without a Nerd Font (the tests' setting), the marks are the ones desl always drew.
    #[test]
    fn without_a_nerd_font_the_marks_are_plain_unicode() {
        assert_eq!((branch(), changed(4), waiting(), notice()), ("⎇", "±4".to_string(), None, "●"));
        assert_eq!((sub_running(), sub_done(), sub_unfinished()), ("●", "✓", "○"));
        assert_eq!(status(Status::Waiting), "◐");
        assert_eq!((project(true), pane(true, false), pane(false, true)), ("●", "◨", "⬒"));
        assert_eq!((model(), finder("git"), fact("Thread")), (None, None, None));
    }
}
