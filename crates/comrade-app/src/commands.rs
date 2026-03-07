use std::sync::Arc;

use chrono::Local;
use comrade_core::{enumerate_devices, Engine};
use comrade_protocol::{Command, DeviceInfo, Event, SerialConfig};
use tauri::ipc::Channel;
use tauri::State;
use tokio::sync::Mutex;

use crate::connection_status::StatusTracker;
use crate::hid_descriptor::{self, HidDescriptorInfo};
use crate::hid_session::{HidReport, HidSession};
use crate::line_assembler::{LineAssembler, SerialLine};
use crate::log_buffer::{LogBuffer, LogEntry};

/// Active connection — either Serial, HID, or nothing.
enum ActiveConnection {
    None,
    Serial {
        engine: Engine,
        assembler: LineAssembler,
    },
    Hid {
        session: HidSession,
    },
}

/// Shared application state managed by Tauri.
pub struct AppState {
    connection: ActiveConnection,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            connection: ActiveConnection::None,
        }
    }
}

pub type SharedState = Arc<Mutex<AppState>>;

#[tauri::command]
pub fn list_devices() -> Result<Vec<DeviceInfo>, String> {
    let mut devices = enumerate_devices().map_err(|e| e.to_string())?;
    devices.retain(|d| {
        // Filter out macOS system serial ports.
        if let Some(ref sp) = d.serial_path {
            if sp == "/dev/cu.debug-console" || sp == "/dev/cu.Bluetooth-Incoming-Port" {
                return false;
            }
        }
        // Filter out Apple internal HID devices (keyboard, trackpad, etc.)
        // and devices with vid/pid 0x0000 (virtual/system HID endpoints).
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
    Ok(devices)
}

#[tauri::command]
pub async fn connect(
    port: String,
    baud: u32,
    on_line: Channel<SerialLine>,
    state: State<'_, SharedState>,
    log_buffer: State<'_, Arc<LogBuffer>>,
    status_tracker: State<'_, Arc<StatusTracker>>,
) -> Result<(), String> {
    let mut app = state.lock().await;

    // Shut down existing connection.
    shutdown_active(&mut app.connection).await;

    let assembler = LineAssembler::new();

    let config = SerialConfig {
        baud_rate: baud,
        ..SerialConfig::default()
    };

    let engine = Engine::spawn();
    let mut event_rx = engine.subscribe();

    engine
        .send(Command::Connect {
            port: port.clone(),
            config: config.clone(),
        })
        .await
        .map_err(|e| e.to_string())?;

    app.connection = ActiveConnection::Serial { engine, assembler };

    status_tracker.set_serial(&port, baud);

    // Spawn a task that streams engine events → line assembler → Channel.
    let state_clone = state.inner().clone();
    let log_buf = log_buffer.inner().clone();
    let status = status_tracker.inner().clone();
    tokio::spawn(async move {
        loop {
            // Use a timeout so we can flush partial lines that don't end with \n.
            match tokio::time::timeout(
                std::time::Duration::from_millis(100),
                event_rx.recv(),
            )
            .await
            {
                Ok(Ok(Event::Data { bytes, .. })) => {
                    let lines = {
                        let mut app = state_clone.lock().await;
                        if let ActiveConnection::Serial { ref mut assembler, .. } = app.connection {
                            assembler.feed(&bytes, "received")
                        } else {
                            Vec::new()
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
                        if let ActiveConnection::Serial { ref assembler, .. } = app.connection {
                            assembler.system_line(&format!(
                                "Connected to {} at {} baud",
                                port, config.baud_rate
                            ))
                        } else {
                            continue;
                        }
                    };
                    log_buf.push(LogEntry::Serial(line.clone()));
                    let _ = on_line.send(line);
                }
                Ok(Ok(Event::Disconnected { reason, .. })) => {
                    let lines = {
                        let mut app = state_clone.lock().await;
                        if let ActiveConnection::Serial { ref mut assembler, .. } = app.connection {
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
                        if let ActiveConnection::Serial { ref assembler, .. } = app.connection {
                            assembler.system_line(&format!("Error: {message}"))
                        } else {
                            continue;
                        }
                    };
                    log_buf.push(LogEntry::Serial(line.clone()));
                    let _ = on_line.send(line);
                }
                Ok(Ok(Event::Shutdown)) | Ok(Err(_)) => return,
                Ok(Ok(_)) => {}
                Err(_timeout) => {
                    // Flush any partial line sitting in the buffer.
                    let partial = {
                        let mut app = state_clone.lock().await;
                        if let ActiveConnection::Serial { ref mut assembler, .. } = app.connection {
                            assembler.flush("received")
                        } else {
                            None
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
    hid_path: String,
    vid: u16,
    pid: u16,
    on_report: Channel<HidReport>,
    state: State<'_, SharedState>,
    log_buffer: State<'_, Arc<LogBuffer>>,
    status_tracker: State<'_, Arc<StatusTracker>>,
) -> Result<(), String> {
    let mut app = state.lock().await;

    // Shut down existing connection.
    shutdown_active(&mut app.connection).await;

    let log_buf = log_buffer.inner().clone();
    let status = status_tracker.inner().clone();

    let hid_path_clone = hid_path.clone();
    let session = HidSession::open(hid_path, vid, pid, move |report| {
        let _ = on_report.send(report.clone());
        log_buf.push(LogEntry::Hid(report.clone()));
        status.update_rx_bytes(report.rx_bytes_total);
    })
    .await?;

    status_tracker.set_hid(&hid_path_clone, None);

    app.connection = ActiveConnection::Hid { session };

    Ok(())
}

#[tauri::command]
pub async fn send_data(
    text: String,
    state: State<'_, SharedState>,
    log_buffer: State<'_, Arc<LogBuffer>>,
) -> Result<(), String> {
    // Grab a clone of the engine sender under the lock, then drop it.
    let engine_send = {
        let app = state.lock().await;
        match &app.connection {
            ActiveConnection::Serial { engine, .. } => engine.cmd_sender(),
            _ => return Err("Not connected (serial)".to_string()),
        }
    };

    // Log the sent line (don't feed through the shared assembler — that
    // would corrupt its partial-receive buffer).
    log_buffer.push(LogEntry::Serial(SerialLine {
        timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
        text: text.clone(),
        kind: "sent",
        rx_bytes_total: 0,
    }));

    // Send to device with newline.
    let mut data = text.into_bytes();
    data.push(b'\n');
    engine_send
        .send(Command::Send { data })
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn send_hid_report(
    data: Vec<u8>,
    report_type: String,
    state: State<'_, SharedState>,
) -> Result<(), String> {
    let app = state.lock().await;
    match &app.connection {
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
    state: State<'_, SharedState>,
) -> Result<HidDescriptorInfo, String> {
    let app = state.lock().await;
    match &app.connection {
        ActiveConnection::Hid { session } => {
            let raw = session.raw_descriptor();
            Ok(hid_descriptor::parse_hid_descriptor(raw))
        }
        _ => Err("Not connected (HID)".to_string()),
    }
}

#[tauri::command]
pub async fn disconnect(
    state: State<'_, SharedState>,
    status_tracker: State<'_, Arc<StatusTracker>>,
) -> Result<(), String> {
    let mut app = state.lock().await;
    shutdown_active(&mut app.connection).await;
    status_tracker.set_disconnected();
    Ok(())
}

async fn shutdown_active(conn: &mut ActiveConnection) {
    match std::mem::replace(conn, ActiveConnection::None) {
        ActiveConnection::Serial { engine, .. } => {
            let _ = engine.send(Command::Shutdown).await;
        }
        ActiveConnection::Hid { session } => {
            session.stop().await;
        }
        ActiveConnection::None => {}
    }
}
