mod app;
mod clipboard;
mod config;
mod event;
mod exec;
mod explorer;
mod layout;
mod notify;
mod pty;
mod setup;
mod ui;
mod update;
mod viewer;

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
            // Claude hook callback: forward an attention event to the running
            // mipoco that spawned this pane, then exit. No TUI.
            "--hook" => return notify::hook_client(args.next()),
            _ => {
                println!("mipoco {} — minimal terminal multiplexer", env!("CARGO_PKG_VERSION"));
                println!("usage: mipoco            start (no arguments)");
                println!("       inside the app:   Alt+? keys · Alt+o settings · Alt+q close pane");
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
    let first_run = !config.setup_complete;
    let check_updates = config.check_updates;
    let mut app = App::new(config, tx.clone(), (size.width, size.height))?;
    app.status_msg = warn;
    // Keep Claude's hooks in sync with the notifications setting.
    if app.config.notifications
        && app.status_msg.is_none()
        && let Err(e) = notify::install_hooks()
    {
        app.status_msg = Some(format!("notify hooks: {e}"));
    }
    if first_run {
        app.open_setup_wizard();
    }
    if check_updates {
        std::thread::spawn(move || {
            if let Some(info) = update::check() {
                let _ = tx.send(event::AppEvent::UpdateChecked(Box::new(info)));
            }
        });
    }
    // Preview helper: `MIPOCO_FAKE_UPDATE=0.9.9 mipoco` seeds a fake update so
    // the banner/overlay can be exercised before any release is published.
    if let Ok(v) = std::env::var("MIPOCO_FAKE_UPDATE") {
        app.update = Some(update::UpdateInfo {
            version: v,
            notes: "## What's new\n\n- in-app update check + changelog\n- first-run setup wizard\n- explorer folder access control".into(),
            release_url: "https://github.com/NFD-Joel/mipoco/releases".into(),
            asset_url: None,
        });
    }

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
