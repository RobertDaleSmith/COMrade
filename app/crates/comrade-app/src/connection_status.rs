use std::sync::Mutex;

use serde::Serialize;

/// Serializable snapshot of the current connection state.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionStatus {
    /// "disconnected", "serial", or "hid"
    pub state: String,
    pub device_path: Option<String>,
    pub baud_rate: Option<u32>,
    pub rx_bytes: u64,
    pub device_product: Option<String>,
}

impl ConnectionStatus {
    fn disconnected() -> Self {
        Self {
            state: "disconnected".to_string(),
            device_path: None,
            baud_rate: None,
            rx_bytes: 0,
            device_product: None,
        }
    }
}

/// Thread-safe tracker for the current connection status.
///
/// Uses `std::sync::Mutex` — all ops are fast in-memory.
pub struct StatusTracker {
    inner: Mutex<ConnectionStatus>,
}

impl StatusTracker {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(ConnectionStatus::disconnected()),
        }
    }

    pub fn set_serial(&self, port: &str, baud: u32) {
        let mut s = self.inner.lock().unwrap();
        s.state = "serial".to_string();
        s.device_path = Some(port.to_string());
        s.baud_rate = Some(baud);
        s.rx_bytes = 0;
        s.device_product = None;
    }

    pub fn set_hid(&self, path: &str, product: Option<&str>) {
        let mut s = self.inner.lock().unwrap();
        s.state = "hid".to_string();
        s.device_path = Some(path.to_string());
        s.baud_rate = None;
        s.rx_bytes = 0;
        s.device_product = product.map(|p| p.to_string());
    }

    pub fn set_ble_nus(&self, ble_id: &str) {
        let mut s = self.inner.lock().unwrap();
        s.state = "ble_nus".to_string();
        s.device_path = Some(ble_id.to_string());
        s.baud_rate = None;
        s.rx_bytes = 0;
        s.device_product = None;
    }

    pub fn set_disconnected(&self) {
        *self.inner.lock().unwrap() = ConnectionStatus::disconnected();
    }

    pub fn update_rx_bytes(&self, total: u64) {
        self.inner.lock().unwrap().rx_bytes = total;
    }

    pub fn snapshot(&self) -> ConnectionStatus {
        self.inner.lock().unwrap().clone()
    }
}
