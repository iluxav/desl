//! What the shared chrome needs to know, independent of which prototype is drawing it.

use crate::app::Notice;
use crate::git::GitInfo;
use crate::keymap::{Action, Ctx};
use crate::model::Thread;

pub struct ChromeView<'a> {
    pub project: &'a str,
    pub threads: &'a [Thread],
    pub current: usize,
    pub sidebar_focused: bool,
    pub notice: Option<&'a Notice>,
    pub git: &'a GitInfo,
    pub clock: &'a str,
    pub prefix_pending: bool,
    /// The focused element's key context, for the help overlay.
    pub focused_ctx: Ctx,
    pub approval_pending: bool,
    /// The status bar's hint, e.g. `ctrl+b ? help`.
    pub hint: &'a str,
    /// Actions this prototype has not built even though the shared keymap marks them built.
    pub not_built: &'a [Action],
}

impl ChromeView<'_> {
    pub fn thread(&self) -> &Thread {
        &self.threads[self.current.min(self.threads.len().saturating_sub(1))]
    }
}
