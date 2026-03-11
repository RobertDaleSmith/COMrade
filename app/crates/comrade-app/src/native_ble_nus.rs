//! Native CoreBluetooth NUS session for macOS.
//!
//! Used for BLE devices that are already paired/connected to the OS but
//! invisible to btleplug (which doesn't call `retrieveConnectedPeripheralsWithServices`).
//! All CoreBluetooth work runs on a dedicated thread with its own dispatch queue.

use std::ffi::CString;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{declare_class, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_core_bluetooth::*;
use objc2_foundation::*;
use tokio::sync::mpsc;
use tracing::debug;

use crate::line_assembler::{LineAssembler, SerialLine};

// ---- NUS UUIDs ----

const NUS_SERVICE_STR: &str = "6E400001-B5A3-F393-E0A9-E50E24DCCA9E";
const NUS_RX_CHAR_STR: &str = "6E400002-B5A3-F393-E0A9-E50E24DCCA9E"; // write, host→device
const NUS_TX_CHAR_STR: &str = "6E400003-B5A3-F393-E0A9-E50E24DCCA9E"; // notify, device→host

// ---- Delegate events ----

enum CbEvent {
    StateChanged(CBManagerState),
    Connected,
    Disconnected,
    #[allow(dead_code)]
    DiscoveredServices,
    DiscoveredCharacteristics,
    NotificationData(Vec<u8>),
    NotifyStateChanged(bool),
}

// ---- Delegate class ----

declare_class!(
    #[derive(Debug)]
    struct NusDelegate;

    unsafe impl ClassType for NusDelegate {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "ComradeNusDelegate";
    }

    impl DeclaredClass for NusDelegate {
        type Ivars = std_mpsc::Sender<CbEvent>;
    }

    unsafe impl NSObjectProtocol for NusDelegate {}

    unsafe impl CBCentralManagerDelegate for NusDelegate {
        #[method(centralManagerDidUpdateState:)]
        fn did_update_state(&self, central: &CBCentralManager) {
            let state = unsafe { central.state() };
            let _ = self.ivars().send(CbEvent::StateChanged(state));
        }

        #[method(centralManager:didConnectPeripheral:)]
        fn did_connect(&self, _central: &CBCentralManager, peripheral: &CBPeripheral) {
            unsafe { peripheral.setDelegate(Some(ProtocolObject::from_ref(self))) };
            let _ = self.ivars().send(CbEvent::Connected);
        }

        #[method(centralManager:didDisconnectPeripheral:error:)]
        fn did_disconnect(
            &self,
            _central: &CBCentralManager,
            _peripheral: &CBPeripheral,
            _error: Option<&NSError>,
        ) {
            let _ = self.ivars().send(CbEvent::Disconnected);
        }

        #[method(centralManager:didFailToConnectPeripheral:error:)]
        fn did_fail_to_connect(
            &self,
            _central: &CBCentralManager,
            _peripheral: &CBPeripheral,
            _error: Option<&NSError>,
        ) {
            let _ = self.ivars().send(CbEvent::Disconnected);
        }
    }

    unsafe impl CBPeripheralDelegate for NusDelegate {
        #[method(peripheral:didDiscoverServices:)]
        fn did_discover_services(&self, peripheral: &CBPeripheral, error: Option<&NSError>) {
            if error.is_some() {
                return;
            }
            let services = unsafe { peripheral.services() }.unwrap_or_default();
            for s in services {
                unsafe { peripheral.discoverCharacteristics_forService(None, &s) };
            }
            let _ = self.ivars().send(CbEvent::DiscoveredServices);
        }

        #[method(peripheral:didDiscoverCharacteristicsForService:error:)]
        fn did_discover_characteristics(
            &self,
            _peripheral: &CBPeripheral,
            _service: &CBService,
            _error: Option<&NSError>,
        ) {
            let _ = self.ivars().send(CbEvent::DiscoveredCharacteristics);
        }

        #[method(peripheral:didUpdateValueForCharacteristic:error:)]
        fn did_update_value(
            &self,
            _peripheral: &CBPeripheral,
            characteristic: &CBCharacteristic,
            _error: Option<&NSError>,
        ) {
            let data = unsafe { characteristic.value() }
                .map(|v| v.bytes().to_vec())
                .unwrap_or_default();
            let _ = self.ivars().send(CbEvent::NotificationData(data));
        }

        #[method(peripheral:didUpdateNotificationStateForCharacteristic:error:)]
        fn did_update_notification_state(
            &self,
            _peripheral: &CBPeripheral,
            characteristic: &CBCharacteristic,
            _error: Option<&NSError>,
        ) {
            let notifying = unsafe { characteristic.isNotifying() };
            let _ = self.ivars().send(CbEvent::NotifyStateChanged(notifying));
        }

        #[method(peripheral:didWriteValueForCharacteristic:error:)]
        fn did_write_value(
            &self,
            _peripheral: &CBPeripheral,
            _characteristic: &CBCharacteristic,
            _error: Option<&NSError>,
        ) {
        }
    }
);

impl NusDelegate {
    fn new(sender: std_mpsc::Sender<CbEvent>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(sender);
        unsafe { msg_send_id![super(this), init] }
    }
}

// ---- FFI for dispatch queue ----

extern "C" {
    fn dispatch_queue_create(
        label: *const std::ffi::c_char,
        attr: *const std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
}

// ---- Commands from async world to CB thread ----

enum NusCmd {
    Write(Vec<u8>),
    Stop,
}

// ---- Public session ----

pub struct NativeBleNusSession {
    cmd_tx: mpsc::Sender<NusCmd>,
}

impl NativeBleNusSession {
    /// Open a native CoreBluetooth NUS session to an already-connected device.
    pub async fn open<F>(ble_id: String, on_line: F) -> Result<Self, String>
    where
        F: Fn(SerialLine) + Send + 'static,
    {
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<NusCmd>(32);

        std::thread::Builder::new()
            .name("native-ble-nus".into())
            .spawn(move || {
                Self::cb_thread(ble_id, on_line, result_tx, &mut cmd_rx);
            })
            .map_err(|e| format!("Failed to spawn BLE thread: {e}"))?;

        result_rx
            .await
            .map_err(|_| "BLE thread died during setup".to_string())?
            .map(|()| Self { cmd_tx })
    }

    fn cb_thread<F>(
        ble_id: String,
        on_line: F,
        result_tx: tokio::sync::oneshot::Sender<Result<(), String>>,
        cmd_rx: &mut mpsc::Receiver<NusCmd>,
    ) where
        F: Fn(SerialLine) + Send + 'static,
    {
        macro_rules! bail {
            ($tx:expr, $($arg:tt)*) => {{
                let msg = format!($($arg)*);
                let _ = $tx.send(Err(msg));
                return;
            }};
        }

        let (event_tx, event_rx) = std_mpsc::channel();
        let delegate = NusDelegate::new(event_tx);

        let label = CString::new("com.comrade.ble.nus").unwrap();
        let queue = unsafe { dispatch_queue_create(label.as_ptr(), std::ptr::null()) };

        let delegate_proto: &ProtocolObject<dyn CBCentralManagerDelegate> =
            ProtocolObject::from_ref(&*delegate);
        let queue: *mut objc2::runtime::AnyObject = queue.cast();
        let central: Retained<CBCentralManager> = unsafe {
            msg_send_id![
                CBCentralManager::alloc(),
                initWithDelegate: delegate_proto,
                queue: queue,
            ]
        };

        if !Self::wait_for_powered_on(&event_rx) {
            bail!(result_tx, "Bluetooth not powered on");
        }

        let uuid_nsstring = NSString::from_str(&ble_id);
        let nsuuid = match NSUUID::initWithUUIDString(NSUUID::alloc(), &uuid_nsstring) {
            Some(u) => u,
            None => bail!(result_tx, "Invalid UUID: {ble_id}"),
        };

        let uuid_array = NSArray::from_id_slice(&[nsuuid]);
        let peripherals =
            unsafe { central.retrievePeripheralsWithIdentifiers(&uuid_array) };

        if peripherals.is_empty() {
            bail!(result_tx, "Peripheral not found: {ble_id}");
        }

        let peripheral = peripherals.first().unwrap().retain();
        unsafe { peripheral.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };

        let state = unsafe { peripheral.state() };
        if state != CBPeripheralState::Connected {
            unsafe { central.connectPeripheral_options(&peripheral, None) };

            if !Self::wait_for_event(&event_rx, Duration::from_secs(5), |e| {
                matches!(e, CbEvent::Connected)
            }) {
                bail!(result_tx, "Connection timeout");
            }
        }

        let nus_uuid = unsafe { CBUUID::UUIDWithString(&NSString::from_str(NUS_SERVICE_STR)) };
        let svc_array = NSArray::from_id_slice(&[nus_uuid]);
        unsafe { peripheral.discoverServices(Some(&svc_array)) };

        if !Self::wait_for_event(&event_rx, Duration::from_secs(5), |e| {
            matches!(e, CbEvent::DiscoveredCharacteristics)
        }) {
            bail!(result_tx, "Service discovery timeout");
        }

        let (tx_char, rx_char) = match Self::find_nus_chars(&peripheral) {
            Some(pair) => pair,
            None => bail!(result_tx, "NUS characteristics not found"),
        };

        unsafe { peripheral.setNotifyValue_forCharacteristic(true, &tx_char) };

        if !Self::wait_for_event(&event_rx, Duration::from_secs(5), |e| {
            matches!(e, CbEvent::NotifyStateChanged(true))
        }) {
            bail!(result_tx, "Notification subscription timeout");
        }

        let _ = result_tx.send(Ok(()));

        // Main event loop.
        let mut assembler = LineAssembler::new();

        loop {
            match cmd_rx.try_recv() {
                Ok(NusCmd::Stop) => {
                    debug!("Native BLE NUS: stop command");
                    break;
                }
                Ok(NusCmd::Write(data)) => {
                    let ns_data = NSData::from_vec(data);
                    unsafe {
                        peripheral.writeValue_forCharacteristic_type(
                            &ns_data,
                            &rx_char,
                            CBCharacteristicWriteType::CBCharacteristicWriteWithoutResponse,
                        );
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            }

            match event_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(CbEvent::NotificationData(data)) => {
                    let lines = assembler.feed(&data, "received");
                    for line in lines {
                        on_line(line);
                    }
                }
                Ok(CbEvent::Disconnected) => {
                    if let Some(partial) = assembler.flush("received") {
                        on_line(partial);
                    }
                    on_line(SerialLine {
                        timestamp: chrono::Local::now()
                            .format("%H:%M:%S%.3f")
                            .to_string(),
                        text: "BLE device disconnected".to_string(),
                        kind: "system",
                        rx_bytes_total: assembler.rx_bytes,
                    });
                    break;
                }
                Ok(_) => {}
                Err(std_mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(line) = assembler.flush("received") {
                        on_line(line);
                    }
                }
                Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        // Cleanup.
        unsafe {
            peripheral.setNotifyValue_forCharacteristic(false, &tx_char);
            central.cancelPeripheralConnection(&peripheral);
        }
    }

    fn wait_for_powered_on(rx: &std_mpsc::Receiver<CbEvent>) -> bool {
        for _ in 0..20 {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(CbEvent::StateChanged(state)) if state == CBManagerState::PoweredOn => {
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    fn wait_for_event<P>(
        rx: &std_mpsc::Receiver<CbEvent>,
        timeout: Duration,
        predicate: P,
    ) -> bool
    where
        P: Fn(&CbEvent) -> bool,
    {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            match rx.recv_timeout(remaining) {
                Ok(ref e) if predicate(e) => return true,
                Ok(_) => continue,
                Err(_) => return false,
            }
        }
    }

    fn find_nus_chars(
        peripheral: &CBPeripheral,
    ) -> Option<(Retained<CBCharacteristic>, Retained<CBCharacteristic>)> {
        let services = unsafe { peripheral.services() }?;
        let mut tx_char = None;
        let mut rx_char = None;

        for service in services {
            let chars = unsafe { service.characteristics() }.unwrap_or_default();
            for c in chars {
                let uuid = unsafe { c.UUID() };
                let uuid_str = unsafe { uuid.UUIDString() }.to_string().to_uppercase();
                if uuid_str == NUS_TX_CHAR_STR {
                    tx_char = Some(c.retain());
                } else if uuid_str == NUS_RX_CHAR_STR {
                    rx_char = Some(c.retain());
                }
            }
        }

        match (tx_char, rx_char) {
            (Some(tx), Some(rx)) => Some((tx, rx)),
            _ => None,
        }
    }

    pub async fn send(&self, data: Vec<u8>) -> Result<(), String> {
        self.cmd_tx
            .send(NusCmd::Write(data))
            .await
            .map_err(|_| "Native BLE NUS session closed".to_string())
    }

    pub async fn stop(&self) {
        let _ = self.cmd_tx.send(NusCmd::Stop).await;
    }
}
