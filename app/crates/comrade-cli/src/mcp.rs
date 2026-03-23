use std::collections::VecDeque;
use std::sync::Arc;

use chrono::Local;
use comrade_core::{enumerate_devices, Engine};
use comrade_protocol::{Command, Event, SerialConfig};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler, ServiceExt};
use tokio::sync::Mutex;

// ── Log buffer ───────────────────────────────────────────────────────

const MAX_LOG_ENTRIES: usize = 10_000;

#[derive(Debug, Clone)]
struct LogEntry {
    timestamp: String,
    kind: String,
    text: String,
}

impl LogEntry {
    fn format(&self) -> String {
        format!("[{}] [{}] {}", self.timestamp, self.kind, self.text)
    }
}

struct LogBuffer {
    entries: VecDeque<LogEntry>,
}

impl LogBuffer {
    fn new() -> Self {
        Self {
            entries: VecDeque::with_capacity(MAX_LOG_ENTRIES),
        }
    }

    fn push(&mut self, entry: LogEntry) {
        if self.entries.len() >= MAX_LOG_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    fn tail(&self, n: usize) -> Vec<&LogEntry> {
        let skip = self.entries.len().saturating_sub(n);
        self.entries.iter().skip(skip).collect()
    }

    fn since(&self, ts: &str) -> Vec<&LogEntry> {
        self.entries.iter().filter(|e| e.timestamp.as_str() > ts).collect()
    }

    fn search(&self, pattern: &str, max: usize) -> Vec<&LogEntry> {
        let re = regex::Regex::new(pattern).ok();
        self.entries
            .iter()
            .filter(|e| match &re {
                Some(re) => re.is_match(&e.text),
                None => e.text.contains(pattern),
            })
            .rev()
            .take(max)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

// ── Connection state ─────────────────────────────────────────────────

struct ConnectionState {
    engine: Option<Engine>,
    port: Option<String>,
    baud: Option<u32>,
    connected: bool,
    rx_bytes: u64,
    log: LogBuffer,
}

impl ConnectionState {
    fn new() -> Self {
        Self {
            engine: None,
            port: None,
            baud: None,
            connected: false,
            rx_bytes: 0,
            log: LogBuffer::new(),
        }
    }
}

type State = Arc<Mutex<ConnectionState>>;

// ── Parameter structs ────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GetLogsParams {
    #[schemars(description = "Number of recent log entries (default 100, max 5000)")]
    count: Option<usize>,
    #[schemars(description = "Only return entries after this timestamp")]
    since: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SearchLogsParams {
    #[schemars(description = "Regex or substring to search")]
    pattern: String,
    #[schemars(description = "Max results (default 100)")]
    max_results: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ConnectParams {
    #[schemars(description = "Serial port path (e.g. /dev/cu.usbmodem101)")]
    port: String,
    #[schemars(description = "Baud rate (default 115200)")]
    baud: Option<u32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SendParams {
    #[schemars(description = "Text to send (newline appended automatically)")]
    text: String,
}

// ── MCP handler ──────────────────────────────────────────────────────

#[derive(Clone)]
struct ComradeMcp {
    state: State,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl ComradeMcp {
    fn new(state: State) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "List available serial and HID devices.")]
    fn list_devices(&self) -> Result<CallToolResult, McpError> {
        let devices = enumerate_devices()
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let filtered: Vec<_> = devices
            .into_iter()
            .filter(|d| {
                if let Some(ref sp) = d.serial_path {
                    if sp == "/dev/cu.debug-console" || sp == "/dev/cu.Bluetooth-Incoming-Port" {
                        return false;
                    }
                }
                if d.kind == comrade_protocol::DeviceKind::Hid && d.vid == Some(0x05AC) {
                    return false;
                }
                true
            })
            .collect();

        let json = serde_json::to_string_pretty(&filtered).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    #[tool(description = "Connect to a serial device. Disconnects any existing connection first.")]
    async fn connect(
        &self,
        Parameters(params): Parameters<ConnectParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut state = self.state.lock().await;

        // Disconnect existing.
        if let Some(ref engine) = state.engine {
            let _ = engine.send(Command::Shutdown).await;
        }
        state.engine = None;
        state.connected = false;
        state.rx_bytes = 0;

        let baud = params.baud.unwrap_or(115200);
        let config = SerialConfig {
            baud_rate: baud,
            ..SerialConfig::default()
        };

        let engine = Engine::spawn();
        engine
            .send(Command::Connect {
                port: params.port.clone(),
                config,
            })
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        state.port = Some(params.port.clone());
        state.baud = Some(baud);
        state.engine = Some(engine);

        // Spawn a background task to drain events into the log buffer.
        let state_clone = self.state.clone();
        tokio::spawn(async move {
            drain_events(state_clone).await;
        });

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Connected to {} at {} baud",
            params.port, baud
        ))]))
    }

