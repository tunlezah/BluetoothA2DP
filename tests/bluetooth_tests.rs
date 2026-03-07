//! Integration tests for Bluetooth device helpers.

use soundsync::bluetooth::device::{address_from_path, has_a2dp, path_from_address};

// ── Device path helpers ───────────────────────────────────────────────────────

#[test]
fn address_from_standard_path() {
    let path = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF";
    assert_eq!(address_from_path(path), Some("AA:BB:CC:DD:EE:FF".to_string()));
}

#[test]
fn address_from_path_lowercase() {
    let path = "/org/bluez/hci0/dev_aa_bb_cc_dd_ee_ff";
    assert_eq!(address_from_path(path), Some("aa:bb:cc:dd:ee:ff".to_string()));
}

#[test]
fn address_from_non_device_path_returns_none() {
    for path in &["/org/bluez", "/org/bluez/hci0", "/org/bluez/hci0/media", ""] {
        assert!(address_from_path(path).is_none(), "Should return None for: {}", path);
    }
}

#[test]
fn path_from_address_builds_correctly() {
    let path = path_from_address("/org/bluez/hci0", "AA:BB:CC:DD:EE:FF");
    assert_eq!(path, "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF");
}

#[test]
fn path_round_trip() {
    let addr = "DE:AD:BE:EF:CA:FE";
    let path = path_from_address("/org/bluez/hci0", addr);
    assert_eq!(address_from_path(&path), Some(addr.to_string()));
}

// ── A2DP UUID detection ───────────────────────────────────────────────────────

#[test]
fn a2dp_sink_uuid_detected() {
    let uuids = vec![
        "0000110b-0000-1000-8000-00805f9b34fb".to_string(),
        "00001200-0000-1000-8000-00805f9b34fb".to_string(),
    ];
    assert!(has_a2dp(&uuids));
}

#[test]
fn a2dp_source_uuid_detected() {
    let uuids = vec!["0000110a-0000-1000-8000-00805f9b34fb".to_string()];
    assert!(has_a2dp(&uuids));
}

#[test]
fn no_a2dp_uuid_returns_false() {
    let uuids = vec![
        "00001200-0000-1000-8000-00805f9b34fb".to_string(),
        "0000110e-0000-1000-8000-00805f9b34fb".to_string(),
    ];
    assert!(!has_a2dp(&uuids));
}

#[test]
fn empty_uuid_list_returns_false() {
    assert!(!has_a2dp(&[]));
}

#[test]
fn a2dp_uuid_case_insensitive() {
    let uuids = vec!["0000110B-0000-1000-8000-00805F9B34FB".to_string()];
    assert!(has_a2dp(&uuids));
}
