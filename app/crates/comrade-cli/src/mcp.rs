//! MCP server for Claude Code integration.
//!
//! `comrade --mcp` starts as a stdio MCP server. If the COMrade GUI is
//! running, it bridges to the GUI's HTTP MCP server on port 9712 so
//! Claude can interact with the app's live tabs. If the GUI is not
//! running, it operates standalone with its own serial Engine.

use std::collections::VecDeque;
use std::io::{self, BufRead, Write};
use std::sync::Arc;

use anyhow::Result;
use chrono::Local;
use comrade_core::{enumerate_devices, Engine};
use comrade_protocol::{Command, Event, SerialConfig};
use tokio::sync::Mutex;

const MCP_URL: &str = "http://127.0.0.1:9712/mcp";

/// Check if the GUI MCP server is reachable.
async fn is_gui_running() -> bool {
    reqwest::Client::new()
        .post(MCP_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body("{}")
        .send()
        .await
        .is_ok()
}

pub async fn run_mcp() -> Result<()> {
    if is_gui_running().await {
        eprintln!("COMrade GUI detected, bridging to app...");
        run_bridge().await
    } else {
        eprintln!("COMrade running headless (standalone MCP server)");
        run_standalone().await
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Bridge mode — proxy stdio JSON-RPC to the GUI's HTTP MCP server
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
// Standalone mode — headless MCP server with its own Engine
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

struct ConnState {
    engine: Option<Engine>,
    port: Option<String>,
    baud: Option<u32>,
    connected: bool,
    rx_bytes: u64,
    log: VecDeque<LogEntry>,
}

type State = Arc<Mutex<ConnState>>;

fn now() -> String {
    Local::now().format("%H:%M:%S%.3f").to_string()
}

/// Minimal JSON-RPC dispatcher — no framework dependency, just parse and route.
async fn run_standalone() -> Result<()> {
    let state: State = Arc::new(Mutex::new(ConnState {
        engine: None,
        port: None,
        baud: None,
        connected: false,
        rx_bytes: 0,
        log: VecDeque::with_capacity(MAX_LOG),
    }));

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

        let req: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = json_rpc_error(None, -32700, &format!("Parse error: {e}"));
                writeln!(stdout, "{err}")?;
                stdout.flush()?;
                continue;
            }
        };

        let id = req.get("id").cloned();
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(serde_json::json!({}));

        let result = match method {
            "initialize" => handle_initialize(),
            "notifications/initialized" | "initialized" => continue, // no response
            "tools/list" => handle_tools_list(),
            "tools/call" => handle_tool_call(&params, &state).await,
            _ => Err(format!("Unknown method: {method}")),
        };

        if let Some(id) = id {
            let resp = match result {
                Ok(val) => serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": val }),
                Err(msg) => json_rpc_error(Some(id), -32603, &msg),
            };
            writeln!(stdout, "{resp}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn json_rpc_error(id: Option<serde_json::Value>, code: i32, msg: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": msg }
    })
}

fn handle_initialize() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "protocolVersion": "2025-03-26",
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "comrade", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "COMrade serial monitor (headless). Tools: list_devices, connect, disconnect, get_status, get_logs, search_logs, send_serial, clear_logs."
    }))
}

fn handle_tools_list() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "tools": [
            { "name": "list_devices", "description": "List available serial and HID devices.", "inputSchema": { "type": "object", "properties": {} } },
            { "name": "connect", "description": "Connect to a serial device.", "inputSchema": { "type": "object", "properties": { "port": { "type": "string", "description": "Serial port path" }, "baud": { "type": "integer", "description": "Baud rate (default 115200)" } }, "required": ["port"] } },
            { "name": "disconnect", "description": "Disconnect from the current device.", "inputSchema": { "type": "object", "properties": {} } },
            { "name": "get_status", "description": "Get connection status.", "inputSchema": { "type": "object", "properties": {} } },
            { "name": "get_logs", "description": "Get recent log entries.", "inputSchema": { "type": "object", "properties": { "count": { "type": "integer", "description": "Number of entries (default 100)" }, "since": { "type": "string", "description": "Timestamp to filter from" } } } },
            { "name": "search_logs", "description": "Search logs by regex or substring.", "inputSchema": { "type": "object", "properties": { "pattern": { "type": "string" }, "max_results": { "type": "integer" } }, "required": ["pattern"] } },
            { "name": "send_serial", "description": "Send text to serial device (newline appended).", "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] } },
            { "name": "clear_logs", "description": "Clear all log entries.", "inputSchema": { "type": "object", "properties": {} } }
        ]
    }))
}

async fn handle_tool_call(params: &serde_json::Value, state: &State) -> Result<serde_json::Value, String> {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(serde_json::json!({}));

    let text = match name {
        "list_devices" => tool_list_devices()?,
        "connect" => tool_connect(&args, state).await?,
        "disconnect" => tool_disconnect(state).await?,
        "get_status" => tool_get_status(state).await?,
        "get_logs" => tool_get_logs(&args, state).await?,
        "search_logs" => tool_search_logs(&args, state).await?,
        "send_serial" => tool_send_serial(&args, state).await?,
        "clear_logs" => tool_clear_logs(state).await?,
        _ => return Err(format!("Unknown tool: {name}")),
    };

    Ok(serde_json::json!({
        "content": [{ "type": "text", "text": text }]
    }))
}

