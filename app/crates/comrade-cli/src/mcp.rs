//! MCP server for Claude Code integration.
//!
//! `comrade --mcp` operates in one of two modes:
//!
//! 1. **Bridge** — GUI is running on port 9712. The CLI proxies stdio
//!    JSON-RPC to the GUI's HTTP MCP server. Claude interacts with the
//!    app's live tabs.
//!
//! 2. **Headless** — GUI is not running. The CLI starts its own serial
//!    Engine AND an HTTP MCP server on port 9712. The GUI can open later
//!    and connect to the CLI's MCP server to view the same data.
//!
//! Either way, port 9712 is the shared MCP endpoint. Whoever starts
//! first owns the serial connection and serves MCP. The other connects.

use std::collections::VecDeque;
use std::io::{self, BufRead, Write};
use std::sync::Arc;

use anyhow::Result;
use chrono::Local;
use comrade_core::{enumerate_devices, Engine};
use comrade_protocol::{Command, Event, SerialConfig};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};
use tokio::sync::Mutex;
use tracing::{info, warn};

const MCP_URL: &str = "http://127.0.0.1:9712/mcp";
const MCP_PORT: &str = "127.0.0.1:9712";

// ═══════════════════════════════════════════════════════════════════════
// Entry point
// ═══════════════════════════════════════════════════════════════════════

pub async fn run_mcp() -> Result<()> {
    if is_mcp_reachable().await {
        eprintln!("COMrade detected on port 9712, bridging...");
        run_bridge().await
    } else {
        eprintln!("Starting COMrade headless MCP server on port 9712...");
        run_headless().await
    }
}

async fn is_mcp_reachable() -> bool {
    reqwest::Client::new()
        .post(MCP_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body("{}")
        .send()
        .await
        .is_ok()
}

// ═══════════════════════════════════════════════════════════════════════
// Bridge mode — proxy stdio to running GUI/CLI MCP server
// ═══════════════════════════════════════════════════════════════════════

async fn run_bridge() -> Result<()> {
    let client = reqwest::Client::new();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }

        let response = client
            .post(MCP_URL)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(line)
            .send()
            .await;

        match response {
            Ok(resp) => {
                let body = resp.text().await.unwrap_or_default();
                if !body.is_empty() {
                    writeln!(stdout, "{body}")?;
                    stdout.flush()?;
                }
            }
            Err(e) => {
                let err = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": { "code": -32603, "message": format!("Bridge error: {e}") }
                });
                writeln!(stdout, "{err}")?;
                stdout.flush()?;
            }
        }
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════
// Headless mode — own Engine + HTTP MCP server + stdio proxy
// ═══════════════════════════════════════════════════════════════════════

const MAX_LOG: usize = 10_000;

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

pub(crate) struct ConnState {
    engine: Option<Engine>,
    port: Option<String>,
    baud: Option<u32>,
    connected: bool,
    rx_bytes: u64,
    log: VecDeque<LogEntry>,
}

type SharedState = Arc<Mutex<ConnState>>;

fn now() -> String {
    Local::now().format("%H:%M:%S%.3f").to_string()
}

fn push_log(log: &mut VecDeque<LogEntry>, kind: &str, text: &str) {
    if log.len() >= MAX_LOG {
        log.pop_front();
    }
    log.push_back(LogEntry {
        timestamp: now(),
        kind: kind.to_string(),
        text: text.to_string(),
    });
}

async fn run_headless() -> Result<()> {
    let state: SharedState = Arc::new(Mutex::new(ConnState {
        engine: None,
        port: None,
        baud: None,
        connected: false,
        rx_bytes: 0,
        log: VecDeque::with_capacity(MAX_LOG),
    }));

    // Start HTTP MCP server on port 9712 in background.
    let http_state = state.clone();
    tokio::spawn(async move {
        start_http_mcp(http_state).await;
    });

    // Run stdio proxy to our own HTTP server (so Claude gets stdio transport).
    // Wait for our HTTP server to be ready.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    run_bridge().await
}

