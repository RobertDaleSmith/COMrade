use std::io;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use anyhow::Result;
use comrade_core::DaemonClient;
use comrade_protocol::{Command, SerialConfig};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc as tokio_mpsc;

use super::app::{App, AppEvent};
use super::ui;

/// Entry point for the TUI mode.
///
/// Takes ownership of the tokio runtime handle to spawn async bridges.
pub fn run_tui(
    rt: &tokio::runtime::Runtime,
    port: String,
    config: SerialConfig,
) -> Result<()> {
    // Connect to the daemon (or spawn one).
    let client = rt.block_on(async {
        DaemonClient::connect_or_spawn(&port, &config).await
    })?;

    // Subscribe to engine events.
    let mut event_rx = client.subscribe();

    // Unified event channel (std sync_channel for the blocking main loop).
    let (app_tx, app_rx) = std_mpsc::sync_channel::<AppEvent>(256);

    // Command forwarding channel: TUI (sync) → tokio task → daemon client.
    let (cmd_tx, mut cmd_fwd_rx) = tokio_mpsc::unbounded_channel::<Command>();
    rt.spawn(async move {
        while let Some(cmd) = cmd_fwd_rx.recv().await {
            if client.send_command(cmd).await.is_err() {
                break;
            }
        }
    });

    // Bridge 1: Engine events → AppEvent channel.
    let tx1 = app_tx.clone();
    rt.spawn(async move {
        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    if tx1.send(AppEvent::Engine(event)).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // Bridge 2: Tick timer → AppEvent channel.
    let tx2 = app_tx.clone();
    rt.spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        loop {
            interval.tick().await;
            if tx2.send(AppEvent::Tick).is_err() {
                break;
            }
        }
    });

    // Bridge 3: Crossterm events → AppEvent channel (OS thread).
    let tx3 = app_tx;
    std::thread::spawn(move || {
        loop {
            // Poll with a short timeout so the thread can notice when the channel is dropped.
            if crossterm::event::poll(Duration::from_millis(50)).unwrap_or(false) {
                if let Ok(event) = crossterm::event::read() {
                    if tx3.send(AppEvent::Terminal(event)).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Set up terminal.
    install_panic_hook();
    enable_raw_mode()?;
    crossterm::execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut app = App::new(cmd_tx, port);

    // Main loop: blocking recv → update → draw.
    loop {
        // Draw first, then wait for events.
        terminal.draw(|f| ui::draw(f, &app))?;

        if app.should_quit {
            break;
        }

        match app_rx.recv_timeout(Duration::from_millis(16)) {
            Ok(event) => {
                match event {
                    AppEvent::Terminal(ev) => app.handle_terminal_event(ev),
                    AppEvent::Engine(ev) => app.handle_engine_event(ev),
                    AppEvent::Tick => app.handle_tick(),
                }
                // Drain any additional pending events before redrawing.
                while let Ok(event) = app_rx.try_recv() {
                    match event {
                        AppEvent::Terminal(ev) => app.handle_terminal_event(ev),
                        AppEvent::Engine(ev) => app.handle_engine_event(ev),
                        AppEvent::Tick => app.handle_tick(),
                    }
                    if app.should_quit {
                        break;
                    }
                }
            }
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                // No events, just redraw (handles cursor blink via tick).
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }

    // Restore terminal.
    restore_terminal();
    Ok(())
}

fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        original_hook(info);
    }));
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = crossterm::execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
}
