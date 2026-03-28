use std::sync::Arc;

use chrono::Local;
use comrade_protocol::Command;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};
use serde::Serialize;
use tauri::Manager;
use tracing::{info, warn};

use crate::commands::{ActiveConnection, SharedState};
use crate::line_assembler::SerialLine;
use crate::log_buffer::{LogBuffer, LogEntry};

// ── Parameter structs ────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetLogsParams {
    #[schemars(description = "Tab ID to get logs from. If omitted, returns logs from all tabs.")]
    pub tab_id: Option<String>,
    #[schemars(description = "Number of recent log entries to return (default 100, max 5000)")]
    pub count: Option<usize>,
    #[schemars(description = "Only return entries after this timestamp (e.g. \"12:34:56.789\")")]
    pub since: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchLogsParams {
    #[schemars(description = "Tab ID to search. If omitted, searches all tabs.")]
    pub tab_id: Option<String>,
    #[schemars(description = "Regex pattern (or substring) to search for in log entries")]
    pub pattern: String,
    #[schemars(description = "Maximum number of matching entries to return (default 100)")]
    pub max_results: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SendSerialParams {
    #[schemars(description = "Tab ID. If omitted, uses the first active connection.")]
    pub tab_id: Option<String>,
    #[schemars(description = "Text to send to the serial device (newline appended automatically)")]
    pub text: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SendHidReportParams {
    #[schemars(description = "Tab ID. If omitted, uses the first active connection.")]
    pub tab_id: Option<String>,
    #[schemars(description = "Byte array to send (first byte is report ID)")]
    pub data: Vec<u8>,
    #[schemars(
        description = "Report type: 'output' (default) or 'feature'"
    )]
    pub report_type: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ConnectDeviceParams {
    #[schemars(description = "Connection type: 'serial', 'hid', or 'ble_nus'")]
    pub connection_type: String,
    #[schemars(description = "Device path: serial port path for serial, HID path for HID, or BLE peripheral ID for BLE NUS")]
    pub path: String,
    #[schemars(description = "Baud rate (only for serial connections, defaults to 115200)")]
    pub baud: Option<u32>,
    #[schemars(description = "Device name (optional, for display purposes)")]
    pub device_name: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DisconnectDeviceParams {
    #[schemars(description = "Tab ID. If omitted, disconnects the first active connection.")]
    pub tab_id: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ClearLogsParams {
    #[schemars(description = "Tab ID to clear. If omitted, clears all tabs.")]
    pub tab_id: Option<String>,
}

// ── MCP Handler ──────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ComradeMcp {
    shared_state: SharedState,
    tool_router: ToolRouter<Self>,
}

/// Helper: resolve tab_id — if None, find the first active (non-disconnected) tab.
fn resolve_tab_id(app: &crate::commands::AppState, tab_id: Option<&str>) -> Result<String, McpError> {
    if let Some(id) = tab_id {
        if app.tabs.contains_key(id) {
            return Ok(id.to_string());
        }
        return Err(McpError::invalid_request("Tab not found".to_string(), None));
    }
    // Find first active tab.
    for (id, tab) in &app.tabs {
        let status = tab.status_tracker.snapshot();
        if status.state != "disconnected" {
            return Ok(id.clone());
        }
    }
    // Fall back to any tab.
    app.tabs.keys().next().cloned().ok_or_else(|| {
        McpError::invalid_request("No active connections".to_string(), None)
    })
}

/// Helper: collect log buffers from tabs.
async fn collect_log_buffers(
    state: &SharedState,
    tab_id: Option<&str>,
) -> Vec<Arc<LogBuffer>> {
    let app = state.lock().await;
    match tab_id {
        Some(id) => app
            .tabs
            .get(id)
            .map(|t| vec![t.log_buffer.clone()])
            .unwrap_or_default(),
        None => app.tabs.values().map(|t| t.log_buffer.clone()).collect(),
    }
}

#[tool_router]
impl ComradeMcp {
    pub fn new(shared_state: SharedState) -> Self {
        Self {
            shared_state,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Get recent serial/HID log entries. Optionally filter by tab_id.")]
    async fn get_logs(
        &self,
        Parameters(params): Parameters<GetLogsParams>,
    ) -> Result<CallToolResult, McpError> {
        let buffers = collect_log_buffers(&self.shared_state, params.tab_id.as_deref()).await;

        let mut entries = Vec::new();
        for buf in &buffers {
            if let Some(ref since) = params.since {
                entries.extend(buf.since(since));
            } else {
                let count = params.count.unwrap_or(100).min(5000);
                entries.extend(buf.tail(count));
            }
        }

        // Sort by timestamp when aggregating multiple tabs.
        if buffers.len() > 1 {
            entries.sort_by(|a, b| a.timestamp().cmp(b.timestamp()));
        }

        if entries.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No log entries available.",
            )]));
        }

        let text: String = entries.iter().map(|e| e.format_line()).collect::<Vec<_>>().join("\n");
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(description = "Search log entries by regex pattern or substring. Optionally filter by tab_id.")]
    async fn search_logs(
        &self,
        Parameters(params): Parameters<SearchLogsParams>,
    ) -> Result<CallToolResult, McpError> {
        let buffers = collect_log_buffers(&self.shared_state, params.tab_id.as_deref()).await;
        let max = params.max_results.unwrap_or(100).min(5000);

        let mut entries = Vec::new();
        for buf in &buffers {
            entries.extend(buf.search(&params.pattern, max));
        }

        if buffers.len() > 1 {
            entries.sort_by(|a, b| a.timestamp().cmp(b.timestamp()));
            entries.truncate(max);
        }

        if entries.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No entries matching '{}'.",
                params.pattern
            ))]));
        }

        let text: String = entries.iter().map(|e| e.format_line()).collect::<Vec<_>>().join("\n");
        Ok(CallToolResult::success(vec![Content::text(format!(
            "{} match(es) for '{}':\n{}",
            entries.len(),
            params.pattern,
            text,
        ))]))
    }

    #[tool(description = "Get the current connection status for all tabs.")]
    async fn get_status(&self) -> Result<CallToolResult, McpError> {
        let app = self.shared_state.lock().await;

        #[derive(Serialize)]
        struct TabStatus {
            tab_id: String,
            #[serde(flatten)]
            status: crate::connection_status::ConnectionStatus,
        }

        let statuses: Vec<TabStatus> = app
            .tabs
            .iter()
            .map(|(id, tab)| TabStatus {
                tab_id: id.clone(),
                status: tab.status_tracker.snapshot(),
            })
            .collect();

        if statuses.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No active connections.",
            )]));
        }

        let json = serde_json::to_string_pretty(&statuses).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    #[tool(description = "List available serial and HID devices that can be connected to.")]
    fn list_devices(&self) -> Result<CallToolResult, McpError> {
        let devices = comrade_core::enumerate_devices()
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let filtered: Vec<_> = devices
            .into_iter()
            .filter(|d| {
                if let Some(ref sp) = d.serial_path {
                    if sp == "/dev/cu.debug-console" || sp == "/dev/cu.Bluetooth-Incoming-Port" {
                        return false;
                    }
                }
                if d.kind == comrade_protocol::DeviceKind::Hid {
                    if d.vid == Some(0x05AC) {
                        return false;
                    }
                    if d.vid == Some(0x0000) && d.pid == Some(0x0000) {
                        return false;
                    }
                }
                true
            })
            .collect();

        let json = serde_json::to_string_pretty(&filtered).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    #[tool(description = "Connect to a device and open a tab in the UI. Returns the tab_id for subsequent operations. If the device is already connected, returns the existing tab_id.")]
    async fn connect_device(
        &self,
        Parameters(params): Parameters<ConnectDeviceParams>,
    ) -> Result<CallToolResult, McpError> {
        // Check if already connected to this device (skip disconnected tabs).
        let app = self.shared_state.lock().await;
        for (id, tab) in &app.tabs {
            let status = tab.status_tracker.snapshot();
            if status.state != "disconnected"
                && status.device_path.as_deref() == Some(&params.path)
            {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Already connected in tab {id}"
                ))]));
            }
        }

        // Get app handle to invoke frontend connect.
        let app_handle = app
            .app_handle
            .clone()
            .ok_or_else(|| McpError::internal_error("App not ready".to_string(), None))?;
        drop(app);

        // Look up device name from enumeration if not provided.
        let name = params.device_name.clone().unwrap_or_else(|| {
            comrade_core::enumerate_devices()
                .ok()
                .and_then(|devs| {
                    devs.iter().find(|d| {
                        d.serial_path.as_deref() == Some(&params.path)
                            || d.hid_path.as_deref() == Some(&params.path)
                    }).and_then(|d| d.product.clone().or(d.manufacturer.clone()))
                })
                .unwrap_or_else(|| params.path.rsplit('/').next().unwrap_or("Device").to_string())
        });
        let name = name.as_str();
        let js = match params.connection_type.as_str() {
            "serial" => {
                let baud = params.baud.unwrap_or(115200);
                format!(
                    "window.__mcpConnect && window.__mcpConnect('serial', {}, {}, '{}')",
                    serde_json::to_string(&params.path).unwrap_or_default(),
                    baud,
                    name.replace('\'', "\\'"),
                )
            }
            "hid" => {
                format!(
                    "window.__mcpConnect && window.__mcpConnect('hid', {}, 0, '{}')",
                    serde_json::to_string(&params.path).unwrap_or_default(),
                    name.replace('\'', "\\'"),
                )
            }
            "ble_nus" => {
                format!(
                    "window.__mcpConnect && window.__mcpConnect('ble_nus', {}, 0, '{}')",
                    serde_json::to_string(&params.path).unwrap_or_default(),
                    name.replace('\'', "\\'"),
                )
            }
            other => {
                return Err(McpError::invalid_request(
                    format!("Unknown connection type: {other}. Use 'serial', 'hid', or 'ble_nus'."),
                    None,
                ));
            }
        };

        if let Some(window) = app_handle.get_webview_window("main") {
            window.eval(&js).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        }

        // Wait briefly for the frontend to create the tab and backend to register it.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Find the newly created tab.
        let app = self.shared_state.lock().await;
        for (id, tab) in &app.tabs {
            let status = tab.status_tracker.snapshot();
            if status.device_path.as_deref() == Some(&params.path) {
                return Ok(CallToolResult::success(vec![Content::text(format!(
                    "Connected. Tab ID: {id}"
                ))]));
            }
        }

        Ok(CallToolResult::success(vec![Content::text(
            "Connection initiated. Use get_status to check progress."
        )]))
    }

    #[tool(description = "Disconnect a device and close its tab, releasing the port. Use connect_device to open a fresh connection later.")]
    async fn disconnect_device(
        &self,
        Parameters(params): Parameters<DisconnectDeviceParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut app = self.shared_state.lock().await;
        let tab_id = resolve_tab_id(&app, params.tab_id.as_deref())?;

        if let Some(mut tab) = app.tabs.remove(&tab_id) {
            crate::commands::shutdown_connection(&mut tab.connection).await;
        }

        // Tell frontend to close the tab too.
        if let Some(ref handle) = app.app_handle {
            if let Some(window) = handle.get_webview_window("main") {
                let _ = window.eval(format!(
                    "window.__mcpCloseTab && window.__mcpCloseTab('{}')",
                    tab_id
                ));
            }
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Disconnected and closed tab {tab_id}. Port released.",
        ))]))
    }

    #[tool(description = "Clear log entries. Optionally specify tab_id.")]
    async fn clear_logs(
        &self,
        Parameters(params): Parameters<ClearLogsParams>,
    ) -> Result<CallToolResult, McpError> {
        let buffers = collect_log_buffers(&self.shared_state, params.tab_id.as_deref()).await;
        for buf in &buffers {
            buf.clear();
        }
        Ok(CallToolResult::success(vec![Content::text(
            "Logs cleared.",
        )]))
    }

    #[tool(description = "Send text to a connected serial device. A newline is appended automatically.")]
    async fn send_serial(
        &self,
        Parameters(params): Parameters<SendSerialParams>,
    ) -> Result<CallToolResult, McpError> {
        let app = self.shared_state.lock().await;
        let tab_id = resolve_tab_id(&app, params.tab_id.as_deref())?;
        let tab = app.tabs.get(&tab_id).unwrap();

        let mcp_line = SerialLine {
            timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
            text: params.text.clone(),
            kind: "mcp",
            rx_bytes_total: 0,
        };
        tab.log_buffer.push(LogEntry::Serial(mcp_line.clone()));
        if let Some(ref ch) = tab.line_channel {
            let _ = ch.send(mcp_line);
        }

        match &tab.connection {
            ActiveConnection::Serial { client, .. } => {
                let sender = client.cmd_sender();
                drop(app);
                let mut data = params.text.into_bytes();
                data.push(b'\n');
                sender
                    .send(Command::Send { data })
                    .await
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
            }
            ActiveConnection::BleNus { session } => {
                let mut data = params.text.into_bytes();
                data.push(b'\n');
                session
                    .send(data)
                    .await
                    .map_err(|e| McpError::internal_error(e, None))?;
            }
            _ => {
                return Err(McpError::invalid_request(
                    "Not connected (serial/NUS)".to_string(),
                    None,
                ))
            }
        }

        Ok(CallToolResult::success(vec![Content::text("Sent.")]))
    }

    #[tool(description = "Send a HID report to a connected HID device. The first byte of the data array is the report ID.")]
    async fn send_hid_report(
        &self,
        Parameters(params): Parameters<SendHidReportParams>,
    ) -> Result<CallToolResult, McpError> {
        let app = self.shared_state.lock().await;
        let tab_id = resolve_tab_id(&app, params.tab_id.as_deref())?;
        let tab = app.tabs.get(&tab_id).unwrap();
        match &tab.connection {
            ActiveConnection::Hid { session } => {
                let report_type = params.report_type.as_deref().unwrap_or("output");
                let result = match report_type {
                    "feature" => session.send_feature_report(params.data).await,
                    _ => session.send_output_report(params.data).await,
                };
                result.map_err(|e| McpError::internal_error(e, None))?;
                Ok(CallToolResult::success(vec![Content::text("Sent.")]))
            }
            _ => Err(McpError::invalid_request(
                "Not connected (HID)".to_string(),
                None,
            )),
        }
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
                "COMrade serial/HID monitor with tabbed connections. Use get_status \
                 to see active tabs and their connections, get_logs/search_logs to \
                 read device output (optionally filtered by tab_id), list_devices to \
                 see available ports, clear_logs to reset log buffers, send_serial \
                 to send text to a serial device, and send_hid_report to send a HID \
                 report. All send commands require a tab_id."
                    .to_string(),
            )
    }
}

// ── Server startup ───────────────────────────────────────────────────

pub async fn start_mcp_server(shared_state: SharedState) {
    let ct = tokio_util::sync::CancellationToken::new();

    let service = StreamableHttpService::new(
        move || {
            Ok(ComradeMcp::new(shared_state.clone()))
        },
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig {
            stateful_mode: false,
            json_response: true,
            cancellation_token: ct.child_token(),
            ..Default::default()
        },
    );

    let router = axum::Router::new().nest_service("/mcp", service);

    match tokio::net::TcpListener::bind("127.0.0.1:9712").await {
        Ok(listener) => {
            info!("MCP server listening on http://127.0.0.1:9712/mcp");
            if let Err(e) = axum::serve(listener, router).await {
                warn!("MCP server error: {e}");
            }
        }
        Err(e) => {
            warn!("MCP server failed to bind 127.0.0.1:9712: {e} (continuing without MCP)");
        }
    }
}
