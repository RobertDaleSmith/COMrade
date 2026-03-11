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
use tracing::{info, warn};

use crate::commands::{ActiveConnection, SharedState};
use crate::connection_status::StatusTracker;
use crate::line_assembler::SerialLine;
use crate::log_buffer::{LogBuffer, LogEntry};

// ── Parameter structs ────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetLogsParams {
    #[schemars(description = "Number of recent log entries to return (default 100, max 5000)")]
    pub count: Option<usize>,
    #[schemars(description = "Only return entries after this timestamp (e.g. \"12:34:56.789\")")]
    pub since: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchLogsParams {
    #[schemars(description = "Regex pattern (or substring) to search for in log entries")]
    pub pattern: String,
    #[schemars(description = "Maximum number of matching entries to return (default 100)")]
    pub max_results: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SendSerialParams {
    #[schemars(description = "Text to send to the serial device (newline appended automatically)")]
    pub text: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SendHidReportParams {
    #[schemars(description = "Byte array to send (first byte is report ID)")]
    pub data: Vec<u8>,
    #[schemars(
        description = "Report type: 'output' (default) or 'feature'"
    )]
    pub report_type: Option<String>,
}

// ── MCP Handler ──────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ComradeMcp {
    log_buffer: Arc<LogBuffer>,
    status_tracker: Arc<StatusTracker>,
    shared_state: SharedState,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl ComradeMcp {
    pub fn new(
        log_buffer: Arc<LogBuffer>,
        status_tracker: Arc<StatusTracker>,
        shared_state: SharedState,
    ) -> Self {
        Self {
            log_buffer,
            status_tracker,
            shared_state,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Get recent serial/HID log entries from the connected device. Returns timestamped lines.")]
    fn get_logs(
        &self,
        Parameters(params): Parameters<GetLogsParams>,
    ) -> Result<CallToolResult, McpError> {
        let entries = if let Some(ref since) = params.since {
            self.log_buffer.since(since)
        } else {
            let count = params.count.unwrap_or(100).min(5000);
            self.log_buffer.tail(count)
        };

        if entries.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No log entries available.",
            )]));
        }

        let text: String = entries.iter().map(|e| e.format_line()).collect::<Vec<_>>().join("\n");
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(description = "Search log entries by regex pattern or substring. Returns matching timestamped lines.")]
    fn search_logs(
        &self,
        Parameters(params): Parameters<SearchLogsParams>,
    ) -> Result<CallToolResult, McpError> {
        let max = params.max_results.unwrap_or(100).min(5000);
        let entries = self.log_buffer.search(&params.pattern, max);

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

    #[tool(description = "Get the current connection status: device type, path, baud rate, and bytes received.")]
    fn get_status(&self) -> Result<CallToolResult, McpError> {
        let status = self.status_tracker.snapshot();
        let json = serde_json::to_string_pretty(&status).unwrap_or_default();
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

    #[tool(description = "Clear all log entries from the buffer.")]
    fn clear_logs(&self) -> Result<CallToolResult, McpError> {
        self.log_buffer.clear();
        Ok(CallToolResult::success(vec![Content::text(
            "Logs cleared.",
        )]))
    }

    #[tool(description = "Send text to the connected serial device. A newline is appended automatically.")]
    async fn send_serial(
        &self,
        Parameters(params): Parameters<SendSerialParams>,
    ) -> Result<CallToolResult, McpError> {
        self.log_buffer.push(LogEntry::Serial(SerialLine {
            timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
            text: params.text.clone(),
            kind: "sent",
            rx_bytes_total: 0,
        }));

        let app = self.shared_state.lock().await;
        match &app.connection {
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

    #[tool(description = "Send a HID report to the connected HID device. The first byte of the data array is the report ID.")]
    async fn send_hid_report(
        &self,
        Parameters(params): Parameters<SendHidReportParams>,
    ) -> Result<CallToolResult, McpError> {
        let app = self.shared_state.lock().await;
        match &app.connection {
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
                "COMrade serial/HID monitor. Use get_logs to read device output, \
                 search_logs to find specific patterns, get_status to check the \
                 current connection, list_devices to see available ports, \
                 clear_logs to reset the log buffer, send_serial to send text \
                 to a serial device, and send_hid_report to send a HID report \
                 to a HID device."
                    .to_string(),
            )
    }
}

// ── Server startup ───────────────────────────────────────────────────

pub async fn start_mcp_server(
    log_buffer: Arc<LogBuffer>,
    status_tracker: Arc<StatusTracker>,
    shared_state: SharedState,
) {
    let ct = tokio_util::sync::CancellationToken::new();

    let service = StreamableHttpService::new(
        move || {
            Ok(ComradeMcp::new(
                log_buffer.clone(),
                status_tracker.clone(),
                shared_state.clone(),
            ))
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
