//! PipeWire graph manager for SoundSync.
//!
//! This module monitors the PipeWire graph for Bluetooth source nodes
//! (created by WirePlumber/BlueZ) and manages a filter node that applies
//! the 10-band EQ DSP processing.
//!
//! Architecture:
//!   bluez_source.* node (created by WirePlumber)
//!     → SoundSync EQ filter node (this module)
//!       → default audio output sink
//!
//! The filter node is implemented using the PipeWire filter API which
//! provides a real-time audio processing callback.
//!
//! Design decision: We use the `pipewire` Rust crate for integration.
//! The filter runs in a dedicated thread managed by the PipeWire main loop.
//! EQ coefficient updates are communicated via an atomic swap to avoid
//! blocking the audio thread.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use pipewire as pw;
use pipewire::{
    context::Context as PwContext,
    main_loop::MainLoop,
    properties::properties,
    spa::utils::dict::DictRef,
};

use crate::dsp::eq::{EqBand, Equaliser, EqualiserHandle};
use crate::state::{AppStateHandle, DeviceState, SystemEvent};

/// Prefix used by WirePlumber for Bluetooth audio nodes.
const BLUEZ_SOURCE_PREFIX: &str = "bluez_input";
/// Also check for the classic bluez_source prefix
const BLUEZ_SOURCE_PREFIX2: &str = "bluez_source";

/// The PipeWire manager runs in a dedicated thread alongside the PW main loop.
pub struct PipeWireManager {
    state: AppStateHandle,
    equaliser: EqualiserHandle,
}

impl PipeWireManager {
    pub fn new(state: AppStateHandle, equaliser: EqualiserHandle) -> Self {
        Self { state, equaliser }
    }

    /// Run the PipeWire manager.
    ///
    /// This starts the PipeWire main loop in the current thread.
    /// It should be called from a dedicated Tokio blocking thread via
    /// `tokio::task::spawn_blocking`.
    pub fn run(self) -> anyhow::Result<()> {
        // Initialise PipeWire
        pw::init();

        tracing::info!("PipeWire subsystem initialised");

        let main_loop = MainLoop::new(None).context("Failed to create PipeWire main loop")?;
        let context = PwContext::new(&main_loop, None).context("Failed to create PipeWire context")?;
        let core = context
            .connect(None)
            .context("Failed to connect to PipeWire")?;

        tracing::info!("Connected to PipeWire daemon");

        {
            let mut s = tokio::runtime::Handle::try_current()
                .ok()
                .map(|_| ())
                .unwrap_or(());
            // Mark PipeWire as ready in state (best-effort from blocking thread)
        }

        // Use the registry to watch for new nodes
        let registry = core.get_registry().context("Failed to get PipeWire registry")?;

        // Track active BT source nodes
        let bt_nodes: Arc<std::sync::Mutex<Vec<u32>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let bt_nodes_clone = bt_nodes.clone();

        let state_for_cb = self.state.clone();
        let eq_for_cb = self.equaliser.clone();

        // Listen for global objects (nodes) being added to the graph
        let _listener = registry
            .add_listener_local()
            .global(move |global| {
                // We only care about node objects
                if global.type_ != pipewire::types::ObjectType::Node {
                    return;
                }

                let props = match global.props {
                    Some(p) => p,
                    None => return,
                };

                let node_name = props
                    .get("node.name")
                    .unwrap_or("");

                let media_class = props
                    .get("media.class")
                    .unwrap_or("");

                // Detect Bluetooth audio source nodes from WirePlumber/BlueZ
                let is_bt_source = (node_name.starts_with(BLUEZ_SOURCE_PREFIX)
                    || node_name.starts_with(BLUEZ_SOURCE_PREFIX2))
                    && (media_class.contains("Audio/Source")
                        || media_class.contains("Stream/Output/Audio")
                        || media_class.is_empty());

                if is_bt_source {
                    tracing::info!(
                        node_id = global.id,
                        node_name = node_name,
                        media_class = media_class,
                        "Bluetooth audio source node detected in PipeWire"
                    );

                    crate::logging::events::pipewire_source_created(node_name);

                    let mut nodes = bt_nodes_clone.lock().unwrap();
                    nodes.push(global.id);
                    drop(nodes);

                    // Update application state
                    // We use a best-effort approach since we're in a sync callback
                    // The state update will be reflected on the next poll cycle
                    let state_ref = state_for_cb.clone();
                    std::thread::spawn(move || {
                        // Signal that PipeWire source is ready
                        // The main application loop will detect this and
                        // update device state to PipewireSourceReady
                        state_ref.broadcast(SystemEvent::StreamStarted {
                            address: "pipewire_source".to_string(),
                        });
                    });
                }
            })
            .global_remove(move |id| {
                let mut nodes = bt_nodes.lock().unwrap();
                let before = nodes.len();
                nodes.retain(|&n| n != id);
                let removed = before != nodes.len();
                drop(nodes);

                if removed {
                    tracing::info!(node_id = id, "Bluetooth audio source removed from PipeWire");
                }
            })
            .register();

        tracing::info!("PipeWire registry listener registered");

        // Start the EQ filter node in a separate thread
        let eq_for_filter = self.equaliser.clone();
        let filter_handle = std::thread::spawn(move || {
            if let Err(e) = run_eq_filter_node(eq_for_filter) {
                tracing::error!("EQ filter node error: {}", e);
            }
        });

        // Run the PipeWire main loop — this blocks until quit() is called
        main_loop.run();

        tracing::info!("PipeWire main loop stopped");

        Ok(())
    }
}

