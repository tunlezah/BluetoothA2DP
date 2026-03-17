//! Application state management for SoundSync.
#![allow(dead_code)]
//!
//! Provides a shared, thread-safe state container and an event bus
//! for broadcasting state changes to the WebSocket layer.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};

use crate::dsp::eq::EqBand;

/// Track metadata received via Bluetooth AVRCP (MediaPlayer1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackInfo {
    /// Track title
    pub title: Option<String>,
    /// Artist name
    pub artist: Option<String>,
    /// Album name
    pub album: Option<String>,
    /// Duration in milliseconds
    pub duration_ms: Option<u32>,
}

/// Playback status from AVRCP MediaPlayer1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
    #[default]
    Unknown,
}

/// Bluetooth connection state machine states.
/// Follows the state machine defined in bluetooth_state_machine.md.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    Disconnected,
    Discovered,
    Pairing,
    Paired,
    Connected,
    ProfileNegotiated,
    PipewireSourceReady,
    AudioActive,
}

impl DeviceState {
    /// Human-readable description of the state.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Discovered => "Discovered",
            Self::Pairing => "Pairing",
            Self::Paired => "Paired",
            Self::Connected => "Connected",
            Self::ProfileNegotiated => "Profile Negotiated",
            Self::PipewireSourceReady => "Audio Ready",
            Self::AudioActive => "Streaming",
        }
    }

    /// Whether this state represents an active audio stream.
    pub fn is_streaming(&self) -> bool {
        matches!(self, Self::AudioActive)
    }

    /// Whether this state represents a connected device.
    pub fn is_connected(&self) -> bool {
        matches!(
            self,
            Self::Connected
                | Self::ProfileNegotiated
                | Self::PipewireSourceReady
                | Self::AudioActive
        )
    }
}

impl std::fmt::Display for DeviceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.description())
    }
}

/// Information about a known Bluetooth device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Bluetooth MAC address (e.g. "AA:BB:CC:DD:EE:FF")
    pub address: String,
    /// Human-readable device name
    pub name: String,
    /// Current connection state
    pub state: DeviceState,
    /// RSSI signal strength in dBm (if available during scan)
    pub rssi: Option<i16>,
    /// Whether this device is trusted/paired
    pub trusted: bool,
    /// Whether device supports A2DP profile
    pub has_a2dp: bool,
    /// Timestamp of last state change
    pub last_seen: DateTime<Utc>,
    /// PipeWire node name when audio is active
    pub pipewire_node: Option<String>,
}

impl DeviceInfo {
    pub fn new(address: String, name: String) -> Self {
        Self {
            address,
            name,
            state: DeviceState::Discovered,
            rssi: None,
            trusted: false,
            has_a2dp: false,
            last_seen: Utc::now(),
            pipewire_node: None,
        }
    }

    pub fn transition(&mut self, new_state: DeviceState) {
        tracing::debug!(
            addr = %self.address,
            from = %self.state,
            to = %new_state,
            "Device state transition"
        );
        self.state = new_state;
        self.last_seen = Utc::now();
    }
}

/// Overall Bluetooth adapter status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BluetoothStatus {
    /// Adapter present and powered
    Ready,
    /// Scanning for devices
    Scanning,
    /// Adapter powered off or not present
    Unavailable,
    /// Adapter in error/recovery state
    Error(String),
}

impl BluetoothStatus {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Ready | Self::Scanning)
    }
}

/// Events broadcast to WebSocket clients when state changes occur.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum SystemEvent {
    /// Bluetooth adapter status changed
    BluetoothStatusChanged { status: String },
    /// A device's state changed
    DeviceStateChanged {
        address: String,
        name: String,
        state: DeviceState,
    },
    /// Device list updated (new device found or removed)
    DeviceListUpdated,
    /// Audio stream started
    StreamStarted { address: String },
    /// Audio stream stopped
    StreamStopped { address: String },
    /// EQ settings changed
    EqChanged,
    /// Track metadata changed (AVRCP)
    TrackChanged { track: Option<TrackInfo> },
    /// Playback status changed (AVRCP)
    PlaybackStatusChanged { status: PlaybackStatus },
    /// Real-time spectrum analysis data (64 log-spaced bands, 0.0–1.0)
    SpectrumData { bands: Vec<f32> },
    /// Line-in audio source activated
    LineInActivated,
    /// Line-in audio source deactivated
    LineInDeactivated,
    /// System error occurred
    Error { message: String },
    /// Service is about to shut down — clients should reconnect after a delay.
    ServiceStopping,
    /// Full state snapshot (sent on WebSocket connect)
    StateSnapshot {
        status: String,
        devices: Vec<DeviceInfo>,
        eq: Vec<EqBand>,
        active_device: Option<String>,
        track_info: Option<TrackInfo>,
        playback_status: PlaybackStatus,
        line_in_active: bool,
        line_in_available: bool,
    },
}

/// Configuration loaded from config.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub port: u16,
    pub adapter: String,
    pub device_name: String,
    pub auto_pair: bool,
    pub max_devices: u32,
    /// AAC encoder to use: "libfdk_aac" (higher quality) or "aac" (built-in fallback).
    /// Detected at install time by the installer.
    #[serde(default = "Config::default_aac_encoder")]
    pub aac_encoder: String,
    /// Default browser stream quality: "mp3" | "aac" | "wav"
    #[serde(default = "Config::default_stream_quality")]
    pub stream_quality: String,
}

impl Config {
    fn default_aac_encoder() -> String {
        "aac".to_string()
    }
    fn default_stream_quality() -> String {
        "mp3".to_string()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 8080,
            adapter: "hci0".to_string(),
            device_name: "SoundSync".to_string(),
            auto_pair: true,
            max_devices: 1,
            aac_encoder: "aac".to_string(),
            stream_quality: "mp3".to_string(),
        }
    }
}

