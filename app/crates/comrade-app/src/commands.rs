use std::collections::HashMap;
use std::sync::Arc;

use chrono::Local;
use comrade_core::{enumerate_devices, DaemonClient};
use comrade_protocol::{Command, DeviceInfo, DeviceKind, Event, SerialConfig};
use tauri::ipc::Channel;
use tauri::State;
use tokio::sync::Mutex;

use crate::auto_log::AutoLogger;
use crate::ble_session::BleNusSession;
use crate::connection_status::StatusTracker;
use crate::hid_descriptor::{self, HidDescriptorInfo};
use crate::hid_session::{HidReport, HidSession};
use crate::line_assembler::{LineAssembler, SerialLine};
use crate::log_buffer::{LogBuffer, LogEntry};
#[cfg(target_os = "macos")]
use crate::native_ble_nus::NativeBleNusSession;

/// Active connection — Serial, HID, BLE, Remote, or nothing.
pub(crate) enum ActiveConnection {
    None,
    Serial {
        client: DaemonClient,
        assembler: LineAssembler,
    },
    Hid {
        session: HidSession,
    },
    BleNus {
        session: BleNusSession,
    },
    #[cfg(target_os = "macos")]
    NativeBleNus {
        session: NativeBleNusSession,
    },
    /// Remote connection to a headless CLI MCP server.
    Remote {
        cancel: tokio_util::sync::CancellationToken,
    },
}

/// Per-tab state: connection + log buffer + status tracker.
pub(crate) struct TabState {
    pub connection: ActiveConnection,
    pub log_buffer: Arc<LogBuffer>,
    pub status_tracker: Arc<StatusTracker>,
    /// Channel to push lines to the frontend (serial/NUS tabs).
    pub line_channel: Option<Channel<SerialLine>>,
}

/// Shared application state managed by Tauri.
pub struct AppState {
    pub(crate) tabs: HashMap<String, TabState>,
    pub(crate) auto_logger: Arc<AutoLogger>,
    pub(crate) app_handle: Option<tauri::AppHandle>,
}

impl AppState {
    pub fn new(auto_logger: Arc<AutoLogger>) -> Self {
        Self {
            tabs: HashMap::new(),
            auto_logger,
            app_handle: None,
        }
    }

    /// Get or create tab state for a given tab ID.
    fn get_or_create_tab(&mut self, tab_id: &str) -> &mut TabState {
        self.tabs.entry(tab_id.to_string()).or_insert_with(|| {
            let log_buffer = Arc::new(LogBuffer::new());
            log_buffer.set_auto_logger(self.auto_logger.clone());
            TabState {
                connection: ActiveConnection::None,
                log_buffer,
                status_tracker: Arc::new(StatusTracker::new()),
                line_channel: None,
            }
        })
    }
}

pub type SharedState = Arc<Mutex<AppState>>;

#[tauri::command]
pub async fn list_devices() -> Result<Vec<DeviceInfo>, String> {
    let mut devices = enumerate_devices().map_err(|e| e.to_string())?;
    devices.retain(|d| {
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
    });

    // BLE scan with timeout so it doesn't block the initial device list.
    let ble_devices = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        crate::ble_session::list_ble_devices(),
    )
    .await
    .unwrap_or(Ok(Vec::new()))
    .unwrap_or_default();

    for ble in ble_devices {
        let merged = ble.product.as_ref().and_then(|ble_name| {
            devices.iter_mut().find(|d| {
                d.bus_type.as_deref() == Some("Bluetooth")
                    && d.product.as_ref() == Some(ble_name)
            })
        });

        if let Some(existing) = merged {
            existing.ble_id = ble.ble_id;
            if let Some(ref mut svcs) = existing.ble_services {
                if let Some(new_svcs) = &ble.ble_services {
                    for s in new_svcs {
                        if !svcs.contains(s) {
                            svcs.push(s.clone());
                        }
                    }
                }
            } else {
                existing.ble_services = ble.ble_services;
            }
            if existing.kind == DeviceKind::Hid {
                let svcs = existing.ble_services.get_or_insert_with(Vec::new);
                if !svcs.contains(&"hid".to_string()) {
                    svcs.push("hid".to_string());
                }
            }
            existing.kind = DeviceKind::Ble;
        } else {
            devices.push(ble);
        }
    }

    Ok(devices)
}

#[tauri::command]
pub async fn scan_ble() -> Result<Vec<DeviceInfo>, String> {
    crate::ble_session::list_ble_devices().await
}

