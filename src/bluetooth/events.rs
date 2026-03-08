//! Bluetooth event types emitted by the BluetoothManager.

/// Events produced by the Bluetooth subsystem.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum BluetoothEvent {
    /// Adapter became available and powered
    AdapterReady { name: String },
    /// Adapter lost or powered off
    AdapterLost,
    /// Adapter in error state
    AdapterError { reason: String },
    /// Scanning started
    ScanStarted,
    /// Scanning stopped
    ScanStopped,
    /// A new device was discovered during scan
    DeviceDiscovered {
        address: String,
        name: String,
        rssi: Option<i16>,
    },
    /// A device's properties changed (name, RSSI, UUIDs, etc.)
    DevicePropertiesChanged {
        address: String,
        connected: Option<bool>,
        paired: Option<bool>,
        trusted: Option<bool>,
        uuids: Option<Vec<String>>,
        rssi: Option<i16>,
    },
    /// A device connected to us
    DeviceConnected { address: String, name: String },
    /// A device disconnected from us
    DeviceDisconnected { address: String },
    /// A device was removed from BlueZ
    DeviceRemoved { address: String },
    /// A2DP profile was successfully negotiated for a device
    A2dpProfileReady { address: String },
    /// Pairing succeeded
    PairingSucceeded { address: String },
    /// Pairing failed
    PairingFailed { address: String, reason: String },
}
