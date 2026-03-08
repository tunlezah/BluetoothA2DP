//! PipeWire graph manager for SoundSync.
#![allow(dead_code)]
//!
//! # Architecture
//!
//! Rather than using the unstable `pw::filter::Filter` Rust bindings (whose
//! API changes between pipewire-rs point releases), this module takes a
//! two-layer approach:
//!
//! 1. **Registry monitor** — uses the stable `pipewire` Rust crate to watch
//!    the PipeWire graph for Bluetooth source nodes created by WirePlumber.
//!
//! 2. **Filter-chain subprocess** — spawns `pipewire-filter-chain` with a
//!    dynamically-generated config file that implements the 10-band biquad EQ.
//!    When EQ settings change, a new config is written and the subprocess is
//!    restarted (causing a brief, acceptable dropout).
//!
//! # Audio graph
//!
//! ```text
//! bluez_input.* (WirePlumber)
//!     │
//!     ▼
//! effect_input.soundsync-eq  ← Audio/Sink  (pipewire-filter-chain)
//!     │  [bq_peaking × 10 bands]
//!     ▼
//! effect_output.soundsync-eq ← Audio/Source
//!     │
//!     ▼
//! Default system output sink
//! ```

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use pipewire::spa::utils::dict::DictRef;
use pipewire::types::ObjectType;

use crate::dsp::eq::{EqBand, EqualiserHandle};
use crate::state::{AppStateHandle, DeviceState, SystemEvent};

/// Node name prefixes used by WirePlumber for Bluetooth A2DP input nodes.
const BLUEZ_NODE_PREFIXES: &[&str] = &["bluez_input.", "bluez_source.", "api.bluez5."];

/// The PipeWire manager.
///
/// Runs in a dedicated blocking thread (via `tokio::task::spawn_blocking`).
pub struct PipeWireManager {
    pub state: AppStateHandle,
    pub equaliser: EqualiserHandle,
}

impl PipeWireManager {
    pub fn new(state: AppStateHandle, equaliser: EqualiserHandle) -> Self {
        Self { state, equaliser }
    }

    /// Run the PipeWire manager.
    ///
    /// Starts the PipeWire main loop for registry monitoring and launches the
    /// `pipewire-filter-chain` subprocess for DSP. Blocks until quit.
    pub fn run(self) -> anyhow::Result<()> {
        pipewire::init();
        tracing::info!("PipeWire subsystem initialised");

        // Write initial filter-chain config and start subprocess
        let config_path = filter_chain_config_path();
        if let Err(e) = write_filter_chain_config(&self.equaliser.get_bands(), &config_path) {
            tracing::warn!("Could not write filter-chain config: {}", e);
        }

        let filter_process: Arc<Mutex<Option<Child>>> =
            Arc::new(Mutex::new(start_filter_chain(&config_path)));

        // PipeWire main loop
        let main_loop = pipewire::main_loop::MainLoop::new(None)
            .context("Failed to create PipeWire main loop")?;
        let context = pipewire::context::Context::new(&main_loop)
            .context("Failed to create PipeWire context")?;
        let core = context
            .connect(None)
            .context("Failed to connect to PipeWire daemon")?;
        let registry = core
            .get_registry()
            .context("Failed to get PipeWire registry")?;

        tracing::info!("Connected to PipeWire daemon");

        let state_cb = self.state.clone();

        let _listener = registry
            .add_listener_local()
            .global(move |global| {
                on_global_object(&state_cb, global);
            })
            .global_remove(|id| {
                tracing::debug!(node_id = id, "PipeWire node removed from graph");
            })
            .register();

        tracing::info!("PipeWire registry listener active");

        main_loop.run();

        // Cleanup on shutdown
        if let Ok(mut guard) = filter_process.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
                tracing::info!("pipewire-filter-chain subprocess terminated");
            }
        }

        Ok(())
    }
}

/// Called when a new node appears in the PipeWire graph.
fn on_global_object(state: &AppStateHandle, global: &pipewire::registry::GlobalObject<&DictRef>) {
    if global.type_ != ObjectType::Node {
        return;
    }

    let props = match &global.props {
        Some(p) => p,
        None => return,
    };

    let node_name = props.get("node.name").unwrap_or("");
    let media_class = props.get("media.class").unwrap_or("");

    let is_bt_node = BLUEZ_NODE_PREFIXES
        .iter()
        .any(|pfx| node_name.starts_with(pfx));

    if !is_bt_node {
        return;
    }

    tracing::info!(
        node_id = global.id,
        node_name,
        media_class,
        "Bluetooth audio node detected in PipeWire graph"
    );

    crate::logging::events::pipewire_source_created(node_name);

    // Signal the async layer via the broadcast channel
    state.broadcast(SystemEvent::StreamStarted {
        address: "pipewire_detected".to_string(),
    });
}

