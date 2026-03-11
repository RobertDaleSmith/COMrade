use crate::*;
use chrono::Utc;

#[test]
fn test_serial_config_default() {
    let config = SerialConfig::default();
    assert_eq!(config.baud_rate, 115200);
    assert_eq!(config.data_bits, DataBits::Eight);
    assert_eq!(config.parity, Parity::None);
    assert_eq!(config.stop_bits, StopBits::One);
    assert_eq!(config.flow_control, FlowControl::None);
}

#[test]
fn test_serial_config_roundtrip_json() {
    let config = SerialConfig {
        baud_rate: 9600,
        data_bits: DataBits::Seven,
        parity: Parity::Even,
        stop_bits: StopBits::Two,
        flow_control: FlowControl::Hardware,
    };
    let json = serde_json::to_string(&config).unwrap();
    let parsed: SerialConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(config, parsed);
}

#[test]
fn test_event_serialization() {
    let event = Event::Data {
        ts: Timestamp::new(Utc::now(), 12345),
        bytes: vec![0x48, 0x65, 0x6c, 0x6c, 0x6f],
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("Data"));
    assert!(json.contains("12345"));
}

#[test]
fn test_command_serialization() {
    let cmd = Command::Connect {
        port: "/dev/ttyUSB0".into(),
        config: SerialConfig::default(),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    let parsed: Command = serde_json::from_str(&json).unwrap();
    match parsed {
        Command::Connect { port, config } => {
            assert_eq!(port, "/dev/ttyUSB0");
            assert_eq!(config.baud_rate, 115200);
        }
        _ => panic!("expected Connect"),
    }
}

#[test]
fn test_reconnect_strategy_default() {
    let strategy = ReconnectStrategy::default();
    assert!(matches!(strategy, ReconnectStrategy::Direct));
}

#[test]
fn test_port_info_serialization() {
    let info = PortInfo {
        path: "/dev/cu.usbserial-1420".into(),
        vid: Some(0x1A86),
        pid: Some(0x7523),
        serial_number: Some("12345".into()),
        manufacturer: Some("QinHeng Electronics".into()),
        product: Some("CH340".into()),
    };
    let json = serde_json::to_string(&info).unwrap();
    let parsed: PortInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.path, "/dev/cu.usbserial-1420");
    assert_eq!(parsed.vid, Some(0x1A86));
}

#[test]
fn test_device_kind_roundtrip() {
    for kind in [DeviceKind::Serial, DeviceKind::Hid, DeviceKind::Both] {
        let json = serde_json::to_string(&kind).unwrap();
        let parsed: DeviceKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kind);
    }
}

#[test]
fn test_device_info_roundtrip() {
    let info = DeviceInfo {
        path: "/dev/cu.usbserial-1420".into(),
        serial_path: Some("/dev/cu.usbserial-1420".into()),
        hid_path: Some("IOService:/path/to/hid".into()),
        vid: Some(0x1A86),
        pid: Some(0x7523),
        serial_number: Some("12345".into()),
        manufacturer: Some("QinHeng Electronics".into()),
        product: Some("CH340".into()),
        kind: DeviceKind::Both,
        hid_usage: Some(HidUsageInfo {
            usage_page: 0x01,
            usage: 0x06,
            usage_name: Some("Keyboard".into()),
        }),
        ble_id: None,
        ble_services: None,
        bus_type: Some("USB".to_string()),
    };
    let json = serde_json::to_string(&info).unwrap();
    let parsed: DeviceInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.path, "/dev/cu.usbserial-1420");
    assert_eq!(parsed.kind, DeviceKind::Both);
    assert_eq!(parsed.vid, Some(0x1A86));
    assert!(parsed.hid_usage.is_some());
    let usage = parsed.hid_usage.unwrap();
    assert_eq!(usage.usage_page, 0x01);
    assert_eq!(usage.usage, 0x06);
    assert_eq!(usage.usage_name, Some("Keyboard".into()));
}

#[test]
fn test_device_info_minimal() {
    let info = DeviceInfo {
        path: "HID#Device123".into(),
        serial_path: None,
        hid_path: Some("HID#Device123".into()),
        vid: None,
        pid: None,
        serial_number: None,
        manufacturer: None,
        product: None,
        kind: DeviceKind::Hid,
        hid_usage: None,
        ble_id: None,
        ble_services: None,
        bus_type: None,
    };
    let json = serde_json::to_string(&info).unwrap();
    let parsed: DeviceInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.kind, DeviceKind::Hid);
    assert!(parsed.serial_path.is_none());
}
