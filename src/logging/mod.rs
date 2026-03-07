//! Structured logging setup for SoundSync.
//!
//! Provides JSON-structured logging with event types matching the architecture
//! spec (BT_DEVICE_CONNECTED, PIPEWIRE_SOURCE_CREATED, etc.)

use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter,
};

/// Initialise the global tracing subscriber.
///
/// Log format is controlled by `LOG_FORMAT` environment variable:
/// - `json`  → structured JSON (default for production)
/// - `pretty` → human-readable coloured output (development)
pub fn init() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("soundsync=info,tower_http=debug"));

    let log_format = std::env::var("LOG_FORMAT").unwrap_or_else(|_| "pretty".to_string());

    if log_format == "json" {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().json().with_span_events(FmtSpan::CLOSE))
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(
                fmt::layer()
                    .with_target(true)
                    .with_thread_ids(false)
                    .with_span_events(FmtSpan::CLOSE),
            )
            .init();
    }
}

/// Log structured Bluetooth events matching the architecture specification.
pub mod events {
    /// Bluetooth device connected event.
    pub fn bt_device_connected(addr: &str, name: &str) {
        tracing::info!(
            event = "BT_DEVICE_CONNECTED",
            device_addr = addr,
            device_name = name,
            "Bluetooth device connected"
        );
    }

    /// Bluetooth device disconnected event.
    pub fn bt_device_disconnected(addr: &str, reason: &str) {
        tracing::info!(
            event = "BT_DEVICE_DISCONNECTED",
            device_addr = addr,
            reason = reason,
            "Bluetooth device disconnected"
        );
    }

    /// PipeWire Bluetooth source node created.
    pub fn pipewire_source_created(node_name: &str) {
        tracing::info!(
            event = "PIPEWIRE_SOURCE_CREATED",
            node_name = node_name,
            "PipeWire Bluetooth source node created"
        );
    }

    /// Audio stream started.
    pub fn stream_started(device_addr: &str) {
        tracing::info!(
            event = "STREAM_STARTED",
            device_addr = device_addr,
            "Audio stream started"
        );
    }

    /// Audio stream stopped.
    pub fn stream_stopped(device_addr: &str, reason: &str) {
        tracing::info!(
            event = "STREAM_STOPPED",
            device_addr = device_addr,
            reason = reason,
            "Audio stream stopped"
        );
    }

    /// Bluetooth adapter failure detected.
    pub fn adapter_failure(error: &str) {
        tracing::error!(
            event = "ADAPTER_FAILURE",
            error = error,
            "Bluetooth adapter failure detected"
        );
    }

    /// EQ preset changed.
    pub fn eq_preset_changed(preset_name: &str) {
        tracing::info!(
            event = "EQ_PRESET_CHANGED",
            preset_name = preset_name,
            "EQ preset changed"
        );
    }
}