impl Config {
    /// Load config from `~/.config/soundsync/config.toml` or return defaults.
    pub fn load() -> Self {
        let config_path = dirs::config_dir()
            .map(|d| d.join("soundsync").join("config.toml"))
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp/soundsync/config.toml"));

        if config_path.exists() {
            match std::fs::read_to_string(&config_path) {
                Ok(content) => match toml::from_str(&content) {
                    Ok(config) => return config,
                    Err(e) => tracing::warn!("Failed to parse config.toml: {}", e),
                },
                Err(e) => tracing::warn!("Failed to read config.toml: {}", e),
            }
        }

        Self::default()
    }

    /// Save current config to disk.
    pub fn save(&self) {
        let config_dir = dirs::config_dir()
            .map(|d| d.join("soundsync"))
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp/soundsync"));

        if let Err(e) = std::fs::create_dir_all(&config_dir) {
            tracing::warn!("Failed to create config directory: {}", e);
            return;
        }

        match toml::to_string_pretty(self) {
            Ok(content) => {
                if let Err(e) = std::fs::write(config_dir.join("config.toml"), content) {
                    tracing::warn!("Failed to write config.toml: {}", e);
                }
            }
            Err(e) => tracing::warn!("Failed to serialise config: {}", e),
        }
    }
}

/// The central shared application state.
///
/// All state mutations go through this struct to ensure consistency.
/// WebSocket subscribers receive events via the broadcast channel.
pub struct AppState {
    /// Bluetooth adapter status
    pub bluetooth_status: BluetoothStatus,
    /// Known devices indexed by MAC address
    pub devices: HashMap<String, DeviceInfo>,
    /// Currently active (streaming) device MAC address
    pub active_device: Option<String>,
    /// Current EQ bands
    pub eq_bands: Vec<EqBand>,
    /// Whether EQ is enabled
    pub eq_enabled: bool,
    /// Application configuration
    pub config: Config,
    /// Application start time
    pub started_at: Instant,
    /// PipeWire status
    pub pipewire_ready: bool,
    /// Current track info from AVRCP
    pub track_info: Option<TrackInfo>,
    /// Current playback status from AVRCP
    pub playback_status: PlaybackStatus,
    /// Whether line-in is the active audio source
    pub line_in_active: bool,
    /// Detected line-in source name (e.g. "alsa_input.pci-...")
    pub line_in_source: Option<String>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            bluetooth_status: BluetoothStatus::Unavailable,
            devices: HashMap::new(),
            active_device: None,
            eq_bands: EqBand::default_bands(),
            eq_enabled: true,
            config,
            started_at: Instant::now(),
            pipewire_ready: false,
            track_info: None,
            playback_status: PlaybackStatus::Unknown,
            line_in_active: false,
            line_in_source: None,
        }
    }

    /// Update or insert a device in the device map.
    pub fn upsert_device(&mut self, device: DeviceInfo) {
        self.devices.insert(device.address.clone(), device);
    }

    /// Remove a device from the device map.
    pub fn remove_device(&mut self, address: &str) {
        self.devices.remove(address);
        if self.active_device.as_deref() == Some(address) {
            self.active_device = None;
        }
    }

    /// Get all devices as a sorted vec (connected first, then by RSSI).
    pub fn device_list(&self) -> Vec<DeviceInfo> {
        let mut devices: Vec<DeviceInfo> = self.devices.values().cloned().collect();
        devices.sort_by(|a, b| {
            // Connected devices first
            let a_connected = a.state.is_connected() as u8;
            let b_connected = b.state.is_connected() as u8;
            b_connected.cmp(&a_connected).then_with(|| {
                // Then by RSSI (stronger signal first)
                let a_rssi = a.rssi.unwrap_or(i16::MIN);
                let b_rssi = b.rssi.unwrap_or(i16::MIN);
                b_rssi.cmp(&a_rssi)
            })
        });
        devices
    }

    /// Bluetooth status as a display string.
    pub fn bluetooth_status_str(&self) -> String {
        match &self.bluetooth_status {
            BluetoothStatus::Ready => "ready".to_string(),
            BluetoothStatus::Scanning => "scanning".to_string(),
            BluetoothStatus::Unavailable => "unavailable".to_string(),
            BluetoothStatus::Error(e) => format!("error: {}", e),
        }
    }

    /// Build a full state snapshot event for new WebSocket connections.
    pub fn snapshot_event(&self) -> SystemEvent {
        SystemEvent::StateSnapshot {
            status: self.bluetooth_status_str(),
            devices: self.device_list(),
            eq: self.eq_bands.clone(),
            active_device: self.active_device.clone(),
            track_info: self.track_info.clone(),
            playback_status: self.playback_status.clone(),
            line_in_active: self.line_in_active,
            line_in_available: self.line_in_source.is_some(),
        }
    }
}

/// Thread-safe handle to AppState with an event broadcast channel.
#[derive(Clone)]
pub struct AppStateHandle {
    pub state: Arc<RwLock<AppState>>,
    pub events: broadcast::Sender<SystemEvent>,
}

impl AppStateHandle {
    pub fn new(config: Config) -> Self {
        let (tx, _rx) = broadcast::channel(256);
        Self {
            state: Arc::new(RwLock::new(AppState::new(config))),
            events: tx,
        }
    }

    /// Broadcast an event to all WebSocket subscribers.
    pub fn broadcast(&self, event: SystemEvent) {
        // Ignore send errors — no subscribers is fine
        let _ = self.events.send(event);
    }

    /// Subscribe to system events.
    pub fn subscribe(&self) -> broadcast::Receiver<SystemEvent> {
        self.events.subscribe()
    }
}
