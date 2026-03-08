//! SoundSync — Bluetooth A2DP Sink with DSP EQ and Web UI
//!
//! Entry point and service orchestration.
//!
//! Architecture:
//! - Bluetooth manager (zbus/BlueZ) — runs as a Tokio task
//! - PipeWire manager (pipewire-rs) — runs in a blocking thread
//! - Axum web server — runs as a Tokio task
//! - Shared AppState — accessed via Arc<RwLock<>>
//! - Event bus — broadcast channel for WebSocket updates

mod api;
mod bluetooth;
mod dsp;
mod logging;
mod pipewire;
mod state;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use dsp::{eq::Equaliser, presets::PresetManager};
use tokio::sync::Mutex;

use crate::api::build_router;
use crate::bluetooth::BluetoothManager;
use crate::pipewire::PipeWireManager;
use crate::state::{AppStateHandle, Config};

/// SoundSync — Bluetooth A2DP Sink with DSP equaliser and web UI.
#[derive(Parser, Debug)]
#[command(name = "soundsync", version, about)]
struct Args {
    /// Port to serve the web UI on
    #[arg(short, long, env = "SOUNDSYNC_PORT", default_value = "8080")]
    port: u16,

    /// Bluetooth adapter name (e.g. hci0)
    #[arg(short, long, env = "SOUNDSYNC_ADAPTER", default_value = "hci0")]
    adapter: String,

    /// Bluetooth device/speaker name
    #[arg(short, long, env = "SOUNDSYNC_NAME", default_value = "SoundSync")]
    name: String,

    /// Log format: "pretty" or "json"
    #[arg(long, env = "LOG_FORMAT", default_value = "pretty")]
    log_format: String,

    /// Disable automatic device pairing
    #[arg(long, default_value = "false")]
    no_auto_pair: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Set log format before initialising logging
    std::env::set_var("LOG_FORMAT", &args.log_format);
    logging::init();

    tracing::info!("╔══════════════════════════════════════╗");
    tracing::info!(
        "║     SoundSync v{}              ║",
        env!("CARGO_PKG_VERSION")
    );
    tracing::info!("║  Bluetooth A2DP Sink + DSP + Web UI  ║");
    tracing::info!("╚══════════════════════════════════════╝");

    // Load or create config
    let mut config = Config::load();

    // CLI args override config file
    if args.port != 8080 {
        config.port = args.port;
    }
    if args.adapter != "hci0" {
        config.adapter = args.adapter.clone();
    }
    if args.name != "SoundSync" {
        config.device_name = args.name.clone();
    }
    config.auto_pair = !args.no_auto_pair;
    config.save();

    let port = config.port;
    let adapter_name = config.adapter.clone();

    tracing::info!(
        port = port,
        adapter = %adapter_name,
        name = %config.device_name,
        "Starting SoundSync"
    );

    // Ensure a capture sink exists so BT audio has somewhere to route
    ensure_capture_sink();

    // Initialise shared state
    let state = AppStateHandle::new(config);

    // Initialise DSP equaliser at 48kHz (standard Bluetooth A2DP sample rate)
    let equaliser = Arc::new(Equaliser::new(48000.0));

    // Load EQ presets
    let presets = Arc::new(Mutex::new(PresetManager::new()));

    // Initialise Bluetooth manager
    let bt_manager = BluetoothManager::new(state.clone(), &adapter_name);
    let bt_cmd_tx = bt_manager.command_sender();

    // Start PipeWire state monitor (async)
    let pw_state = state.clone();
    tokio::spawn(async move {
        pipewire::manager::monitor_pipewire_state(pw_state).await;
    });

    // Start real-time spectrum analyzer (captures from default sink monitor)
    let spectrum_state = state.clone();
    tokio::spawn(async move {
        pipewire::spectrum::run_spectrum_analyzer(spectrum_state).await;
    });

    // Start AVRCP track-info monitor (polls MediaPlayer1 on connected devices)
    let avrcp_state = state.clone();
    tokio::spawn(async move {
        bluetooth::avrcp::run_avrcp_monitor(avrcp_state).await;
    });

    // Start PipeWire manager in a dedicated blocking thread.
    // PipeWire's main loop is synchronous and must run on a non-async thread.
    let eq_for_pw = equaliser.clone();
    let pw_state = state.clone();
    tokio::task::spawn_blocking(move || {
        let manager = PipeWireManager::new(pw_state, eq_for_pw);
        if let Err(e) = manager.run() {
            tracing::error!("PipeWire manager error: {}", e);
        }
    });

    // Build the web router
    let router = build_router(state.clone(), bt_cmd_tx, equaliser.clone(), presets.clone());

    // Start Bluetooth manager in an async task
    tokio::spawn(async move {
        if let Err(e) = bt_manager.run().await {
            tracing::error!("Bluetooth manager error: {}", e);
        }
    });

    // Update state EQ bands from the equaliser
    {
        let mut s = state.state.write().await;
        s.eq_bands = equaliser.get_bands();
    }

    // Bind and serve the web UI + API
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(
        addr = %addr,
        "Web UI available at http://localhost:{}",
        port
    );
    tracing::info!("Press Ctrl+C to stop");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("Failed to bind to port {}", port))?;

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("Web server error")?;

    tracing::info!("SoundSync stopped");
    Ok(())
}

/// Create a PipeWire/PulseAudio null sink that acts as the audio capture bus.
///
/// On headless servers (no sound card) WirePlumber has no output sink to route
/// Bluetooth A2DP audio to, so nothing ever reaches the default sink monitor
/// that `parec` and the browser stream capture from.  Creating this null sink
/// and setting it as the system default gives WirePlumber a routing target;
/// `parec --device=@DEFAULT_MONITOR@` then captures whatever BT audio arrives.
///
/// The call is best-effort — if `pactl` is absent the app continues without it.
fn ensure_capture_sink() {
    let load = std::process::Command::new("pactl")
        .args([
            "load-module",
            "module-null-sink",
            "media.class=Audio/Sink",
            "sink_name=soundsync-capture",
            "sink_properties=device.description=SoundSync-Capture",
        ])
        .output();

    match load {
        Ok(out) if out.status.success() => {
            tracing::info!("Created null sink 'soundsync-capture' for audio capture");
            // Make it the default so WirePlumber routes BT audio here
            let _ = std::process::Command::new("pactl")
                .args(["set-default-sink", "soundsync-capture"])
                .status();
        }
        Ok(_) => {
            // Module may already be loaded from a previous run — that's fine.
            // Still try to set it as the default in case it exists.
            let _ = std::process::Command::new("pactl")
                .args(["set-default-sink", "soundsync-capture"])
                .status();
            tracing::debug!("pactl load-module returned non-zero (sink may already exist)");
        }
        Err(e) => {
            tracing::warn!(
                "pactl not available, cannot create capture sink: {}. \
                 Audio capture will use whatever @DEFAULT_MONITOR@ resolves to.",
                e
            );
        }
    }
}

/// Wait for Ctrl+C or SIGTERM to initiate graceful shutdown.
async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received Ctrl+C, shutting down...");
        }
        _ = terminate => {
            tracing::info!("Received SIGTERM, shutting down...");
        }
    }
}
