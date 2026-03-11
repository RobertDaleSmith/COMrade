use std::time::Duration;

use btleplug::api::{
    Central, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use chrono::Local;
use comrade_protocol::DeviceInfo;
use futures::StreamExt;
use tokio::sync::{mpsc, OnceCell};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::line_assembler::{LineAssembler, SerialLine};

// ---- Well-known BLE service/characteristic UUIDs ----

/// Nordic UART Service
const NUS_SERVICE: Uuid = Uuid::from_u128(0x6e400001_b5a3_f393_e0a9_e50e24dcca9e);
/// NUS RX characteristic (write, host → device)
const NUS_RX_CHAR: Uuid = Uuid::from_u128(0x6e400002_b5a3_f393_e0a9_e50e24dcca9e);
/// NUS TX characteristic (notify, device → host)
const NUS_TX_CHAR: Uuid = Uuid::from_u128(0x6e400003_b5a3_f393_e0a9_e50e24dcca9e);

// ---- BLE adapter singleton ----

static BLE_ADAPTER: OnceCell<Adapter> = OnceCell::const_new();

async fn get_adapter() -> Result<&'static Adapter, String> {
    BLE_ADAPTER
        .get_or_try_init(|| async {
            let manager = Manager::new()
                .await
                .map_err(|e| format!("BLE init: {e}"))?;
            let adapters = manager
                .adapters()
                .await
                .map_err(|e| format!("BLE adapters: {e}"))?;
            adapters
                .into_iter()
                .next()
                .ok_or_else(|| "No BLE adapter found".to_string())
        })
        .await
}

// ---- List connected BLE devices ----

/// Return nearby BLE devices advertising the Nordic UART Service (NUS).
///
/// Uses a persistent background scan filtered to the NUS service UUID so we
/// don't flood the list with every nearby BLE beacon. The scan starts once
/// and stays running; each call just reads the cached peripheral list.
pub async fn list_ble_devices() -> Result<Vec<DeviceInfo>, String> {
    let adapter = get_adapter().await?;

    // Start a persistent scan (once) filtered to NUS service UUID.
    use std::sync::atomic::{AtomicBool, Ordering};
    static SCAN_STARTED: AtomicBool = AtomicBool::new(false);
    if !SCAN_STARTED.swap(true, Ordering::Relaxed) {
        info!("Starting BLE scan for NUS devices");
        let filter = ScanFilter {
            services: vec![NUS_SERVICE],
        };
        let _ = adapter.start_scan(filter).await;
    }

    let peripherals = adapter
        .peripherals()
        .await
        .map_err(|e| format!("BLE peripherals: {e}"))?;

    let mut devices = Vec::new();

    for p in peripherals {
        let props = match p.properties().await {
            Ok(Some(props)) => props,
            _ => continue,
        };

        let name = match props.local_name {
            Some(ref n) if !n.is_empty() => n.clone(),
            _ => continue,
        };

        // Only include devices advertising NUS.
        if !props.services.contains(&NUS_SERVICE) {
            continue;
        }

        let ble_id = p.id().to_string();
        let connected = p.is_connected().await.unwrap_or(false);

        devices.push(DeviceInfo {
            path: format!("ble://{ble_id}"),
            serial_path: None,
            hid_path: None,
            vid: None,
            pid: None,
            serial_number: None,
            manufacturer: if connected {
                Some("Connected".to_string())
            } else {
                None
            },
            product: Some(name),
            kind: comrade_protocol::DeviceKind::Ble,
            hid_usage: None,
            ble_id: Some(ble_id),
            ble_services: Some(vec!["nus".to_string()]),
        });
    }

    devices.sort_by(|a, b| a.product.cmp(&b.product));
    Ok(devices)
}

// ---- Find peripheral by ID ----

async fn find_peripheral(ble_id: &str) -> Result<Peripheral, String> {
    let adapter = get_adapter().await?;
    let peripherals = adapter
        .peripherals()
        .await
        .map_err(|e| format!("BLE peripherals: {e}"))?;

    for p in peripherals {
        if p.id().to_string() == ble_id {
            return Ok(p);
        }
    }
    Err(format!("BLE device not found: {ble_id}"))
}