#[tauri::command]
pub async fn connect(
    tab_id: String,
    port: String,
    baud: u32,
    on_line: Channel<SerialLine>,
    state: State<'_, SharedState>,
) -> Result<(), String> {
    let mut app = state.lock().await;
    let tab = app.get_or_create_tab(&tab_id);

    shutdown_connection(&mut tab.connection).await;

    let assembler = LineAssembler::new();

    let config = SerialConfig {
        baud_rate: baud,
        ..SerialConfig::default()
    };

    let client = DaemonClient::connect_or_spawn(&port, &config)
        .await
        .map_err(|e| e.to_string())?;

    // Send a Connect command in case the daemon's Engine is disconnected
    // (e.g. device was unplugged and re-plugged).
    client.send_command(Command::Connect {
        port: port.clone(),
        config: config.clone(),
    }).await.map_err(|e| e.to_string())?;
    let mut event_rx = client.subscribe();

    tab.connection = ActiveConnection::Serial { client, assembler };

    tab.status_tracker.set_serial(&port, baud);
    tab.line_channel = Some(on_line.clone());

    let log_buf = tab.log_buffer.clone();
    let status = tab.status_tracker.clone();
    let state_clone = state.inner().clone();
    let tab_id_clone = tab_id.clone();
    tokio::spawn(async move {
        loop {
            match tokio::time::timeout(
                std::time::Duration::from_millis(100),
                event_rx.recv(),
            )
            .await
            {
                Ok(Ok(Event::Data { bytes, .. })) => {
                    let lines = {
                        let mut app = state_clone.lock().await;
                        if let Some(tab) = app.tabs.get_mut(&tab_id_clone) {
                            if let ActiveConnection::Serial { ref mut assembler, .. } = tab.connection {
                                assembler.feed(&bytes, "received")
                            } else {
                                Vec::new()
                            }
                        } else {
                            return;
                        }
                    };
                    for line in lines {
                        log_buf.push(LogEntry::Serial(line.clone()));
                        status.update_rx_bytes(line.rx_bytes_total);
                        if on_line.send(line).is_err() {
                            return;
                        }
                    }
                }
                Ok(Ok(Event::Connected { port, config, .. })) => {
                    let line = {
                        let app = state_clone.lock().await;
                        if let Some(tab) = app.tabs.get(&tab_id_clone) {
                            if let ActiveConnection::Serial { ref assembler, .. } = tab.connection {
                                assembler.system_line(&format!(
                                    "Connected to {} at {} baud",
                                    port, config.baud_rate
                                ))
                            } else {
                                continue;
                            }
                        } else {
                            return;
                        }
                    };
                    log_buf.push(LogEntry::Serial(line.clone()));
                    let _ = on_line.send(line);
                }
                Ok(Ok(Event::Disconnected { reason, .. })) => {
                    let lines = {
                        let mut app = state_clone.lock().await;
                        if let Some(tab) = app.tabs.get_mut(&tab_id_clone) {
                            if let ActiveConnection::Serial { ref mut assembler, .. } = tab.connection {
                                let mut result = Vec::new();
                                if let Some(partial) = assembler.flush("received") {
                                    result.push(partial);
                                }
                                result.push(
                                    assembler.system_line(&format!("Disconnected: {reason}")),
                                );
                                result
                            } else {
                                Vec::new()
                            }
                        } else {
                            return;
                        }
                    };
                    for line in lines {
                        log_buf.push(LogEntry::Serial(line.clone()));
                        let _ = on_line.send(line);
                    }
                    status.set_disconnected();
                    return;
                }
                Ok(Ok(Event::Error { message, .. })) => {
                    let line = {
                        let app = state_clone.lock().await;
                        if let Some(tab) = app.tabs.get(&tab_id_clone) {
                            if let ActiveConnection::Serial { ref assembler, .. } = tab.connection {
                                assembler.system_line(&format!("Error: {message}"))
                            } else {
                                continue;
                            }
                        } else {
                            return;
                        }
                    };
                    log_buf.push(LogEntry::Serial(line.clone()));
                    let _ = on_line.send(line);
                }
                Ok(Ok(Event::Shutdown)) | Ok(Err(_)) => return,
                Ok(Ok(_)) => {}
                Err(_timeout) => {
                    let partial = {
                        let mut app = state_clone.lock().await;
                        if let Some(tab) = app.tabs.get_mut(&tab_id_clone) {
                            if let ActiveConnection::Serial { ref mut assembler, .. } = tab.connection {
                                assembler.flush("received")
                            } else {
                                None
                            }
                        } else {
                            return;
                        }
                    };
                    if let Some(line) = partial {
                        log_buf.push(LogEntry::Serial(line.clone()));
                        let _ = on_line.send(line);
                    }
                }
            }
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn connect_hid(
    tab_id: String,
    hid_path: String,
    vid: u16,
    pid: u16,
    on_report: Channel<HidReport>,
    state: State<'_, SharedState>,
) -> Result<(), String> {
    let mut app = state.lock().await;
    let tab = app.get_or_create_tab(&tab_id);

    shutdown_connection(&mut tab.connection).await;

    let log_buf = tab.log_buffer.clone();
    let status = tab.status_tracker.clone();

    let hid_path_clone = hid_path.clone();
    let session = HidSession::open(hid_path, vid, pid, move |report| {
        let _ = on_report.send(report.clone());
        log_buf.push(LogEntry::Hid(report.clone()));
        status.update_rx_bytes(report.rx_bytes_total);
    })
    .await?;

    tab.status_tracker.set_hid(&hid_path_clone, None);
    tab.connection = ActiveConnection::Hid { session };

    Ok(())
}

#[tauri::command]
pub async fn connect_ble_nus(
    tab_id: String,
    ble_id: String,
    on_line: Channel<SerialLine>,
    state: State<'_, SharedState>,
) -> Result<(), String> {
    let mut app = state.lock().await;
    let tab = app.get_or_create_tab(&tab_id);
    shutdown_connection(&mut tab.connection).await;

    tab.line_channel = Some(on_line.clone());

    let log_buf = tab.log_buffer.clone();
    let status = tab.status_tracker.clone();

    let log_buf2 = log_buf.clone();
    let status2 = status.clone();
    let on_line2 = on_line.clone();
    let ble_id2 = ble_id.clone();

    match BleNusSession::open(ble_id.clone(), move |line| {
        log_buf.push(LogEntry::Serial(line.clone()));
        status.update_rx_bytes(line.rx_bytes_total);
        let _ = on_line.send(line);
    })
    .await
    {
        Ok(session) => {
            tab.status_tracker.set_ble_nus(&ble_id);
            tab.connection = ActiveConnection::BleNus { session };
            Ok(())
        }
        #[cfg(target_os = "macos")]
        Err(_btleplug_err) => {
            tracing::debug!("btleplug failed: {_btleplug_err}, trying native CoreBluetooth");
            let session = NativeBleNusSession::open(ble_id2.clone(), move |line| {
                log_buf2.push(LogEntry::Serial(line.clone()));
                status2.update_rx_bytes(line.rx_bytes_total);
                let _ = on_line2.send(line);
            })
            .await?;

            tab.status_tracker.set_ble_nus(&ble_id2);
            tab.connection = ActiveConnection::NativeBleNus { session };
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        Err(e) => Err(e),
    }
}

#[tauri::command]
pub async fn send_data(
    tab_id: String,
    text: String,
    state: State<'_, SharedState>,
) -> Result<(), String> {
    let app = state.lock().await;
    let tab = app.tabs.get(&tab_id).ok_or("Tab not found")?;

    tab.log_buffer.push(LogEntry::Serial(SerialLine {
        timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
        text: text.clone(),
        kind: "sent",
        rx_bytes_total: 0,
    }));

    match &tab.connection {
        ActiveConnection::Serial { client, .. } => {
            let sender = client.cmd_sender();
            drop(app);
            let mut data = text.into_bytes();
            data.push(b'\n');
            sender
                .send(Command::Send { data })
                .await
                .map_err(|e| e.to_string())
        }
        ActiveConnection::BleNus { session } => {
            let mut data = text.into_bytes();
            data.push(b'\n');
            session.send(data).await
        }
        #[cfg(target_os = "macos")]
        ActiveConnection::NativeBleNus { session } => {
            let mut data = text.into_bytes();
            data.push(b'\n');
            session.send(data).await
        }
        _ => Err("Not connected (serial/NUS)".to_string()),
    }
}

#[tauri::command]
pub async fn send_hid_report(
    tab_id: String,
    data: Vec<u8>,
    report_type: String,
    state: State<'_, SharedState>,
) -> Result<(), String> {
    let app = state.lock().await;
    let tab = app.tabs.get(&tab_id).ok_or("Tab not found")?;
    match &tab.connection {
        ActiveConnection::Hid { session } => {
            match report_type.as_str() {
                "feature" => session.send_feature_report(data).await,
                _ => session.send_output_report(data).await,
            }
        }
        _ => Err("Not connected (HID)".to_string()),
    }
}

#[tauri::command]
pub async fn get_hid_descriptor(
    tab_id: String,
    state: State<'_, SharedState>,
) -> Result<HidDescriptorInfo, String> {
    let app = state.lock().await;
    let tab = app.tabs.get(&tab_id).ok_or("Tab not found")?;
    match &tab.connection {
        ActiveConnection::Hid { session } => {
            let raw = session.raw_descriptor();
            Ok(hid_descriptor::parse_hid_descriptor(raw))
        }
        _ => Err("Not connected (HID)".to_string()),
    }
}

#[tauri::command]
pub async fn set_dtr(
    tab_id: String,
    active: bool,
    state: State<'_, SharedState>,
) -> Result<(), String> {
    let app = state.lock().await;
    let tab = app.tabs.get(&tab_id).ok_or("Tab not found")?;
    if let ActiveConnection::Serial { ref client, .. } = tab.connection {
        client
            .send_command(Command::SetDtr { active })
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Not connected (serial)".to_string())
    }
}

#[tauri::command]
pub async fn set_rts(
    tab_id: String,
    active: bool,
    state: State<'_, SharedState>,
) -> Result<(), String> {
    let app = state.lock().await;
    let tab = app.tabs.get(&tab_id).ok_or("Tab not found")?;
    if let ActiveConnection::Serial { ref client, .. } = tab.connection {
        client
            .send_command(Command::SetRts { active })
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Not connected (serial)".to_string())
    }
}

#[tauri::command]
pub async fn send_break(
    tab_id: String,
    state: State<'_, SharedState>,
) -> Result<(), String> {
    let app = state.lock().await;
    let tab = app.tabs.get(&tab_id).ok_or("Tab not found")?;
    if let ActiveConnection::Serial { ref client, .. } = tab.connection {
        client
            .send_command(Command::SendBreak)
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("Not connected (serial)".to_string())
    }
}

#[tauri::command]
pub async fn disconnect(
    tab_id: String,
    state: State<'_, SharedState>,
) -> Result<(), String> {
    let mut app = state.lock().await;
    if let Some(mut tab) = app.tabs.remove(&tab_id) {
        shutdown_connection(&mut tab.connection).await;
    }
    Ok(())
}

#[tauri::command]
pub async fn export_log(
    tab_id: String,
    path: String,
    format: String,
    state: State<'_, SharedState>,
) -> Result<usize, String> {
    let app = state.lock().await;
    let tab = app.tabs.get(&tab_id).ok_or("Tab not found")?;
    let entries = tab.log_buffer.tail(MAX_EXPORT_ENTRIES);
    drop(app);

    let content = match format.as_str() {
        "csv" => {
            let mut out = String::from("timestamp,direction,text\n");
            for e in &entries {
                let ts = e.timestamp();
                let kind = e.kind();
                let text = e.text_content().replace('"', "\"\"");
                out.push_str(&format!("\"{ts}\",\"{kind}\",\"{text}\"\n"));
            }
            out
        }
        _ => {
            let mut out = String::new();
            for e in &entries {
                out.push_str(&e.format_line());
                out.push('\n');
            }
            out
        }
    };
    let count = entries.len();
    std::fs::write(&path, content).map_err(|e| format!("Write failed: {e}"))?;
    Ok(count)
}

const MAX_EXPORT_ENTRIES: usize = 10_000;

/// Check if a headless CLI MCP server is running on port 9712.
#[tauri::command]
pub async fn check_remote_mcp() -> Result<Option<serde_json::Value>, String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": { "name": "get_status", "arguments": {} }
    });
    match client
        .post("http://127.0.0.1:9712/mcp")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => {
            let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            // Extract the text content from the MCP response.
            let text = json
                .pointer("/result/content/0/text")
                .and_then(|t| t.as_str())
                .unwrap_or("{}");
            let status: serde_json::Value =
                serde_json::from_str(text).unwrap_or(serde_json::json!(null));
            Ok(Some(status))
        }
        Err(_) => Ok(None),
    }
}

/// Connect to a remote headless CLI MCP session. Polls get_logs and
/// streams lines into the frontend channel.
#[tauri::command]
pub async fn connect_remote(
    tab_id: String,
    on_line: Channel<SerialLine>,
    state: State<'_, SharedState>,
) -> Result<(), String> {
    let mut app = state.lock().await;
    let tab = app.get_or_create_tab(&tab_id);
    shutdown_connection(&mut tab.connection).await;

    let cancel = tokio_util::sync::CancellationToken::new();
    tab.connection = ActiveConnection::Remote {
        cancel: cancel.clone(),
    };
    tab.line_channel = Some(on_line.clone());

    // Get initial status from headless CLI.
    let log_buf = tab.log_buffer.clone();
    let status = tab.status_tracker.clone();
    drop(app);

    let client = reqwest::Client::new();
    let mut last_ts = String::new();

    // Poll loop: fetch new logs from the headless CLI every 200ms.
    tokio::spawn(async move {
        loop {
            if cancel.is_cancelled() {
                return;
            }

            let body = if last_ts.is_empty() {
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "method": "tools/call",
                    "params": { "name": "get_logs", "arguments": { "count": 200 } }
                })
            } else {
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "method": "tools/call",
                    "params": { "name": "get_logs", "arguments": { "since": last_ts } }
                })
            };

            if let Ok(resp) = client
                .post("http://127.0.0.1:9712/mcp")
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .json(&body)
                .send()
                .await
            {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    let text = json
                        .pointer("/result/content/0/text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("");

                    if !text.is_empty() && text != "No log entries." {
                        for raw_line in text.lines() {
                            // Parse "[HH:MM:SS.mmm] [kind] text"
                            if let Some(line) = parse_mcp_log_line(raw_line) {
                                if line.timestamp > last_ts {
                                    last_ts = line.timestamp.clone();
                                }
                                log_buf.push(LogEntry::Serial(line.clone()));
                                status.update_rx_bytes(line.rx_bytes_total);
                                let _ = on_line.send(line);
                            }
                        }
                    }
                }
            }

            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
            }
        }
    });

    Ok(())
}

