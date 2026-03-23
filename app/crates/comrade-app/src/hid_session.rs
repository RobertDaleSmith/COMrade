use std::ffi::CString;
use std::io::Read;
use std::time::Duration;

use chrono::Local;
use nusb::descriptors::TransferType;
use nusb::transfer::{Direction, In, Interrupt};
use nusb::MaybeFuture;
use serde::Serialize;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

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
    ///
    /// `vid` and `pid` are used as a fallback to locate the device via nusb
    /// when hidapi cannot read input reports (e.g. composite CDC+HID on macOS).
    pub async fn open<F>(
        hid_path: String,
        vid: u16,
        pid: u16,
        on_report: F,
    ) -> Result<Self, String>
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
            Self::read_loop(device, vid, pid, cmd_rx, on_report);
        });

        debug!(
            "HID session opened: {hid_path} (descriptor {} bytes)",
            raw_descriptor_clone.len()
        );

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
        vid: u16,
        pid: u16,
        mut cmd_rx: mpsc::Receiver<HidCommand>,
        on_report: F,
    ) where
        F: Fn(HidReport),
    {
        let mut buf = vec![0u8; 4096];
        let mut report_count: u64 = 0;
        let mut rx_bytes_total: u64 = 0;

        // Test read — detect devices where hidapi can't receive reports.
        // Some composite CDC+HID devices on macOS fail here with IOHidManager.
        match device.read_timeout(&mut buf, 500) {
            Ok(n) if n > 0 => {
                // First report succeeded — process it and continue with hidapi.
                process_report(&buf[..n], &mut report_count, &mut rx_bytes_total, &on_report);
            }
            Ok(_) => {
                // Timeout (device idle) — hidapi seems fine, continue.
            }
            Err(e) => {
                // hidapi read failed — fall back to nusb.
                warn!("hidapi read failed ({e}), falling back to nusb");
                drop(device);
                match open_nusb_readers(vid, pid) {
                    Ok(mut readers) => {
                        nusb_read_loop(
                            &mut readers,
                            &mut cmd_rx,
                            &mut report_count,
                            &mut rx_bytes_total,
                            &on_report,
                        );
                    }
                    Err(e) => {
                        error!("nusb fallback failed: {e}");
                        let now = Local::now().format("%H:%M:%S%.3f").to_string();
                        on_report(HidReport {
                            timestamp: now,
                            data: Vec::new(),
                            hex: format!("nusb fallback failed: {e}"),
                            ascii: String::new(),
                            report_id: None,
                            report_count,
                            rx_bytes_total,
                            kind: "error",
                        });
                    }
                }
                debug!("HID read loop ended (reports={report_count}, bytes={rx_bytes_total})");
                return;
            }
        }

        // hidapi read loop — continues when the test read succeeded or timed out.
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
                    process_report(&buf[..n], &mut report_count, &mut rx_bytes_total, &on_report);
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

/// Process a single HID report and send it via the callback.
fn process_report<F>(
    data: &[u8],
    report_count: &mut u64,
    rx_bytes_total: &mut u64,
    on_report: &F,
) where
    F: Fn(HidReport),
{
    let n = data.len();
    let data = data.to_vec();
    *report_count += 1;
    *rx_bytes_total += n as u64;

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
        report_count: *report_count,
        rx_bytes_total: *rx_bytes_total,
        kind: "input",
    });
}

/// Holds a nusb interface claim and its interrupt IN reader.
struct NusbReader {
    reader: nusb::io::EndpointRead<Interrupt>,
    _interface: nusb::Interface,
}

/// Open nusb interrupt IN readers for ALL HID interfaces of a device.
fn open_nusb_readers(
    vid: u16,
    pid: u16,
) -> Result<Vec<NusbReader>, String> {
    let dev_info = nusb::list_devices()
        .wait()
        .map_err(|e| format!("nusb enumerate: {e}"))?
        .find(|d| d.vendor_id() == vid && d.product_id() == pid)
        .ok_or("Device not found via nusb")?;

    let device = dev_info
        .open()
        .wait()
        .map_err(|e| format!("nusb open: {e}"))?;

    let config = device
        .active_configuration()
        .map_err(|e| format!("nusb config: {e}"))?;

    // Find ALL HID interfaces (class 0x03) with interrupt IN endpoints.
    let hid_endpoints: Vec<(u8, u8)> = config
        .interface_alt_settings()
        .filter(|i| i.class() == 0x03)
        .filter_map(|i| {
            i.endpoints()
                .find(|e| {
                    e.transfer_type() == TransferType::Interrupt
                        && e.direction() == Direction::In
                })
                .map(|e| (i.interface_number(), e.address()))
        })
        .collect();

    if hid_endpoints.is_empty() {
        return Err("No HID interrupt IN endpoints found".to_string());
    }

    let mut readers = Vec::new();
    for (intf_num, ep_addr) in hid_endpoints {
        info!("nusb: claiming interface {intf_num}, endpoint 0x{ep_addr:02X}");

        let interface = device
            .detach_and_claim_interface(intf_num)
            .wait()
            .map_err(|e| format!("nusb claim interface {intf_num}: {e}"))?;

        let endpoint = interface
            .endpoint::<Interrupt, In>(ep_addr)
            .map_err(|e| format!("nusb endpoint: {e}"))?;

        let max_packet = endpoint.max_packet_size();
        let reader = endpoint
            .reader(max_packet)
            .with_read_timeout(Duration::from_millis(100));

        readers.push(NusbReader {
            reader,
            _interface: interface,
        });
    }

    Ok(readers)
}

/// Read loop using nusb, polls all HID interface readers round-robin.
fn nusb_read_loop<F>(
    readers: &mut [NusbReader],
    cmd_rx: &mut mpsc::Receiver<HidCommand>,
    report_count: &mut u64,
    rx_bytes_total: &mut u64,
    on_report: &F,
) where
    F: Fn(HidReport),
{
    info!("nusb: read loop started ({} interface(s))", readers.len());
    let mut buf = vec![0u8; 4096];

    loop {
        // Check for commands (non-blocking).
        match cmd_rx.try_recv() {
            Ok(HidCommand::Stop) => {
                debug!("nusb read loop: stop command received");
                break;
            }
            Ok(HidCommand::SendOutputReport { .. }) => {
                warn!("Output reports not supported in nusb fallback mode");
            }
            Ok(HidCommand::SendFeatureReport { .. }) => {
                warn!("Feature reports not supported in nusb fallback mode");
            }
            Err(mpsc::error::TryRecvError::Empty) => {}
            Err(mpsc::error::TryRecvError::Disconnected) => {
                debug!("nusb read loop: command channel closed");
                break;
            }
        }

        // Poll all readers round-robin.
        let mut any_error = false;
        for nusb_reader in readers.iter_mut() {
            match nusb_reader.reader.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => {
                    process_report(&buf[..n], report_count, rx_bytes_total, on_report);
                }
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => {
                    error!("nusb read error: {e}");
                    let now = Local::now().format("%H:%M:%S%.3f").to_string();
                    on_report(HidReport {
                        timestamp: now,
                        data: Vec::new(),
                        hex: format!("nusb read error: {e}"),
                        ascii: String::new(),
                        report_id: None,
                        report_count: *report_count,
                        rx_bytes_total: *rx_bytes_total,
                        kind: "error",
                    });
                    any_error = true;
                    break;
                }
            }
        }
        if any_error {
            break;
        }
    }

    debug!(
        "nusb read loop ended (reports={}, bytes={})",
        report_count, rx_bytes_total
    );
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
