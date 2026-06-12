mod app;
mod clipboard;
mod config;
mod event;
mod exec;
mod explorer;
mod layout;
mod pty;
mod ui;

use std::io::stdout;
use std::sync::mpsc;

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::execute;

use crate::app::App;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    if let Some(arg) = args.next() {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("mipoco {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ => {
                println!("mipoco {} — minimal terminal multiplexer", env!("CARGO_PKG_VERSION"));
                println!("usage: mipoco            start (no arguments)");
                println!("       inside the app:   Alt+? keys · Alt+o settings · Alt+q twice quits");
                return Ok(());
            }
        }
    }
    let (config, warn) = config::load();
    let mut terminal = ratatui::init();
    let _ = execute!(stdout(), EnableBracketedPaste, EnableMouseCapture);
    // ratatui::init's panic hook restores raw mode + alt screen; chain bracketed paste
    // off, but only for the main thread — PTY reader threads catch vt100 panics and
    // keep running, so those must not tear down the live terminal.
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some("main") {
            let _ = execute!(stdout(), DisableBracketedPaste, DisableMouseCapture);
            prev_hook(info);
        }
    }));

    let result = run(&mut terminal, config, warn);

    let _ = execute!(stdout(), DisableBracketedPaste, DisableMouseCapture);
    ratatui::restore();
    result
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    config: config::Config,
    warn: Option<String>,
) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    event::spawn_input_thread(tx.clone());

    let size = terminal.size()?;
    let mut app = App::new(config, tx, (size.width, size.height))?;
    app.status_msg = warn;

    loop {
        let size = terminal.size()?;
        app.sync_layout(size.width, size.height);
        terminal.draw(|f| ui::draw(f, &mut app))?;

        let Ok(ev) = rx.recv() else { break };
        app.handle_event(ev);
        // drain pending events so output bursts coalesce into one redraw
        while !app.should_quit
            && let Ok(ev) = rx.try_recv()
        {
            app.handle_event(ev);
        }
        if app.should_quit {
            break;
        }
    }
    Ok(())
}