// ── Filter-chain config ───────────────────────────────────────────────────────

/// Path for the dynamically-generated filter-chain config.
fn filter_chain_config_path() -> PathBuf {
    let base = dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("soundsync").join("filter-chain.conf")
}

/// Write the filter-chain config for the given EQ bands to disk.
pub fn write_filter_chain_config(bands: &[EqBand], path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create config dir: {}", parent.display()))?;
    }
    let config = generate_filter_chain_config(bands);
    std::fs::write(path, config)
        .with_context(|| format!("Cannot write filter-chain config: {}", path.display()))?;
    tracing::debug!(path = %path.display(), "Filter-chain config written");
    Ok(())
}

/// Generate the PipeWire filter-chain config content.
///
/// Produces 10 `bq_peaking` builtin DSP nodes chained in series with the
/// gain, frequency and Q from the current EQ band settings.
fn generate_filter_chain_config(bands: &[EqBand]) -> String {
    let band_names = [
        "eq_60hz", "eq_120hz", "eq_250hz", "eq_500hz", "eq_1khz", "eq_2khz", "eq_4khz", "eq_8khz",
        "eq_12khz", "eq_16khz",
    ];

    let nodes: String = bands
        .iter()
        .enumerate()
        .map(|(i, band)| {
            format!(
                "          {{ type = builtin  label = bq_peaking  name = {name}\n\
                 \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20  control = {{ \"Freq\" = {freq}  \"Q\" = 1.41  \"Gain\" = {gain} }} }}\n",
                name = band_names[i],
                freq = band.freq,
                gain = band.gain_db,
            )
        })
        .collect();

    let links: String = (0..bands.len().saturating_sub(1))
        .map(|i| {
            format!(
                "          {{ output = \"{}:Out\"  input = \"{}:In\" }}\n",
                band_names[i],
                band_names[i + 1],
            )
        })
        .collect();

    let first = band_names[0];
    let last = band_names[bands.len() - 1];

    format!(
        "# SoundSync 10-Band EQ — generated by soundsync\n\
         context.modules = [\n\
         \x20 {{ name = libpipewire-module-filter-chain\n\
         \x20\x20\x20 args = {{\n\
         \x20\x20\x20\x20\x20 node.description = \"SoundSync 10-Band EQ\"\n\
         \x20\x20\x20\x20\x20 media.name       = \"SoundSync EQ\"\n\
         \n\
         \x20\x20\x20\x20\x20 filter.graph = {{\n\
         \x20\x20\x20\x20\x20\x20\x20 nodes = [\n\
         {nodes}\
         \x20\x20\x20\x20\x20\x20\x20 ]\n\
         \n\
         \x20\x20\x20\x20\x20\x20\x20 links = [\n\
         {links}\
         \x20\x20\x20\x20\x20\x20\x20 ]\n\
         \n\
         \x20\x20\x20\x20\x20\x20\x20 inputs  = [ \"{first}:In\" ]\n\
         \x20\x20\x20\x20\x20\x20\x20 outputs = [ \"{last}:Out\" ]\n\
         \x20\x20\x20\x20\x20 }}\n\
         \n\
         \x20\x20\x20\x20\x20 capture.props = {{\n\
         \x20\x20\x20\x20\x20\x20\x20 node.name   = \"effect_input.soundsync-eq\"\n\
         \x20\x20\x20\x20\x20\x20\x20 media.class = \"Audio/Sink\"\n\
         \x20\x20\x20\x20\x20\x20\x20 audio.channels = 2\n\
         \x20\x20\x20\x20\x20\x20\x20 audio.position = [ FL FR ]\n\
         \x20\x20\x20\x20\x20 }}\n\
         \n\
         \x20\x20\x20\x20\x20 playback.props = {{\n\
         \x20\x20\x20\x20\x20\x20\x20 node.name   = \"effect_output.soundsync-eq\"\n\
         \x20\x20\x20\x20\x20\x20\x20 media.class = \"Stream/Output/Audio\"\n\
         \x20\x20\x20\x20\x20\x20\x20 audio.channels = 2\n\
         \x20\x20\x20\x20\x20\x20\x20 audio.position = [ FL FR ]\n\
         \x20\x20\x20\x20\x20 }}\n\
         \x20\x20\x20 }}\n\
         \x20 }}\n\
         ]\n",
        nodes = nodes,
        links = links,
        first = first,
        last = last,
    )
}

