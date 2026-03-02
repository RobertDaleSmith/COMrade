use std::ffi::CString;

use chrono::Local;
use serde::Serialize;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

/// A report received from a HID device, sent to the frontend via Tauri Channel.
#[derive(Debug, Clone, Serialize)]
pub struct HidReport {
    pub timestamp: String,
    pub data: Vec<u8>,
    pub hex: String,
    pub ascii: String,
    pub report_id: Option<u8>,
    pub report_count: u64,
    pub rx_bytes_total: u64,
    pub kind: &'static str,
}

/// Commands that can be sent to the HID read loop.
pub enum HidCommand {
    Stop,
    SendOutputReport { data: Vec<u8> },
    SendFeatureReport { data: Vec<u8> },
}

/// Manages an open HID device connection.
pub struct HidSession {
    cmd_tx: mpsc::Sender<HidCommand>,
    raw_descriptor: Vec<u8>,
}

impl HidSession {
    /// Open a HID device by path. Spawns a blocking read loop that streams
    /// reports via the `on_report` callback. Returns the session handle.
    pub async fn open<F>(hid_path: String, on_report: F) -> Result<Self, String>
    where
        F: Fn(HidReport) + Send + 'static,
    {
        let (cmd_tx, cmd_rx) = mpsc::channel::<HidCommand>(32);

        // Open device and grab descriptor on the blocking pool.
        let path_clone = hid_path.clone();
        let (device, raw_descriptor) = tokio::task::spawn_blocking(move || {
            let api = hidapi::HidApi::new().map_err(|e| format!("HidApi init: {e}"))?;
            let c_path =
                CString::new(path_clone.as_bytes()).map_err(|e| format!("Invalid path: {e}"))?;
            let device = api.open_path(&c_path).map_err(|e| format!("Open HID: {e}"))?;

            // Get raw report descriptor.
            let mut desc_buf = vec![0u8; hidapi::MAX_REPORT_DESCRIPTOR_SIZE];
            let desc_len = device
                .get_report_descriptor(&mut desc_buf)
                .unwrap_or(0);
            desc_buf.truncate(desc_len);

            Ok::<_, String>((device, desc_buf))
        })
        .await
        .map_err(|e| format!("spawn_blocking: {e}"))??;

        let raw_descriptor_clone = raw_descriptor.clone();

        // Move device into the blocking read loop thread.
        // HidDevice is Send but not Sync, so it lives exclusively in one thread.
        tokio::task::spawn_blocking(move || {
            Self::read_loop(device, cmd_rx, on_report);
        });

        debug!("HID session opened: {hid_path} (descriptor {} bytes)", raw_descriptor_clone.len());

        Ok(Self {
            cmd_tx,
            raw_descriptor: raw_descriptor_clone,
        })
    }

    /// Get the raw HID report descriptor bytes.
    pub fn raw_descriptor(&self) -> &[u8] {
        &self.raw_descriptor
    }

    /// Send an output report to the device.
    pub async fn send_output_report(&self, data: Vec<u8>) -> Result<(), String> {
        self.cmd_tx
            .send(HidCommand::SendOutputReport { data })
            .await
            .map_err(|_| "HID session closed".to_string())
    }

    /// Send a feature report to the device.
    pub async fn send_feature_report(&self, data: Vec<u8>) -> Result<(), String> {
        self.cmd_tx
            .send(HidCommand::SendFeatureReport { data })
            .await
            .map_err(|_| "HID session closed".to_string())
    }

    /// Stop the HID session.
    pub async fn stop(&self) {
        let _ = self.cmd_tx.send(HidCommand::Stop).await;
    }

    fn read_loop<F>(
        device: hidapi::HidDevice,
        mut cmd_rx: mpsc::Receiver<HidCommand>,
        on_report: F,
    ) where
        F: Fn(HidReport),
    {
        let mut buf = vec![0u8; 4096];
        let mut report_count: u64 = 0;
        let mut rx_bytes_total: u64 = 0;

        loop {
            // Check for commands (non-blocking).
            match cmd_rx.try_recv() {
                Ok(HidCommand::Stop) => {
                    debug!("HID read loop: stop command received");
                    break;
                }
                Ok(HidCommand::SendOutputReport { data }) => {
                    if let Err(e) = device.write(&data) {
                        warn!("HID write error: {e}");
                        let now = Local::now().format("%H:%M:%S%.3f").to_string();
                        on_report(HidReport {
                            timestamp: now,
                            data: Vec::new(),
                            hex: String::new(),
                            ascii: String::new(),
                            report_id: None,
                            report_count,
                            rx_bytes_total,
                            kind: "error",
                        });
                    }
                }
                Ok(HidCommand::SendFeatureReport { data }) => {
                    if let Err(e) = device.send_feature_report(&data) {
                        warn!("HID feature report error: {e}");
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    debug!("HID read loop: command channel closed");
                    break;
                }
            }

            // Read with 100ms timeout so we can check commands regularly.
            match device.read_timeout(&mut buf, 100) {
                Ok(0) => {
                    // Timeout, no data — loop back to check commands.
                }
                Ok(n) => {
                    let data = buf[..n].to_vec();
                    report_count += 1;
                    rx_bytes_total += n as u64;

                    let now = Local::now().format("%H:%M:%S%.3f").to_string();
                    let report_id = if n > 0 { Some(data[0]) } else { None };
                    let hex = format_hex(&data);
                    let ascii = format_ascii(&data);

                    on_report(HidReport {
                        timestamp: now,
                        data,
                        hex,
                        ascii,
                        report_id,
                        report_count,
                        rx_bytes_total,
                        kind: "input",
                    });
                }
                Err(e) => {
                    error!("HID read error: {e}");
                    let now = Local::now().format("%H:%M:%S%.3f").to_string();
                    on_report(HidReport {
                        timestamp: now,
                        data: Vec::new(),
                        hex: format!("Read error: {e}"),
                        ascii: String::new(),
                        report_id: None,
                        report_count,
                        rx_bytes_total,
                        kind: "error",
                    });
                    break;
                }
            }
        }

        debug!("HID read loop ended (reports={report_count}, bytes={rx_bytes_total})");
    }
}

fn format_hex(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_ascii(data: &[u8]) -> String {
    data.iter()
        .map(|&b| if (0x20..=0x7E).contains(&b) { b as char } else { '.' })
        .collect()
}
