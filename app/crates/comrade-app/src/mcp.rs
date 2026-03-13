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
    #[schemars(description = "Tab ID of the connection to send to")]
    pub tab_id: String,
    #[schemars(description = "Text to send to the serial device (newline appended automatically)")]
    pub text: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SendHidReportParams {
    #[schemars(description = "Tab ID of the connection to send to")]
    pub tab_id: String,
    #[schemars(description = "Byte array to send (first byte is report ID)")]
    pub data: Vec<u8>,
    #[schemars(
        description = "Report type: 'output' (default) or 'feature'"
    )]
    pub report_type: Option<String>,
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
        let tab = app
            .tabs
            .get(&params.tab_id)
            .ok_or_else(|| McpError::invalid_request("Tab not found".to_string(), None))?;

        tab.log_buffer.push(LogEntry::Serial(SerialLine {
            timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
            text: params.text.clone(),
            kind: "sent",
            rx_bytes_total: 0,
        }));

        match &tab.connection {
            ActiveConnection::Serial { engine, .. } => {
                let cmd_tx = engine.cmd_sender();
                drop(app);
                let mut data = params.text.into_bytes();
                data.push(b'\n');
                cmd_tx
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
        let tab = app
            .tabs
            .get(&params.tab_id)
            .ok_or_else(|| McpError::invalid_request("Tab not found".to_string(), None))?;
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
