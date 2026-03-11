use serde::{Deserialize, Serialize};

use crate::{SerialConfig, Timestamp};

/// Events emitted by the engine to frontends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    /// Raw data received from the serial port.
    Data {
        ts: Timestamp,
        bytes: Vec<u8>,
    },
    /// Port successfully opened.
    Connected {
        ts: Timestamp,
        port: String,
        config: SerialConfig,
    },
    /// Port disconnected (intentional or error).
    Disconnected {
        ts: Timestamp,
        port: String,
        reason: String,
    },
    /// Attempting to reconnect.
    Reconnecting {
        ts: Timestamp,
        port: String,
        attempt: u32,
    },
    /// Error that doesn't cause a disconnect.
    Error {
        ts: Timestamp,
        message: String,
    },
    /// Available serial ports (response to ListPorts command).
    PortList {
        ts: Timestamp,
        ports: Vec<PortInfo>,
    },
    /// Engine is shutting down.
    Shutdown,
}

/// Information about an available serial port.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortInfo {
    /// Device path (e.g. /dev/cu.usbserial-1420).
    pub path: String,
    /// USB vendor ID, if available.
    pub vid: Option<u16>,
    /// USB product ID, if available.
    pub pid: Option<u16>,
    /// USB serial number, if available.
    pub serial_number: Option<String>,
    /// USB manufacturer string, if available.
    pub manufacturer: Option<String>,
    /// USB product string, if available.
    pub product: Option<String>,
}

/// Whether a device is Serial, HID, BLE, or a combination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceKind {
    Serial,
    Hid,
    Both,
    Ble,
}

/// HID usage page + usage ID with optional human-readable name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HidUsageInfo {
    pub usage_page: u16,
    pub usage: u16,
    pub usage_name: Option<String>,
}

/// Unified device info covering Serial, HID, or composite devices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Display path (serial path or HID path, whichever is primary).
    pub path: String,
    /// Serial port path, if this device has a serial interface.
    pub serial_path: Option<String>,
    /// HID device path, if this device has a HID interface.
    pub hid_path: Option<String>,
    /// USB vendor ID.
    pub vid: Option<u16>,
    /// USB product ID.
    pub pid: Option<u16>,
    /// USB serial number.
    pub serial_number: Option<String>,
    /// USB manufacturer string.
    pub manufacturer: Option<String>,
    /// USB product string.
    pub product: Option<String>,
    /// Device kind.
    pub kind: DeviceKind,
    /// HID usage info, if this is a HID device.
    pub hid_usage: Option<HidUsageInfo>,
    /// BLE peripheral identifier (platform-specific).
    pub ble_id: Option<String>,
    /// BLE services advertised (e.g. "nus", "hid").
    pub ble_services: Option<Vec<String>>,
    /// Transport bus type (e.g. "USB", "Bluetooth", "I2C", "SPI").
    pub bus_type: Option<String>,
}