/// Send text through the remote headless CLI MCP.
#[tauri::command]
pub async fn send_remote(text: String) -> Result<(), String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "tools/call",
        "params": { "name": "send_serial", "arguments": { "text": text } }
    });
    client
        .post("http://127.0.0.1:9712/mcp")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn parse_mcp_log_line(line: &str) -> Option<SerialLine> {
    // Format: "[HH:MM:SS.mmm] [kind] text"
    let line = line.strip_prefix('[')?;
    let (ts, rest) = line.split_once("] [")?;
    let (kind, text) = rest.split_once("] ")?;
    Some(SerialLine {
        timestamp: ts.to_string(),
        text: text.to_string(),
        kind: match kind {
            "received" => "received",
            "sent" => "sent",
            "system" => "system",
            "error" => "system",
            _ => "received",
        },
        rx_bytes_total: 0,
    })
}

#[tauri::command]
pub async fn debug_ble() -> Result<Vec<String>, String> {
    crate::ble_session::debug_ble_peripherals().await
}

#[tauri::command]
pub fn start_auto_log(
    directory: String,
    auto_logger: State<'_, Arc<AutoLogger>>,
) -> Result<String, String> {
    auto_logger.start(&directory)
}

#[tauri::command]
pub fn stop_auto_log(
    auto_logger: State<'_, Arc<AutoLogger>>,
) -> Result<Option<(String, usize)>, String> {
    Ok(auto_logger.stop())
}

#[tauri::command]
pub fn auto_log_status(
    auto_logger: State<'_, Arc<AutoLogger>>,
) -> Result<Option<String>, String> {
    Ok(auto_logger.current_path())
}

pub async fn shutdown_connection(conn: &mut ActiveConnection) {
    match std::mem::replace(conn, ActiveConnection::None) {
        ActiveConnection::Serial { client, .. } => {
            let _ = client.send_command(Command::Disconnect).await;
        }
        ActiveConnection::Hid { session } => {
            session.stop().await;
        }
        ActiveConnection::BleNus { session } => {
            session.stop().await;
        }
        #[cfg(target_os = "macos")]
        ActiveConnection::NativeBleNus { session } => {
            session.stop().await;
        }
        ActiveConnection::Remote { cancel } => {
            cancel.cancel();
        }
        ActiveConnection::None => {}
    }
}