fn find_characteristic(chars: &[Characteristic], uuid: Uuid) -> Option<Characteristic> {
    chars.iter().find(|c| c.uuid == uuid).cloned()
}

// ---- BLE NUS Session ----

enum BleCommand {
    Stop,
    Send { data: Vec<u8> },
}

pub struct BleNusSession {
    cmd_tx: mpsc::Sender<BleCommand>,
}

impl BleNusSession {
    /// Connect to a BLE NUS device and stream lines via `on_line`.
    pub async fn open<F>(ble_id: String, on_line: F) -> Result<Self, String>
    where
        F: Fn(SerialLine) + Send + 'static,
    {
        let peripheral = find_peripheral(&ble_id).await?;

        if !peripheral.is_connected().await.unwrap_or(false) {
            peripheral
                .connect()
                .await
                .map_err(|e| format!("BLE connect: {e}"))?;
        }

        peripheral
            .discover_services()
            .await
            .map_err(|e| format!("BLE discover services: {e}"))?;

        let chars = peripheral.characteristics();
        let chars_vec: Vec<Characteristic> = chars.into_iter().collect();

        let tx_char = find_characteristic(&chars_vec, NUS_TX_CHAR)
            .ok_or("NUS TX characteristic not found")?;
        let rx_char = find_characteristic(&chars_vec, NUS_RX_CHAR)
            .ok_or("NUS RX characteristic not found")?;

        // Subscribe to TX notifications (device → host)
        peripheral
            .subscribe(&tx_char)
            .await
            .map_err(|e| format!("BLE subscribe TX: {e}"))?;

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<BleCommand>(32);

        let p = peripheral.clone();
        tokio::spawn(async move {
            let mut assembler = LineAssembler::new();
            let mut notifications = match p.notifications().await {
                Ok(n) => n,
                Err(e) => {
                    error!("BLE notifications stream: {e}");
                    return;
                }
            };

            info!("BLE NUS read loop started for {ble_id}");

            loop {
                tokio::select! {
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(BleCommand::Stop) | None => {
                                debug!("BLE NUS: stop");
                                break;
                            }
                            Some(BleCommand::Send { data }) => {
                                if let Err(e) = p.write(&rx_char, &data, WriteType::WithoutResponse).await {
                                    warn!("BLE NUS write error: {e}");
                                    on_line(SerialLine {
                                        timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
                                        text: format!("BLE write error: {e}"),
                                        kind: "system",
                                        rx_bytes_total: assembler.rx_bytes,
                                    });
                                }
                            }
                        }
                    }
                    notification = notifications.next() => {
                        match notification {
                            Some(data) => {
                                let lines = assembler.feed(&data.value, "received");
                                for line in lines {
                                    on_line(line);
                                }
                            }
                            None => {
                                // Stream ended — device disconnected
                                if let Some(partial) = assembler.flush("received") {
                                    on_line(partial);
                                }
                                on_line(SerialLine {
                                    timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
                                    text: "BLE device disconnected".to_string(),
                                    kind: "system",
                                    rx_bytes_total: assembler.rx_bytes,
                                });
                                break;
                            }
                        }
                    }
                    // Flush partial lines periodically
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        if let Some(line) = assembler.flush("received") {
                            on_line(line);
                        }
                    }
                }
            }

            let _ = p.disconnect().await;
            info!("BLE NUS session ended for {ble_id}");
        });

        Ok(Self { cmd_tx })
    }

    pub async fn send(&self, data: Vec<u8>) -> Result<(), String> {
        self.cmd_tx
            .send(BleCommand::Send { data })
            .await
            .map_err(|_| "BLE NUS session closed".to_string())
    }

    pub async fn stop(&self) {
        let _ = self.cmd_tx.send(BleCommand::Stop).await;
    }
}
