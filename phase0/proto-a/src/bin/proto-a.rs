//! desl phase 0, prototype A: the rendered experience. Throwaway by design (SPEC.md "Roadmap").
//!
//! `proto-a`            attach to this project's workspace (starts the host if needed)
//! `proto-a status`     the sidebar as text
//! `proto-a stop`       stop the host for this project
//! `proto-a --host DIR` run the host (started for you by the client)


use anyhow::Result;
use proto_a::app::{App, Msg};
use proto_a::log::{Command, Frame};
use proto_a::{host, link, model, ui, workspace};
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::supports_keyboard_enhancement;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir()?;
    match args.first().map(String::as_str) {
        Some("--host") => {
            let dir = args.get(1).map(PathBuf::from).unwrap_or(cwd);
            return host::run(dir);
        }
        Some("stop") => {
            if link::connect(&cwd, false)?.is_none() {
                println!("No desl host is running for {}.", cwd.display());
            } else {
                link::one_shot(&cwd, &Command::Shutdown, false)?;
                println!("Stopped the desl host for {}. Its log is kept.", cwd.display());
            }
            return Ok(());
        }
        Some("status") => return status(cwd),
        Some(other) if other != "attach" => {
            eprintln!("Unknown command `{other}`. Run with no arguments to attach, or `status` or `stop`.");
            std::process::exit(2);
        }
        _ => {}
    }

    let (tx, rx) = mpsc::channel::<Msg>();
    let host_link = link::HostLink::attach(&cwd, tx.clone())?;
    let mut app = App::new(cwd, host_link);

    let mut terminal = ratatui::init();
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    let mut out = io::stdout();
    if enhanced {
        let _ = execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
    }
    let _ = execute!(out, EnableBracketedPaste);

    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = event::read() {
                if tx.send(Msg::Term(ev)).is_err() {
                    break;
                }
            }
        });
    }

    let result = run(&mut terminal, &mut app, rx);

    let _ = execute!(out, DisableBracketedPaste);
    if enhanced {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    ratatui::restore();

    if let Some(msg) = app.farewell.take() {
        println!("{msg}");
    }
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App, rx: mpsc::Receiver<Msg>) -> Result<()> {
    terminal.draw(|f| ui::draw(f, app))?;
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
        app.pulse();
        if app.dirty {
            app.dirty = false;
            terminal.draw(|f| ui::draw(f, app))?;
        }
    }
    Ok(())
}

/// `proto-a status`: the sidebar as text, for scripts and for a glance from another shell.
fn status(cwd: PathBuf) -> Result<()> {
    let Some(Frame::Snapshot { entries, .. }) = link::one_shot(&cwd, &Command::Attach, true)? else {
        println!("No desl host is running for {}. Run the prototype here to start one.", cwd.display());
        return Ok(());
    };
    let mut ws = workspace::Workspace::new(cwd);
    for e in &entries {
        ws.apply(e);
    }
    for t in &ws.threads {
        let name = if t.name.is_empty() { "new" } else { &t.name };
        println!("{} {}  {:<12} {:<8} {}", t.status.glyph(), t.id, name, t.status.word(), t.agent.name());
    }
    let waiting: Vec<&str> = ws.threads.iter().filter(|t| t.status == model::Status::Waiting).map(|t| t.id.as_str()).collect();
    if !waiting.is_empty() {
        println!("waiting: {}", waiting.join(" "));
    }
    Ok(())
}
