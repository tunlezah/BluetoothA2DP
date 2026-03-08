//! BlueZ Device1 D-Bus interface wrapper.

use zbus::proxy;

/// A2DP Sink profile UUID.
pub const A2DP_SINK_UUID: &str = "0000110b-0000-1000-8000-00805f9b34fb";
/// A2DP Source profile UUID.
pub const A2DP_SOURCE_UUID: &str = "0000110a-0000-1000-8000-00805f9b34fb";

/// Proxy for the `org.bluez.Device1` D-Bus interface.
#[proxy(interface = "org.bluez.Device1", default_service = "org.bluez")]
pub trait Device1 {
    /// Initiate connection to the device.
    async fn connect(&self) -> zbus::Result<()>;
    /// Disconnect from the device.
    async fn disconnect(&self) -> zbus::Result<()>;
    /// Connect a specific profile (UUID).
    async fn connect_profile(&self, uuid: &str) -> zbus::Result<()>;
    /// Disconnect a specific profile (UUID).
    async fn disconnect_profile(&self, uuid: &str) -> zbus::Result<()>;
    /// Initiate pairing with the device.
    async fn pair(&self) -> zbus::Result<()>;
    /// Cancel an in-progress pairing.
    async fn cancel_pairing(&self) -> zbus::Result<()>;

    /// Bluetooth MAC address.
    #[zbus(property)]
    async fn address(&self) -> zbus::Result<String>;
    /// Human-readable device name.
    #[zbus(property)]
    async fn name(&self) -> zbus::Result<String>;
    /// Alias (user-set name or same as Name).
    #[zbus(property)]
    async fn alias(&self) -> zbus::Result<String>;
    /// Whether the device is currently connected.
    #[zbus(property)]
    async fn connected(&self) -> zbus::Result<bool>;
    /// Whether the device has been paired.
    #[zbus(property)]
    async fn paired(&self) -> zbus::Result<bool>;
    /// Whether the device is trusted.
    #[zbus(property)]
    async fn trusted(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    async fn set_trusted(&self, value: bool) -> zbus::Result<()>;
    /// List of service UUIDs supported by the device.
    #[zbus(property)]
    async fn uuids(&self) -> zbus::Result<Vec<String>>;
    /// RSSI signal strength in dBm.
    #[zbus(property)]
    async fn rssi(&self) -> zbus::Result<i16>;
    /// Bluetooth device class.
    #[zbus(property)]
    async fn class(&self) -> zbus::Result<u32>;
    /// Icon string for the device type.
    #[zbus(property)]
    async fn icon(&self) -> zbus::Result<String>;
}

/// Check if a device's UUID list includes A2DP support.
pub fn has_a2dp(uuids: &[String]) -> bool {
    uuids
        .iter()
        .any(|u| u.to_lowercase() == A2DP_SINK_UUID || u.to_lowercase() == A2DP_SOURCE_UUID)
}

/// Extract the device MAC address from a BlueZ object path.
///
/// BlueZ paths follow the pattern: `/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF`
pub fn address_from_path(path: &str) -> Option<String> {
    path.split('/').last().and_then(|last| {
        if last.starts_with("dev_") {
            Some(last[4..].replace('_', ":"))
        } else {
            None
        }
    })
}

/// Build a BlueZ device object path from an adapter path and MAC address.
pub fn path_from_address(adapter_path: &str, address: &str) -> String {
    let addr_encoded = address.replace(':', "_");
    format!("{}/dev_{}", adapter_path, addr_encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_from_path_extracts_mac() {
        let path = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF";
        assert_eq!(
            address_from_path(path),
            Some("AA:BB:CC:DD:EE:FF".to_string())
        );
    }

    #[test]
    fn path_from_address_builds_path() {
        let path = path_from_address("/org/bluez/hci0", "AA:BB:CC:DD:EE:FF");
        assert_eq!(path, "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF");
    }

    #[test]
    fn has_a2dp_detects_uuid() {
        let uuids = vec![
            "00001200-0000-1000-8000-00805f9b34fb".to_string(),
            "0000110b-0000-1000-8000-00805f9b34fb".to_string(), // A2DP sink
        ];
        assert!(has_a2dp(&uuids));
    }

    #[test]
    fn has_a2dp_returns_false_without_uuid() {
        let uuids = vec!["00001200-0000-1000-8000-00805f9b34fb".to_string()];
        assert!(!has_a2dp(&uuids));
    }
}
