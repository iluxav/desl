//! What the chrome (sidebar, status bar, help) needs to know to draw itself.

use crate::app::Notice;
use crate::keymap::Ctx;
use crate::model::Thread;
use desl_ir::Project;

pub struct ChromeView<'a> {
    pub projects: &'a [Project],
    /// In sidebar order.
    pub threads: &'a [Thread],
    pub current: usize,
    pub sidebar_focused: bool,
    pub notice: Option<&'a Notice>,
    pub clock: &'a str,
    pub prefix_pending: bool,
    /// The focused element's key context, for the help overlay.
    pub focused_ctx: Ctx,
    pub approval_pending: bool,
    /// The status bar's hint, e.g. `ctrl+b ? help`.
    pub hint: &'a str,
}

impl ChromeView<'_> {
    pub fn thread(&self) -> &Thread {
        &self.threads[self.current.min(self.threads.len().saturating_sub(1))]
    }
}