    #[tool(description = "Disconnect from the current device.")]
    async fn disconnect(&self) -> Result<CallToolResult, McpError> {
        let mut state = self.state.lock().await;
        if let Some(ref engine) = state.engine {
            let _ = engine.send(Command::Shutdown).await;
        }
        state.engine = None;
        state.connected = false;
        let port = state.port.take().unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Disconnected from {port}"
        ))]))
    }

    #[tool(description = "Get connection status.")]
    async fn get_status(&self) -> Result<CallToolResult, McpError> {
        let state = self.state.lock().await;
        let status = serde_json::json!({
            "connected": state.connected,
            "port": state.port,
            "baud": state.baud,
            "rx_bytes": state.rx_bytes,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&status).unwrap_or_default(),
        )]))
    }

    #[tool(description = "Get recent log entries from the connected device.")]
    async fn get_logs(
        &self,
        Parameters(params): Parameters<GetLogsParams>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state.lock().await;
        let entries = if let Some(ref since) = params.since {
            state.log.since(since)
        } else {
            state.log.tail(params.count.unwrap_or(100).min(5000))
        };

        if entries.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No log entries.",
            )]));
        }

        let text: String = entries.iter().map(|e| e.format()).collect::<Vec<_>>().join("\n");
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(description = "Search log entries by regex or substring.")]
    async fn search_logs(
        &self,
        Parameters(params): Parameters<SearchLogsParams>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state.lock().await;
        let max = params.max_results.unwrap_or(100).min(5000);
        let entries = state.log.search(&params.pattern, max);

        if entries.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No matches for '{}'.",
                params.pattern
            ))]));
        }

        let text: String = entries.iter().map(|e| e.format()).collect::<Vec<_>>().join("\n");
        Ok(CallToolResult::success(vec![Content::text(format!(
            "{} match(es):\n{}",
            entries.len(),
            text
        ))]))
    }

    #[tool(description = "Send text to the connected serial device. A newline is appended automatically.")]
    async fn send_serial(
        &self,
        Parameters(params): Parameters<SendParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut state = self.state.lock().await;
        let sender = state
            .engine
            .as_ref()
            .map(|e| e.cmd_sender())
            .ok_or_else(|| McpError::invalid_request("Not connected".to_string(), None))?;

        state.log.push(LogEntry {
            timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
            kind: "sent".to_string(),
            text: params.text.clone(),
        });

        drop(state);

        let mut data = params.text.into_bytes();
        data.push(b'\n');
        sender
            .send(Command::Send { data })
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text("Sent.")]))
    }

    #[tool(description = "Clear all log entries.")]
    async fn clear_logs(&self) -> Result<CallToolResult, McpError> {
        let mut state = self.state.lock().await;
        state.log.clear();
        Ok(CallToolResult::success(vec![Content::text("Logs cleared.")]))
    }
}

#[tool_handler]
impl ServerHandler for ComradeMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "comrade",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_protocol_version(ProtocolVersion::V_2025_03_26)
            .with_instructions(
                "COMrade serial device monitor. Use list_devices to see ports, \
                 connect to open a serial connection, get_logs to read output, \
                 send_serial to send commands, get_status to check connection, \
                 and disconnect to release the port."
                    .to_string(),
            )
    }
}

// ── Event drain task ─────────────────────────────────────────────────

async fn drain_events(state: State) {
    // Get a subscriber to the engine's event broadcast.
    let mut event_rx = {
        let state = state.lock().await;
        match &state.engine {
            Some(engine) => engine.subscribe(),
            None => return,
        }
    };

    let mut line_buf = String::new();

    loop {
        match tokio::time::timeout(
            std::time::Duration::from_millis(100),
            event_rx.recv(),
        )
        .await
        {
            Ok(Ok(Event::Data { bytes, .. })) => {
                let mut state = state.lock().await;
                state.rx_bytes += bytes.len() as u64;

                // Simple line assembly: split on \n, buffer partials.
                let text = String::from_utf8_lossy(&bytes);
                for ch in text.chars() {
                    if ch == '\n' {
                        let line = line_buf.trim_end_matches('\r').to_string();
                        if !line.is_empty() {
                            state.log.push(LogEntry {
                                timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
                                kind: "received".to_string(),
                                text: line,
                            });
                        }
                        line_buf.clear();
                    } else {
                        line_buf.push(ch);
                    }
                }
            }
            Ok(Ok(Event::Connected { port, config, .. })) => {
                let mut state = state.lock().await;
                state.connected = true;
                state.log.push(LogEntry {
                    timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
                    kind: "system".to_string(),
                    text: format!("Connected to {} at {} baud", port, config.baud_rate),
                });
            }
            Ok(Ok(Event::Disconnected { reason, .. })) => {
                let mut state = state.lock().await;
                state.connected = false;
                // Flush partial line.
                if !line_buf.is_empty() {
                    state.log.push(LogEntry {
                        timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
                        kind: "received".to_string(),
                        text: std::mem::take(&mut line_buf),
                    });
                }
                state.log.push(LogEntry {
                    timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
                    kind: "system".to_string(),
                    text: format!("Disconnected: {reason}"),
                });
                return;
            }
            Ok(Ok(Event::Error { message, .. })) => {
                let mut state = state.lock().await;
                state.log.push(LogEntry {
                    timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
                    kind: "error".to_string(),
                    text: message,
                });
            }
            Ok(Ok(Event::Shutdown)) | Ok(Err(_)) => return,
            Ok(Ok(_)) => {}
            Err(_timeout) => {
                // Flush partial line on timeout.
                if !line_buf.is_empty() {
                    let mut state = state.lock().await;
                    let text = std::mem::take(&mut line_buf);
                    state.log.push(LogEntry {
                        timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
                        kind: "received".to_string(),
                        text,
                    });
                }
                // Check if engine is still alive.
                let state = state.lock().await;
                if state.engine.is_none() {
                    return;
                }
            }
        }
    }
}

// ── Entry point ──────────────────────────────────────────────────────

pub async fn run_mcp() -> anyhow::Result<()> {
    let state = Arc::new(Mutex::new(ConnectionState::new()));
    let service = ComradeMcp::new(state);

    let transport = rmcp::transport::io::stdio();
    let server = service.serve(transport).await?;
    server.waiting().await?;
    Ok(())
}
