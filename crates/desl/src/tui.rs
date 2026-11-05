//! The terminal loop: one redraw per batch of events, the terminal left as it was found.

use crate::app::{App, Msg};
use crate::conn::Conn;
use crate::ui;
use anyhow::Result;
use desl_proto::{ServerFrame, read_frame};
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::supports_keyboard_enhancement;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

struct Screen {
    terminal: ratatui::DefaultTerminal,
    enhanced: bool,
}

impl Screen {
    fn enter() -> Screen {
        let terminal = ratatui::init();
        let enhanced = supports_keyboard_enhancement().unwrap_or(false);
        let mut out = io::stdout();
        if enhanced {
            let _ = execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
        }
        let _ = execute!(out, EnableBracketedPaste);
        Screen { terminal, enhanced }
    }

    fn leave(&mut self) {
        let mut out = io::stdout();
        let _ = execute!(out, DisableBracketedPaste);
        if self.enhanced {
            let _ = execute!(out, PopKeyboardEnhancementFlags);
        }
        ratatui::restore();
    }
}

/// `$EDITOR` may carry arguments (`code -w`).
fn run_editor(path: &Path) -> Result<(), String> {
    let editor = std::env::var("EDITOR").unwrap_or_default();
    let mut words = editor.split_whitespace();
    let Some(bin) = words.next() else { return Err("Set $EDITOR to open files from here.".into()) };
    match std::process::Command::new(bin).args(words).arg(path).status() {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("Could not run {bin} ({e}). Check $EDITOR.")),
    }
}

/// Attaches to the workspace and runs until the user detaches. Returns the line to print after
/// the screen is restored, and whether the session ended in an error.
pub fn run(conn: Conn, cwd: PathBuf) -> Result<Option<String>> {
    let (link, mut reader) = conn.split();
    let (tx, rx) = mpsc::channel::<Msg>();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(Some(frame)) = read_frame::<_, ServerFrame>(&mut reader) {
                if tx.send(Msg::Daemon(frame)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Msg::DaemonGone);
        });
    }
    let mut app = App::new(cwd, link);
    let mut screen = Screen::enter();
    // While an editor owns the terminal, nothing here may read from it.
    let paused = Arc::new(AtomicBool::new(false));
    {
        let tx = tx.clone();
        let paused = paused.clone();
        std::thread::spawn(move || {
            loop {
                if paused.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(40));
                    continue;
                }
                match event::poll(Duration::from_millis(40)) {
                    Ok(true) => match event::read() {
                        Ok(ev) => {
                            if tx.send(Msg::Term(ev)).is_err() {
                                return;
                            }
                        }
                        Err(_) => return,
                    },
                    Ok(false) => {}
                    Err(_) => return,
                }
            }
        });
    }

    let result = (|| -> Result<()> {
        screen.terminal.draw(|f| ui::draw(f, &mut app))?;
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(msg) => app.handle(msg),
                Err(mpsc::RecvTimeoutError::Timeout) => app.tick(),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            // Everything that arrived while we were busy lands in the same frame: one redraw per batch.
            while let Ok(msg) = rx.try_recv() {
                app.handle(msg);
            }
            if app.should_quit {
                break;
            }
            if let Some(path) = app.open_in_editor.take() {
                paused.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(90));
                screen.leave();
                let outcome = run_editor(&path);
                screen = Screen::enter();
                paused.store(false, Ordering::SeqCst);
                screen.terminal.clear()?;
                if let Err(sentence) = outcome {
                    app.error(sentence);
                }
                app.dirty = true;
            }
            app.pulse();
            if app.dirty {
                app.dirty = false;
                screen.terminal.draw(|f| ui::draw(f, &mut app))?;
            }
        }
        Ok(())
    })();
    paused.store(true, Ordering::SeqCst);
    screen.leave();
    // Give the detach's last command a moment to reach the daemon before the socket closes.
    std::thread::sleep(Duration::from_millis(30));
    result?;
    Ok(app.farewell.take())
}
