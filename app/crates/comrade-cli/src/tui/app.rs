use chrono::Local;
use comrade_protocol::{Command, Event, SerialConfig};
use tokio::sync::mpsc as tokio_mpsc;

use super::input::InputState;
use super::line_assembler::{LineAssembler, LineKind, LogLine};

/// Events funneled into the main TUI loop.
pub enum AppEvent {
    /// A crossterm terminal event (key press, mouse, resize).
    Terminal(crossterm::event::Event),
    /// An engine event.
    Engine(Event),
    /// Periodic tick for cursor blink.
    Tick,
}

/// Connection status displayed in the status bar.
#[derive(Clone)]
pub enum ConnStatus {
    Connecting,
    Connected { port: String, config: SerialConfig },
    Disconnected { reason: String },
}

/// Application state.
pub struct App {
    /// Log lines (scrollback buffer).
    pub lines: Vec<LogLine>,
    /// Scroll offset from the bottom (0 = auto-scroll to latest).
    pub scroll_offset: usize,
    /// Connection status.
    pub status: ConnStatus,
    /// Total bytes received.
    pub rx_bytes: u64,
    /// Text input state.
    pub input: InputState,
    /// Line assembler for incoming data.
    pub assembler: LineAssembler,
    /// Channel to send commands back to the engine.
    pub cmd_tx: tokio_mpsc::UnboundedSender<Command>,
    /// Whether the app should quit.
    pub should_quit: bool,
    /// Port path we're connecting to (for display before connected).
    pub port_path: String,
    /// Cursor blink state (toggles on tick).
    pub cursor_visible: bool,
    /// Tick counter for cursor blink.
    tick_count: u32,
}

impl App {
    pub fn new(cmd_tx: tokio_mpsc::UnboundedSender<Command>, port_path: String) -> Self {
        Self {
            lines: Vec::new(),
            scroll_offset: 0,
            status: ConnStatus::Connecting,
            rx_bytes: 0,
            input: InputState::new(),
            assembler: LineAssembler::new(),
            cmd_tx,
            should_quit: false,
            port_path,
            cursor_visible: true,
            tick_count: 0,
        }
    }

    /// Add a system message to the log.
    pub fn push_system(&mut self, msg: &str) {
        self.lines.push(LogLine {
            timestamp: Local::now(),
            text: msg.to_string(),
            kind: LineKind::System,
        });
    }

    /// Handle an engine event.
    pub fn handle_engine_event(&mut self, event: Event) {
        match event {
            Event::Connected { port, config, .. } => {
                self.status = ConnStatus::Connected {
                    port: port.clone(),
                    config: config.clone(),
                };
                self.push_system(&format!(
                    "Connected to {} at {} baud",
                    port, config.baud_rate
                ));
            }
            Event::Data { bytes, .. } => {
                self.rx_bytes += bytes.len() as u64;
                let new_lines = self.assembler.feed(&bytes, LineKind::Received);
                // If we're auto-scrolling, stay at bottom.
                let was_at_bottom = self.scroll_offset == 0;
                self.lines.extend(new_lines);
                if was_at_bottom {
                    self.scroll_offset = 0;
                }
            }
            Event::Disconnected { reason, .. } => {
                // Flush any partial line.
                if let Some(line) = self.assembler.flush(LineKind::Received) {
                    self.lines.push(line);
                }
                self.status = ConnStatus::Disconnected {
                    reason: reason.clone(),
                };
                self.push_system(&format!("Disconnected: {reason}"));
            }
            Event::Reconnecting { attempt, .. } => {
                self.push_system(&format!("Reconnecting (attempt {attempt})..."));
            }
            Event::Error { message, .. } => {
                self.push_system(&format!("Error: {message}"));
            }
            Event::Shutdown => {
                self.should_quit = true;
            }
            _ => {}
        }
    }

    /// Handle a terminal input event.
    pub fn handle_terminal_event(&mut self, event: crossterm::event::Event) {
        use crossterm::event::{Event as CEvent, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

        match event {
            CEvent::Key(KeyEvent {
                code, modifiers, ..
            }) => {
                match (code, modifiers) {
                    (KeyCode::Char('c'), KeyModifiers::CONTROL)
                    | (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
                        self.should_quit = true;
                        let _ = self.cmd_tx.send(Command::Shutdown);
                    }
                    (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                        self.lines.clear();
                        self.scroll_offset = 0;
                    }
                    (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                        self.input.clear();
                    }
                    (KeyCode::Enter, _) => {
                        let text = self.input.submit();
                        if !text.is_empty() {
                            // Display the sent text in the log.
                            self.lines.push(LogLine {
                                timestamp: Local::now(),
                                text: text.clone(),
                                kind: LineKind::Sent,
                            });
                            // Send to device with newline.
                            let mut data = text.into_bytes();
                            data.push(b'\n');
                            let _ = self.cmd_tx.send(Command::Send { data });
                            // Auto-scroll to bottom when sending.
                            self.scroll_offset = 0;
                        }
                    }
                    (KeyCode::Backspace, _) => {
                        self.input.backspace();
                    }
                    (KeyCode::Delete, _) => {
                        self.input.delete();
                    }
                    (KeyCode::Left, _) => {
                        self.input.move_left();
                    }
                    (KeyCode::Right, _) => {
                        self.input.move_right();
                    }
                    (KeyCode::Home, _) => {
                        self.input.home();
                    }
                    (KeyCode::End, _) => {
                        self.input.end();
                    }
                    (KeyCode::Up, _) => {
                        self.input.history_up();
                    }
                    (KeyCode::Down, _) => {
                        self.input.history_down();
                    }
                    (KeyCode::PageUp, _) => {
                        self.scroll_up(10);
                    }
                    (KeyCode::PageDown, _) => {
                        self.scroll_down(10);
                    }
                    (KeyCode::Esc, _) => {
                        // Jump to bottom.
                        self.scroll_offset = 0;
                    }
                    (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                        self.input.insert(ch);
                    }
                    _ => {}
                }
            }
            CEvent::Mouse(MouseEvent { kind, .. }) => match kind {
                MouseEventKind::ScrollUp => {
                    self.scroll_up(3);
                }
                MouseEventKind::ScrollDown => {
                    self.scroll_down(3);
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// Handle a tick event (cursor blink).
    pub fn handle_tick(&mut self) {
        self.tick_count += 1;
        // Blink every 2 ticks (500ms at 250ms tick rate).
        if self.tick_count.is_multiple_of(2) {
            self.cursor_visible = !self.cursor_visible;
        }
    }

    fn scroll_up(&mut self, amount: usize) {
        let max = self.lines.len().saturating_sub(1);
        self.scroll_offset = (self.scroll_offset + amount).min(max);
    }

    fn scroll_down(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }
}
