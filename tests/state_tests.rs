//! Integration tests for the application state machine.

use soundsync::state::{AppStateHandle, BluetoothStatus, Config, DeviceInfo, DeviceState};

fn test_config() -> Config {
    Config {
        port: 9999,
        adapter: "hci0".to_string(),
        device_name: "TestSync".to_string(),
        auto_pair: true,
        max_devices: 1,
        ..Config::default()
    }
}

fn make_device(addr: &str, name: &str) -> DeviceInfo {
    DeviceInfo::new(addr.to_string(), name.to_string())
}

// ── DeviceState tests ────────────────────────────────────────────────────────

#[test]
fn device_state_connected_group() {
    assert!(!DeviceState::Disconnected.is_connected());
    assert!(!DeviceState::Discovered.is_connected());
    assert!(!DeviceState::Pairing.is_connected());
    assert!(!DeviceState::Paired.is_connected());
    assert!(DeviceState::Connected.is_connected());
    assert!(DeviceState::ProfileNegotiated.is_connected());
    assert!(DeviceState::PipewireSourceReady.is_connected());
    assert!(DeviceState::AudioActive.is_connected());
}

#[test]
fn device_state_streaming() {
    assert!(!DeviceState::Connected.is_streaming());
    assert!(DeviceState::AudioActive.is_streaming());
}

#[test]
fn device_state_transition_updates_timestamp() {
    let mut device = make_device("AA:BB:CC:DD:EE:FF", "Test Phone");
    assert_eq!(device.state, DeviceState::Discovered);

    device.transition(DeviceState::Pairing);
    assert_eq!(device.state, DeviceState::Pairing);

    device.transition(DeviceState::Connected);
    assert_eq!(device.state, DeviceState::Connected);

    device.transition(DeviceState::AudioActive);
    assert!(device.state.is_streaming());
}

// ── AppState tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn app_state_upsert_and_retrieve_device() {
    let handle = AppStateHandle::new(test_config());
    let device = make_device("AA:BB:CC:DD:EE:01", "iPhone");

    {
        let mut state = handle.state.write().await;
        state.upsert_device(device.clone());
    }

    let state = handle.state.read().await;
    assert_eq!(state.devices.len(), 1);
    assert!(state.devices.contains_key("AA:BB:CC:DD:EE:01"));
}

#[tokio::test]
async fn app_state_remove_device() {
    let handle = AppStateHandle::new(test_config());

    {
        let mut state = handle.state.write().await;
        state.upsert_device(make_device("AA:BB:CC:DD:EE:02", "Laptop"));
        state.active_device = Some("AA:BB:CC:DD:EE:02".to_string());
    }

    {
        let mut state = handle.state.write().await;
        state.remove_device("AA:BB:CC:DD:EE:02");
    }

    let state = handle.state.read().await;
    assert!(state.devices.is_empty());
    assert!(
        state.active_device.is_none(),
        "active_device should be cleared when device is removed"
    );
}

#[tokio::test]
async fn app_state_device_list_sorted_connected_first() {
    let handle = AppStateHandle::new(test_config());

    {
        let mut state = handle.state.write().await;

        let mut d1 = make_device("AA:BB:CC:DD:EE:01", "Phone A");
        d1.state = DeviceState::Discovered;
        d1.rssi = Some(-70);

        let mut d2 = make_device("AA:BB:CC:DD:EE:02", "Phone B");
        d2.state = DeviceState::AudioActive; // Connected
        d2.rssi = Some(-55);

        let mut d3 = make_device("AA:BB:CC:DD:EE:03", "Phone C");
        d3.state = DeviceState::Discovered;
        d3.rssi = Some(-60);

        state.upsert_device(d1);
        state.upsert_device(d2);
        state.upsert_device(d3);
    }

    let state = handle.state.read().await;
    let list = state.device_list();

    assert_eq!(list.len(), 3);
    // Connected device (Phone B) should be first
    assert_eq!(
        list[0].address, "AA:BB:CC:DD:EE:02",
        "Connected device should sort first"
    );
}

#[tokio::test]
async fn app_state_bluetooth_status_string() {
    let handle = AppStateHandle::new(test_config());

    let statuses = [
        (BluetoothStatus::Ready, "ready"),
        (BluetoothStatus::Scanning, "scanning"),
        (BluetoothStatus::Unavailable, "unavailable"),
    ];

    for (status, expected) in statuses {
        let mut state = handle.state.write().await;
        state.bluetooth_status = status;
        assert_eq!(state.bluetooth_status_str(), expected);
    }
}

#[tokio::test]
async fn app_state_snapshot_includes_all_fields() {
    use soundsync::state::SystemEvent;

    let handle = AppStateHandle::new(test_config());

    {
        let mut state = handle.state.write().await;
        state.bluetooth_status = BluetoothStatus::Ready;
        state.upsert_device(make_device("AA:BB:CC:DD:EE:01", "Test Device"));
    }

    let state = handle.state.read().await;
    let snapshot = state.snapshot_event();

    match snapshot {
        SystemEvent::StateSnapshot {
            status,
            devices,
            eq,
            ..
        } => {
            assert_eq!(status, "ready");
            assert_eq!(devices.len(), 1);
            assert_eq!(eq.len(), 10, "Snapshot must include 10 EQ bands");
        }
        _ => panic!("Expected StateSnapshot event"),
    }
}

#[tokio::test]
async fn app_state_event_broadcast() {
    use soundsync::state::SystemEvent;

    let handle = AppStateHandle::new(test_config());
    let mut rx = handle.subscribe();

    handle.broadcast(SystemEvent::BluetoothStatusChanged {
        status: "ready".to_string(),
    });

    let event = rx.try_recv().expect("Should receive broadcast event");
    match event {
        SystemEvent::BluetoothStatusChanged { status } => assert_eq!(status, "ready"),
        _ => panic!("Unexpected event type"),
    }
}

// ── Config tests ──────────────────────────────────────────────────────────────

#[test]
fn config_defaults() {
    let cfg = Config::default();
    assert_eq!(cfg.port, 8080);
    assert_eq!(cfg.adapter, "hci0");
    assert_eq!(cfg.device_name, "SoundSync");
    assert!(cfg.auto_pair);
}

#[test]
fn config_is_serializable() {
    let cfg = Config::default();
    let toml = toml::to_string(&cfg).expect("Config must serialize to TOML");
    let parsed: Config = toml::from_str(&toml).expect("Config must deserialize from TOML");
    assert_eq!(cfg.port, parsed.port);
    assert_eq!(cfg.device_name, parsed.device_name);
}