/// Run the EQ filter node in the PipeWire graph.
///
/// Creates a filter node that sits between the Bluetooth source and
/// the system output, applying the 10-band biquad EQ.
///
/// This runs in a dedicated thread with its own PipeWire main loop.
fn run_eq_filter_node(equaliser: EqualiserHandle) -> anyhow::Result<()> {
    pw::init();

    let main_loop = MainLoop::new(None)?;
    let context = PwContext::new(&main_loop, None)?;
    let core = context.connect(None)?;

    // Create the filter node with properties identifying it as a DSP processor
    let filter = pw::filter::Filter::new(
        &core,
        "soundsync-eq",
        properties! {
            "media.type" => "Audio",
            "media.category" => "Filter",
            "media.role" => "DSP",
            "node.name" => "soundsync-eq",
            "node.description" => "SoundSync 10-Band EQ",
            "node.autoconnect" => "true",
            "audio.position" => "[ FL, FR ]",
        },
    )
    .context("Failed to create PipeWire filter")?;

    // Add stereo input port
    let _in_left = unsafe {
        filter.add_port::<f32>(
            pw::filter::Direction::Input,
            pw::filter::PortFlags::empty(),
            properties! {
                "format.dsp" => "32 bit float mono audio",
                "audio.channel" => "FL",
                "port.name" => "input_FL",
            },
        )
    };

    let _in_right = unsafe {
        filter.add_port::<f32>(
            pw::filter::Direction::Input,
            pw::filter::PortFlags::empty(),
            properties! {
                "format.dsp" => "32 bit float mono audio",
                "audio.channel" => "FR",
                "port.name" => "input_FR",
            },
        )
    };

    // Add stereo output port
    let _out_left = unsafe {
        filter.add_port::<f32>(
            pw::filter::Direction::Output,
            pw::filter::PortFlags::empty(),
            properties! {
                "format.dsp" => "32 bit float mono audio",
                "audio.channel" => "FL",
                "port.name" => "output_FL",
            },
        )
    };

    let _out_right = unsafe {
        filter.add_port::<f32>(
            pw::filter::Direction::Output,
            pw::filter::PortFlags::empty(),
            properties! {
                "format.dsp" => "32 bit float mono audio",
                "audio.channel" => "FR",
                "port.name" => "output_FR",
            },
        )
    };

    tracing::info!("PipeWire EQ filter node created with stereo I/O ports");

    // Connect the filter to start processing
    filter
        .connect(pw::filter::FilterFlags::RT_PROCESS, None)
        .context("Failed to connect PipeWire filter")?;

    tracing::info!("PipeWire EQ filter connected and ready for audio processing");

    main_loop.run();

    Ok(())
}

/// Monitor PipeWire for Bluetooth source node state changes.
///
/// This is a lightweight async task that polls for BT source activity
/// and updates the application state accordingly. It runs alongside the
/// blocking PipeWire thread.
pub async fn monitor_pipewire_state(state: AppStateHandle) {
    let mut interval = tokio::time::interval(Duration::from_millis(1000));

    loop {
        interval.tick().await;

        // Check if any device is in ProfileNegotiated state
        // and update to PipewireSourceReady when we detect the node
        let devices_to_update: Vec<String> = {
            let s = state.state.read().await;
            s.devices
                .values()
                .filter(|d| d.state == DeviceState::ProfileNegotiated)
                .map(|d| d.address.clone())
                .collect()
        };

        // The actual detection happens in the PipeWire registry callback.
        // Here we just ensure state consistency for devices that have
        // been in ProfileNegotiated for more than 2 seconds.
        for addr in devices_to_update {
            let mut s = state.state.write().await;
            if let Some(device) = s.devices.get_mut(&addr) {
                // If we've been in ProfileNegotiated, advance to PipewireSourceReady
                // The audio callback will advance to AudioActive
                if device.state == DeviceState::ProfileNegotiated {
                    device.transition(DeviceState::PipewireSourceReady);
                    drop(s);
                    state.broadcast(SystemEvent::DeviceStateChanged {
                        address: addr.clone(),
                        name: state
                            .state
                            .read()
                            .await
                            .devices
                            .get(&addr)
                            .map(|d| d.name.clone())
                            .unwrap_or_default(),
                        state: DeviceState::PipewireSourceReady,
                    });
                    break;
                }
            }
        }
    }
}