// ═══════════════════════════════════════════════════════════════════════
// HTTP MCP server (shared between headless CLI and GUI)
// ═══════════════════════════════════════════════════════════════════════

#[derive(Clone)]
struct HeadlessMcp {
    state: SharedState,
    tool_router: ToolRouter<Self>,
}

// ── Parameter structs ────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ConnectParams {
    #[schemars(description = "Serial port path (e.g. /dev/cu.usbmodem101)")]
    port: String,
    #[schemars(description = "Baud rate (default 115200)")]
    baud: Option<u32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GetLogsParams {
    #[schemars(description = "Number of recent entries (default 100, max 5000)")]
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
struct SendParams {
    #[schemars(description = "Text to send (newline appended automatically)")]
    text: String,
}

#[tool_router]
impl HeadlessMcp {
    fn new(state: SharedState) -> Self {
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
                    if sp == "/dev/cu.debug-console"
                        || sp == "/dev/cu.Bluetooth-Incoming-Port"
                    {
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
        let mut s = self.state.lock().await;

        if let Some(ref engine) = s.engine {
            let _ = engine.send(Command::Shutdown).await;
        }
        s.engine = None;
        s.connected = false;
        s.rx_bytes = 0;

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

        s.port = Some(params.port.clone());
        s.baud = Some(baud);
        s.engine = Some(engine);

        let state_clone = self.state.clone();
        tokio::spawn(async move {
            drain_events(state_clone).await;
        });

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Connected to {} at {} baud",
            params.port, baud
        ))]))
    }

    #[tool(description = "Disconnect from the current device, releasing the port.")]
    async fn disconnect(&self) -> Result<CallToolResult, McpError> {
        let mut s = self.state.lock().await;
        if let Some(ref engine) = s.engine {
            let _ = engine.send(Command::Shutdown).await;
        }
        s.engine = None;
        s.connected = false;
        let port = s.port.take().unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Disconnected from {port}"
        ))]))
    }

    #[tool(description = "Get connection status.")]
    async fn get_status(&self) -> Result<CallToolResult, McpError> {
        let s = self.state.lock().await;
        let json = serde_json::json!({
            "connected": s.connected,
            "port": s.port,
            "baud": s.baud,
            "rx_bytes": s.rx_bytes,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json).unwrap_or_default(),
        )]))
    }

    #[tool(description = "Get recent log entries from the connected device.")]
    async fn get_logs(
        &self,
        Parameters(params): Parameters<GetLogsParams>,
    ) -> Result<CallToolResult, McpError> {
        let s = self.state.lock().await;
        let entries: Vec<&LogEntry> =
            if let Some(ref since) = params.since {
                s.log.iter().filter(|e| e.timestamp.as_str() > since.as_str()).collect()
            } else {
                let count = params.count.unwrap_or(100).min(5000);
                let skip = s.log.len().saturating_sub(count);
                s.log.iter().skip(skip).collect()
            };

        if entries.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No log entries.",
            )]));
        }
        let text = entries.iter().map(|e| e.format()).collect::<Vec<_>>().join("\n");
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(description = "Search log entries by regex or substring.")]
    async fn search_logs(
        &self,
        Parameters(params): Parameters<SearchLogsParams>,
    ) -> Result<CallToolResult, McpError> {
        let s = self.state.lock().await;
        let max = params.max_results.unwrap_or(100).min(5000);
        let re = regex::Regex::new(&params.pattern).ok();
        let entries: Vec<&LogEntry> = s
            .log
            .iter()
            .filter(|e| match &re {
                Some(re) => re.is_match(&e.text),
                None => e.text.contains(&params.pattern),
            })
            .rev()
            .take(max)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        if entries.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No matches for '{}'.",
                params.pattern
            ))]));
        }
        let text = entries.iter().map(|e| e.format()).collect::<Vec<_>>().join("\n");
        Ok(CallToolResult::success(vec![Content::text(format!(
            "{} match(es):\n{}",
            entries.len(),
            text
        ))]))
    }

    #[tool(description = "Send text to the connected serial device. Newline appended automatically.")]
    async fn send_serial(
        &self,
        Parameters(params): Parameters<SendParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut s = self.state.lock().await;
        let sender = s
            .engine
            .as_ref()
            .map(|e| e.cmd_sender())
            .ok_or_else(|| McpError::invalid_request("Not connected".to_string(), None))?;

        push_log(&mut s.log, "sent", &params.text);
        drop(s);

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
        self.state.lock().await.log.clear();
        Ok(CallToolResult::success(vec![Content::text(
            "Logs cleared.",
        )]))
    }
}

