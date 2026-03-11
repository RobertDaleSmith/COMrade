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

// ---- macOS: retrieve connected peripherals via CoreBluetooth ----

/// Use CoreBluetooth directly to find already-paired peripherals with a given service.
/// Returns (uuid_string, name) pairs.
/// btleplug doesn't call `retrieveConnectedPeripheralsWithServices`, so paired devices
/// that stopped advertising are invisible to it. This fills the gap.
#[cfg(target_os = "macos")]
fn native_connected_nus_peripherals() -> Vec<(String, String)> {
    use objc2_core_bluetooth::{CBCentralManager, CBManagerState, CBUUID};
    use objc2_foundation::{NSArray, NSString};

    // CBCentralManager is !Send, so create and use it on a dedicated thread.
    // Delegate callbacks go to the main dispatch queue (Tauri's run loop), so
    // we just poll `state()` with short sleeps to let the main thread process them.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let central = unsafe { CBCentralManager::new() };

        // Wait for main run loop to process the PoweredOn state callback.
        for _ in 0..20 {
            if unsafe { central.state() } == CBManagerState::PoweredOn {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        let state = unsafe { central.state() };
        if state != CBManagerState::PoweredOn {
            warn!("CoreBluetooth: state={:?}, cannot query peripherals", state);
            let _ = tx.send(Vec::new());
            return;
        }

        let nus_uuid_str = NSString::from_str(&NUS_SERVICE.to_string());
        let nus_cbuuid = unsafe { CBUUID::UUIDWithString(&nus_uuid_str) };
        let services = NSArray::from_id_slice(&[nus_cbuuid]);

        let peripherals =
            unsafe { central.retrieveConnectedPeripheralsWithServices(&services) };

        info!(
            "CoreBluetooth: found {} connected NUS peripherals",
            peripherals.len()
        );

        let mut results = Vec::new();
        for peripheral in peripherals {
            let uuid = unsafe { peripheral.identifier() };
            let uuid_str = uuid.UUIDString().to_string();

            let name = unsafe { peripheral.name() }
                .map(|n| n.to_string())
                .unwrap_or_default();

            info!("CoreBluetooth: peripheral uuid={uuid_str} name={name:?}");

            if !name.is_empty() {
                results.push((uuid_str, name));
            }
        }
        let _ = tx.send(results);
    });

    rx.recv().unwrap_or_default()
}

#[cfg(not(target_os = "macos"))]
fn native_connected_nus_peripherals() -> Vec<(String, String)> {
    Vec::new()
}

// ---- List connected BLE devices ----

/// Return BLE devices that support the Nordic UART Service (NUS).
///
/// Combines two discovery methods:
/// 1. CoreBluetooth `retrieveConnectedPeripheralsWithServices` — finds already-paired
///    devices that are connected but not advertising (macOS only).
/// 2. btleplug scan — finds advertising peripherals.
pub async fn list_ble_devices() -> Result<Vec<DeviceInfo>, String> {
    let adapter = get_adapter().await?;

    // Start a persistent unfiltered scan (once) so we can see advertising peripherals.
    use std::sync::atomic::{AtomicBool, Ordering};
    static SCAN_STARTED: AtomicBool = AtomicBool::new(false);
    if !SCAN_STARTED.swap(true, Ordering::Relaxed) {
        info!("Starting BLE scan");
        let _ = adapter.start_scan(ScanFilter::default()).await;
    }

    let peripherals = adapter
        .peripherals()
        .await
        .map_err(|e| format!("BLE peripherals: {e}"))?;

    let mut devices = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    for p in peripherals {
        let props = match p.properties().await {
            Ok(Some(props)) => props,
            _ => continue,
        };

        let name = match props.local_name {
            Some(ref n) if !n.is_empty() => n.clone(),
            _ => continue,
        };

        let connected = p.is_connected().await.unwrap_or(false);

        // Check if NUS is advertised in properties.
        let mut has_nus = props.services.contains(&NUS_SERVICE);

        // If connected but NUS not in advertisements, try service discovery.
        if !has_nus && connected && p.discover_services().await.is_ok() {
            let chars = p.characteristics();
            has_nus = chars.iter().any(|c| c.uuid == NUS_TX_CHAR || c.uuid == NUS_RX_CHAR);
        }

        if !has_nus {
            continue;
        }

        let ble_id = p.id().to_string();
        seen_ids.insert(ble_id.clone());

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
            bus_type: Some("Bluetooth".to_string()),
        });
    }

    // Add native-discovered connected peripherals that btleplug missed.
    let native = tokio::task::spawn_blocking(native_connected_nus_peripherals)
        .await
        .unwrap_or_default();

    for (uuid_str, name) in native {
        if seen_ids.contains(&uuid_str) {
            continue;
        }
        devices.push(DeviceInfo {
            path: format!("ble://{uuid_str}"),
            serial_path: None,
            hid_path: None,
            vid: None,
            pid: None,
            serial_number: None,
            manufacturer: Some("Connected".to_string()),
            product: Some(name),
            kind: comrade_protocol::DeviceKind::Ble,
            hid_usage: None,
            ble_id: Some(uuid_str),
            ble_services: Some(vec!["nus".to_string()]),
            bus_type: Some("Bluetooth".to_string()),
        });
    }

    devices.sort_by(|a, b| a.product.cmp(&b.product));
    Ok(devices)
}

/// Debug: return all BLE peripherals visible to btleplug with their properties.
pub async fn debug_ble_peripherals() -> Result<Vec<String>, String> {
    let adapter = get_adapter().await?;
    let peripherals = adapter
        .peripherals()
        .await
        .map_err(|e| format!("BLE peripherals: {e}"))?;

    let mut results = Vec::new();

    // Show native-discovered connected NUS peripherals first.
    let native = tokio::task::spawn_blocking(native_connected_nus_peripherals)
        .await
        .unwrap_or_default();
    for (uuid, name) in &native {
        results.push(format!("[native CB] {name} | id={uuid} | connected=true (system-paired)"));
    }

    for p in peripherals {
        let props = match p.properties().await {
            Ok(Some(props)) => props,
            Ok(None) => {
                results.push(format!("{}: <no properties>", p.id()));
                continue;
            }
            Err(e) => {
                results.push(format!("{}: <error: {e}>", p.id()));
                continue;
            }
        };

        let name = props.local_name.as_deref().unwrap_or("<unnamed>");
        let connected = p.is_connected().await.unwrap_or(false);
        let adv_services: Vec<String> = props.services.iter().map(|u| u.to_string()).collect();

        // Try service discovery if connected.
        let mut discovered_services = Vec::new();
        if connected && p.discover_services().await.is_ok() {
            for s in p.services() {
                discovered_services.push(s.uuid.to_string());
            }
        }

        results.push(format!(
            "{name} | id={} | connected={connected} | adv_services=[{}] | discovered_services=[{}]",
            p.id(),
            adv_services.join(", "),
            discovered_services.join(", "),
        ));
    }

    Ok(results)
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