fn tool_list_devices() -> Result<String, String> {
    let devices = enumerate_devices().map_err(|e| e.to_string())?;
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
    serde_json::to_string_pretty(&filtered).map_err(|e| e.to_string())
}

async fn tool_connect(args: &serde_json::Value, state: &State) -> Result<String, String> {
    let port = args.get("port").and_then(|p| p.as_str()).ok_or("Missing 'port'")?;
    let baud = args.get("baud").and_then(|b| b.as_u64()).unwrap_or(115200) as u32;

    let mut s = state.lock().await;

    // Disconnect existing.
    if let Some(ref engine) = s.engine {
        let _ = engine.send(Command::Shutdown).await;
    }
    s.engine = None;
    s.connected = false;
    s.rx_bytes = 0;

    let config = SerialConfig {
        baud_rate: baud,
        ..SerialConfig::default()
    };

    let engine = Engine::spawn();
    engine
        .send(Command::Connect { port: port.to_string(), config })
        .await
        .map_err(|e| e.to_string())?;

    s.port = Some(port.to_string());
    s.baud = Some(baud);
    s.engine = Some(engine);

    let state_clone = state.clone();
    tokio::spawn(async move { drain_events(state_clone).await });

    Ok(format!("Connected to {port} at {baud} baud"))
}

async fn tool_disconnect(state: &State) -> Result<String, String> {
    let mut s = state.lock().await;
    if let Some(ref engine) = s.engine {
        let _ = engine.send(Command::Shutdown).await;
    }
    s.engine = None;
    s.connected = false;
    let port = s.port.take().unwrap_or_default();
    Ok(format!("Disconnected from {port}"))
}

async fn tool_get_status(state: &State) -> Result<String, String> {
    let s = state.lock().await;
    let status = serde_json::json!({
        "connected": s.connected,
        "port": s.port,
        "baud": s.baud,
        "rx_bytes": s.rx_bytes,
    });
    serde_json::to_string_pretty(&status).map_err(|e| e.to_string())
}

async fn tool_get_logs(args: &serde_json::Value, state: &State) -> Result<String, String> {
    let s = state.lock().await;
    let entries: Vec<&LogEntry> = if let Some(since) = args.get("since").and_then(|s| s.as_str()) {
        s.log.iter().filter(|e| e.timestamp.as_str() > since).collect()
    } else {
        let count = args.get("count").and_then(|c| c.as_u64()).unwrap_or(100) as usize;
        let skip = s.log.len().saturating_sub(count.min(5000));
        s.log.iter().skip(skip).collect()
    };

    if entries.is_empty() {
        return Ok("No log entries.".to_string());
    }
    Ok(entries.iter().map(|e| e.format()).collect::<Vec<_>>().join("\n"))
}

async fn tool_search_logs(args: &serde_json::Value, state: &State) -> Result<String, String> {
    let pattern = args.get("pattern").and_then(|p| p.as_str()).ok_or("Missing 'pattern'")?;
    let max = args.get("max_results").and_then(|m| m.as_u64()).unwrap_or(100) as usize;

    let s = state.lock().await;
    let re = regex::Regex::new(pattern).ok();
    let entries: Vec<&LogEntry> = s.log
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
        .collect();

    if entries.is_empty() {
        return Ok(format!("No matches for '{pattern}'."));
    }
    Ok(format!("{} match(es):\n{}", entries.len(),
        entries.iter().map(|e| e.format()).collect::<Vec<_>>().join("\n")))
}

async fn tool_send_serial(args: &serde_json::Value, state: &State) -> Result<String, String> {
    let text = args.get("text").and_then(|t| t.as_str()).ok_or("Missing 'text'")?;
    let mut s = state.lock().await;
    let sender = s.engine.as_ref().map(|e| e.cmd_sender()).ok_or("Not connected")?;

    push_log(&mut s.log, "sent", text);
    drop(s);

    let mut data = text.as_bytes().to_vec();
    data.push(b'\n');
    sender.send(Command::Send { data }).await.map_err(|e| e.to_string())?;
    Ok("Sent.".to_string())
}

async fn tool_clear_logs(state: &State) -> Result<String, String> {
    state.lock().await.log.clear();
    Ok("Logs cleared.".to_string())
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

async fn drain_events(state: State) {
    let mut event_rx = {
        let s = state.lock().await;
        match &s.engine {
            Some(engine) => engine.subscribe(),
            None => return,
        }
    };

    let mut line_buf = String::new();

    loop {
        match tokio::time::timeout(std::time::Duration::from_millis(100), event_rx.recv()).await {
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
                push_log(&mut s.log, "system", &format!("Connected to {} at {} baud", port, config.baud_rate));
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
                if s.engine.is_none() { return; }
            }
        }
    }
}