#[tool_handler]
impl ServerHandler for HeadlessMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "comrade",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_protocol_version(ProtocolVersion::V_2025_03_26)
            .with_instructions(
                "COMrade serial monitor (headless). Tools: list_devices, connect, \
                 disconnect, get_status, get_logs, search_logs, send_serial, clear_logs."
                    .to_string(),
            )
    }
}

async fn start_http_mcp(state: SharedState) {
    let ct = tokio_util::sync::CancellationToken::new();

    let service = StreamableHttpService::new(
        move || Ok(HeadlessMcp::new(state.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig {
            stateful_mode: false,
            json_response: true,
            cancellation_token: ct.child_token(),
            ..Default::default()
        },
    );

    let router = axum::Router::new().nest_service("/mcp", service);

    match tokio::net::TcpListener::bind(MCP_PORT).await {
        Ok(listener) => {
            info!("Headless MCP server listening on http://{MCP_PORT}/mcp");
            if let Err(e) = axum::serve(listener, router).await {
                warn!("MCP server error: {e}");
            }
        }
        Err(e) => {
            warn!("Failed to bind {MCP_PORT}: {e}");
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Event drain — streams Engine events into the shared log buffer
// ═══════════════════════════════════════════════════════════════════════

async fn drain_events(state: SharedState) {
    let mut event_rx = {
        let s = state.lock().await;
        match &s.engine {
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
                let mut s = state.lock().await;
                s.rx_bytes += bytes.len() as u64;
                let text = String::from_utf8_lossy(&bytes);
                for ch in text.chars() {
                    if ch == '\n' {
                        let line = line_buf.trim_end_matches('\r').to_string();
                        if !line.is_empty() {
                            push_log(&mut s.log, "received", &line);
                        }
                        line_buf.clear();
                    } else {
                        line_buf.push(ch);
                    }
                }
            }
            Ok(Ok(Event::Connected { port, config, .. })) => {
                let mut s = state.lock().await;
                s.connected = true;
                push_log(
                    &mut s.log,
                    "system",
                    &format!("Connected to {} at {} baud", port, config.baud_rate),
                );
            }
            Ok(Ok(Event::Disconnected { reason, .. })) => {
                let mut s = state.lock().await;
                s.connected = false;
                if !line_buf.is_empty() {
                    push_log(&mut s.log, "received", &std::mem::take(&mut line_buf));
                }
                push_log(&mut s.log, "system", &format!("Disconnected: {reason}"));
                return;
            }
            Ok(Ok(Event::Error { message, .. })) => {
                let mut s = state.lock().await;
                push_log(&mut s.log, "error", &message);
            }
            Ok(Ok(Event::Shutdown)) | Ok(Err(_)) => return,
            Ok(Ok(_)) => {}
            Err(_timeout) => {
                if !line_buf.is_empty() {
                    let mut s = state.lock().await;
                    push_log(&mut s.log, "received", &std::mem::take(&mut line_buf));
                }
                let s = state.lock().await;
                if s.engine.is_none() {
                    return;
                }
            }
        }
    }
}