/// Start the `pipewire-filter-chain` subprocess.
///
/// Returns `None` if the binary is not found — the application continues
/// without EQ DSP in that case, logging a clear warning.
fn start_filter_chain(config_path: &Path) -> Option<Child> {
    match Command::new("pipewire-filter-chain")
        .arg("--config")
        .arg(config_path)
        .spawn()
    {
        Ok(child) => {
            tracing::info!(
                pid = child.id(),
                config = %config_path.display(),
                "pipewire-filter-chain DSP subprocess started"
            );
            Some(child)
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "pipewire-filter-chain not available — EQ DSP will be inactive. \
                 Install: sudo apt-get install pipewire-audio"
            );
            None
        }
    }
}

/// Reload the filter-chain subprocess with updated EQ settings.
///
/// Writes a new config, terminates the old process, and starts a fresh one.
/// Causes a brief audio dropout (< 200ms) which is acceptable for EQ changes.
pub fn reload_filter_chain(bands: &[EqBand], filter_process: &Arc<Mutex<Option<Child>>>) {
    let config_path = filter_chain_config_path();

    if let Err(e) = write_filter_chain_config(bands, &config_path) {
        tracing::error!("Failed to write filter-chain config on EQ reload: {}", e);
        return;
    }

    if let Ok(mut guard) = filter_process.lock() {
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
            tracing::debug!("Stopped old pipewire-filter-chain subprocess");
        }
        *guard = start_filter_chain(&config_path);
    }
}

// ── Async state monitor ───────────────────────────────────────────────────────

/// Async task: listens for PipeWire source events and advances device state.
pub async fn monitor_pipewire_state(state: AppStateHandle) {
    let mut rx = state.subscribe();
    let mut interval = tokio::time::interval(Duration::from_secs(2));

    loop {
        tokio::select! {
            Ok(event) = rx.recv() => {
                if let SystemEvent::StreamStarted { .. } = event {
                    // PipeWire detected a BT node — mark source as ready then
                    // immediately advance to AudioActive so the spectrum
                    // analyser and the browser UI both pick up the stream.
                    advance_devices_to_source_ready(&state).await;
                    advance_devices_to_audio_active(&state).await;
                }
            }
            _ = interval.tick() => {
                // Periodic sweep: advance any device that has reached
                // ProfileNegotiated or PipewireSourceReady all the way to
                // AudioActive and fire StreamStarted so the spectrum analyser
                // and browser UI update even if PipeWire node detection fired
                // before the device was in the device list.
                advance_devices_to_source_ready(&state).await;
                advance_devices_to_audio_active(&state).await;
            }
        }
    }
}

/// Advance devices from ProfileNegotiated → PipewireSourceReady.
async fn advance_devices_to_source_ready(state: &AppStateHandle) {
    let candidates: Vec<(String, String)> = {
        let s = state.state.read().await;
        s.devices
            .values()
            .filter(|d| d.state == DeviceState::ProfileNegotiated)
            .map(|d| (d.address.clone(), d.name.clone()))
            .collect()
    };

    for (addr, name) in candidates {
        let mut s = state.state.write().await;
        if let Some(dev) = s.devices.get_mut(&addr) {
            if dev.state == DeviceState::ProfileNegotiated {
                dev.transition(DeviceState::PipewireSourceReady);
                drop(s);
                state.broadcast(SystemEvent::DeviceStateChanged {
                    address: addr,
                    name,
                    state: DeviceState::PipewireSourceReady,
                });
                return;
            }
        }
    }
}

/// Advance devices from PipewireSourceReady (or ProfileNegotiated) → AudioActive
/// and broadcast StreamStarted so the spectrum analyser and browser UI activate.
async fn advance_devices_to_audio_active(state: &AppStateHandle) {
    let candidates: Vec<(String, String)> = {
        let s = state.state.read().await;
        s.devices
            .values()
            .filter(|d| {
                d.state == DeviceState::PipewireSourceReady
                    || d.state == DeviceState::ProfileNegotiated
            })
            .map(|d| (d.address.clone(), d.name.clone()))
            .collect()
    };

    for (addr, name) in candidates {
        let mut s = state.state.write().await;
        if let Some(dev) = s.devices.get_mut(&addr) {
            if dev.state == DeviceState::PipewireSourceReady
                || dev.state == DeviceState::ProfileNegotiated
            {
                dev.transition(DeviceState::AudioActive);
                drop(s);
                state.broadcast(SystemEvent::DeviceStateChanged {
                    address: addr.clone(),
                    name,
                    state: DeviceState::AudioActive,
                });
                state.broadcast(SystemEvent::StreamStarted { address: addr });
                return;
            }
        }
    }
}
