use std::collections::HashMap;

use comrade_protocol::{DeviceInfo, DeviceKind, HidUsageInfo, PortInfo};

use crate::CoreError;

/// Enumerate available serial ports. This calls the blocking `serialport` API,
/// so it should be called from a blocking context (e.g. `spawn_blocking`).
pub fn enumerate_ports() -> Result<Vec<PortInfo>, CoreError> {
    let ports = serialport::available_ports()?;
    Ok(ports.into_iter().map(port_to_info).collect())
}

/// Enumerate all devices (serial + HID), merging composite devices by (vid, pid, serial_number).
pub fn enumerate_devices() -> Result<Vec<DeviceInfo>, CoreError> {
    // Merge key: (vid, pid, serial_number) — None if no USB info.
    type MergeKey = (u16, u16, String);

    let mut by_key: HashMap<MergeKey, DeviceInfo> = HashMap::new();
    let mut unkeyed: Vec<DeviceInfo> = Vec::new();

    // Serial ports.
    let serial_ports = serialport::available_ports().unwrap_or_default();
    for port in serial_ports {
        // On macOS, skip /dev/tty.* — cu.* is the correct one for connections.
        if port.port_name.starts_with("/dev/tty.") {
            continue;
        }
        let info = port_to_info(port);
        if let (Some(vid), Some(pid)) = (info.vid, info.pid) {
            let key = (vid, pid, info.serial_number.clone().unwrap_or_default());
            let entry = by_key.entry(key).or_insert_with(|| DeviceInfo {
                path: info.path.clone(),
                serial_path: None,
                hid_path: None,
                vid: Some(vid),
                pid: Some(pid),
                serial_number: info.serial_number.clone(),
                manufacturer: info.manufacturer.clone(),
                product: info.product.clone(),
                kind: DeviceKind::Serial,
                hid_usage: None,
                ble_id: None,
                ble_services: None,
                bus_type: Some("USB".to_string()),
            });
            entry.serial_path = Some(info.path.clone());
            // If it already existed as HID, upgrade to Both.
            if entry.kind == DeviceKind::Hid {
                entry.kind = DeviceKind::Both;
                entry.path = info.path;
            }
            // Fill in missing descriptive fields from serial info.
            if entry.manufacturer.is_none() {
                entry.manufacturer = info.manufacturer;
            }
            if entry.product.is_none() {
                entry.product = info.product;
            }
        } else {
            unkeyed.push(DeviceInfo {
                path: info.path.clone(),
                serial_path: Some(info.path),
                hid_path: None,
                vid: None,
                pid: None,
                serial_number: None,
                manufacturer: info.manufacturer,
                product: info.product,
                kind: DeviceKind::Serial,
                hid_usage: None,
                ble_id: None,
                ble_services: None,
                bus_type: None,
            });
        }
    }

    // HID devices.
    if let Ok(api) = hidapi::HidApi::new() {
        for dev in api.device_list() {
            let vid = dev.vendor_id();
            let pid = dev.product_id();
            let serial = dev
                .serial_number()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let hid_path = dev.path().to_string_lossy().to_string();
            let manufacturer = dev.manufacturer_string().map(|s| s.to_string());
            let product = dev.product_string().map(|s| s.to_string());

            let usage = HidUsageInfo {
                usage_page: dev.usage_page(),
                usage: dev.usage(),
                usage_name: None,
            };

            let bus = match dev.bus_type() {
                hidapi::BusType::Usb => Some("USB".to_string()),
                hidapi::BusType::Bluetooth => Some("Bluetooth".to_string()),
                hidapi::BusType::I2c => Some("I2C".to_string()),
                hidapi::BusType::Spi => Some("SPI".to_string()),
                _ => None,
            };

            let key = (vid, pid, serial.clone());
            let entry = by_key.entry(key).or_insert_with(|| DeviceInfo {
                path: hid_path.clone(),
                serial_path: None,
                hid_path: None,
                vid: Some(vid),
                pid: Some(pid),
                serial_number: if serial.is_empty() {
                    None
                } else {
                    Some(serial)
                },
                manufacturer: manufacturer.clone(),
                product: product.clone(),
                kind: DeviceKind::Hid,
                hid_usage: None,
                ble_id: None,
                ble_services: None,
                bus_type: bus.clone(),
            });

            entry.hid_path = Some(hid_path);
            entry.hid_usage = Some(usage);

            // If it already existed as Serial, upgrade to Both.
            if entry.kind == DeviceKind::Serial {
                entry.kind = DeviceKind::Both;
            }
            // Fill in bus type from HID info if not already set.
            if entry.bus_type.is_none() {
                entry.bus_type = bus;
            }
            // Fill in missing descriptive fields from HID info.
            if entry.manufacturer.is_none() {
                entry.manufacturer = manufacturer;
            }
            if entry.product.is_none() {
                entry.product = product;
            }
        }
    }

    let mut devices: Vec<DeviceInfo> = by_key.into_values().collect();
    devices.append(&mut unkeyed);

    // Sort: Both first, then Serial, then HID; within each kind by product name.
    devices.sort_by(|a, b| {
        let kind_ord = |k: &DeviceKind| match k {
            DeviceKind::Both => 0,
            DeviceKind::Serial => 1,
            DeviceKind::Hid => 2,
            DeviceKind::Ble => 3,
        };
        kind_ord(&a.kind)
            .cmp(&kind_ord(&b.kind))
            .then_with(|| a.product.cmp(&b.product))
            .then_with(|| a.path.cmp(&b.path))
    });

    Ok(devices)
}

fn port_to_info(port: serialport::SerialPortInfo) -> PortInfo {
    let (vid, pid, serial_number, manufacturer, product) = match port.port_type {
        serialport::SerialPortType::UsbPort(usb) => (
            Some(usb.vid),
            Some(usb.pid),
            usb.serial_number,
            usb.manufacturer,
            usb.product,
        ),
        _ => (None, None, None, None, None),
    };

    PortInfo {
        path: port.port_name,
        vid,
        pid,
        serial_number,
        manufacturer,
        product,
    }
}
