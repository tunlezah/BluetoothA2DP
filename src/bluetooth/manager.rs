//! Bluetooth manager — the central controller for all Bluetooth operations.
//!
//! This module:
//! - Connects to BlueZ via D-Bus
//! - Monitors device events using the ObjectManager interface
//! - Manages the device state machine
//! - Handles failure recovery (adapter reset, reconnection)
//! - Exposes a command channel for the API layer

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Context;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use zbus::proxy;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::Connection;

use super::agent;
use super::device::has_a2dp;
use crate::state::{AppStateHandle, BluetoothStatus, DeviceInfo, DeviceState, SystemEvent};

/// Commands sent to the BluetoothManager from the API layer.
#[derive(Debug)]
pub enum BluetoothCommand {
    /// Start scanning for new devices
    StartScan,
    /// Stop scanning
    StopScan,
    /// Connect to a device by MAC address
    Connect { address: String },
    /// Disconnect from a device
    Disconnect { address: String },
    /// Remove a device from trusted list
    Remove { address: String },
    /// Set the adapter/speaker name
    SetName { name: String },
}

/// Proxy for the `org.freedesktop.DBus.ObjectManager` interface on BlueZ.
#[proxy(
    interface = "org.freedesktop.DBus.ObjectManager",
    default_service = "org.bluez",
    default_path = "/"
)]
trait ObjectManager {
    async fn get_managed_objects(
        &self,
    ) -> zbus::Result<HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>>;

    #[zbus(signal)]
    async fn interfaces_added(
        path: OwnedObjectPath,
        interfaces: HashMap<String, HashMap<String, OwnedValue>>,
    );

    #[zbus(signal)]
    async fn interfaces_removed(path: OwnedObjectPath, interfaces: Vec<String>);
}

/// Proxy for the `org.bluez.Adapter1` D-Bus interface.
#[proxy(
    interface = "org.bluez.Adapter1",
    default_service = "org.bluez",
    default_path = "/org/bluez/hci0"
)]
trait Adapter1 {
    async fn start_discovery(&self) -> zbus::Result<()>;
    async fn stop_discovery(&self) -> zbus::Result<()>;
    async fn remove_device(&self, device: &zbus::zvariant::ObjectPath<'_>) -> zbus::Result<()>;

    #[zbus(property)]
    async fn alias(&self) -> zbus::Result<String>;
    #[zbus(property)]
    async fn set_alias(&self, value: &str) -> zbus::Result<()>;
    #[zbus(property)]
    async fn powered(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    async fn set_powered(&self, value: bool) -> zbus::Result<()>;
    #[zbus(property)]
    async fn discoverable(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    async fn set_discoverable(&self, value: bool) -> zbus::Result<()>;
    #[zbus(property)]
    async fn pairable(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    async fn set_pairable(&self, value: bool) -> zbus::Result<()>;
    #[zbus(property)]
    async fn discovering(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    async fn address(&self) -> zbus::Result<String>;
}

/// Proxy for `org.bluez.Device1`.
#[proxy(interface = "org.bluez.Device1", default_service = "org.bluez")]
trait Device1 {
    async fn connect(&self) -> zbus::Result<()>;
    async fn disconnect(&self) -> zbus::Result<()>;
    async fn pair(&self) -> zbus::Result<()>;
    async fn cancel_pairing(&self) -> zbus::Result<()>;

    #[zbus(property)]
    async fn address(&self) -> zbus::Result<String>;
    #[zbus(property)]
    async fn name(&self) -> zbus::Result<String>;
    #[zbus(property)]
    async fn alias(&self) -> zbus::Result<String>;
    #[zbus(property)]
    async fn connected(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    async fn paired(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    async fn trusted(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    async fn set_trusted(&self, value: bool) -> zbus::Result<()>;
    #[zbus(property)]
    async fn uuids(&self) -> zbus::Result<Vec<String>>;
    #[zbus(property)]
    async fn rssi(&self) -> zbus::Result<i16>;
}

/// Proxy for `org.bluez.AgentManager1`.
#[proxy(
    interface = "org.bluez.AgentManager1",
    default_service = "org.bluez",
    default_path = "/org/bluez"
)]
trait AgentManager1 {
    async fn register_agent(
        &self,
        agent: &zbus::zvariant::ObjectPath<'_>,
        capability: &str,
    ) -> zbus::Result<()>;
    async fn unregister_agent(&self, agent: &zbus::zvariant::ObjectPath<'_>) -> zbus::Result<()>;
    async fn request_default_agent(
        &self,
        agent: &zbus::zvariant::ObjectPath<'_>,
    ) -> zbus::Result<()>;
}

/// The main Bluetooth manager.
pub struct BluetoothManager {
    state: AppStateHandle,
    cmd_tx: mpsc::Sender<BluetoothCommand>,
    cmd_rx: Option<mpsc::Receiver<BluetoothCommand>>,
    adapter_path: String,
}

impl BluetoothManager {
    /// Create a new BluetoothManager.
    pub fn new(state: AppStateHandle, adapter_name: &str) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        Self {
            state,
            cmd_tx,
            cmd_rx: Some(cmd_rx),
            adapter_path: format!("/org/bluez/{}", adapter_name),
        }
    }

    /// Get a command sender for use by the API layer.
    pub fn command_sender(&self) -> mpsc::Sender<BluetoothCommand> {
        self.cmd_tx.clone()
    }

    /// Run the Bluetooth manager event loop.
    ///
    /// This method takes ownership and runs indefinitely until the
    /// application shuts down. It must be spawned as a Tokio task.
    pub async fn run(mut self) -> anyhow::Result<()> {
        let cmd_rx = self.cmd_rx.take().expect("run() called twice");

        // Connect to the system D-Bus
        let connection = Connection::system()
            .await
            .context("Failed to connect to system D-Bus")?;

        tracing::info!("Connected to system D-Bus");

        // Register the auto-pairing agent
        let agent_path = agent::register_agent(&connection)
            .await
            .context("Failed to register BlueZ agent")?;

        // Register agent with BlueZ
        let agent_manager = AgentManager1Proxy::new(&connection).await?;
        let agent_obj_path = zbus::zvariant::ObjectPath::try_from(agent_path.as_str())?;
        match agent_manager
            .register_agent(&agent_obj_path, "NoInputNoOutput")
            .await
        {
            Ok(()) => {
                let _ = agent_manager.request_default_agent(&agent_obj_path).await;
                tracing::info!("BlueZ agent registered");
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to register agent (may already be registered): {}",
                    e
                );
            }
        }

        // Initialise the adapter
        self.init_adapter(&connection).await;

        // Start the event loop
        let connection_clone = connection.clone();
        let state_clone = self.state.clone();
        let adapter_path_clone = self.adapter_path.clone();

        // Spawn command handler
        let conn2 = connection.clone();
        let state2 = self.state.clone();
        let ap2 = self.adapter_path.clone();
        tokio::spawn(async move {
            Self::handle_commands(cmd_rx, conn2, state2, ap2).await;
        });

        // Run the main event monitoring loop
        Self::monitor_events(connection_clone, state_clone, adapter_path_clone).await;

        Ok(())
    }

    /// Initialise the Bluetooth adapter.
    async fn init_adapter(&self, connection: &Connection) {
        let mut retry_delay = Duration::from_secs(1);
        for attempt in 1..=10 {
            match self.setup_adapter(connection).await {
                Ok(()) => return,
                Err(e) => {
                    tracing::warn!(
                        attempt = attempt,
                        error = %e,
                        "Failed to initialise adapter, retrying..."
                    );
                    {
                        let mut state = self.state.state.write().await;
                        state.bluetooth_status = BluetoothStatus::Error(e.to_string());
                    }
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(Duration::from_secs(30));
                }
            }
        }
        tracing::error!("Failed to initialise Bluetooth adapter after 10 attempts");
    }

    /// Configure the adapter: power on, set name, enable discoverable.
    async fn setup_adapter(&self, connection: &Connection) -> anyhow::Result<()> {
        let adapter = Adapter1Proxy::builder(connection)
            .path(self.adapter_path.as_str())?
            .build()
            .await?;

        // Power on
        if !adapter.powered().await.unwrap_or(false) {
            adapter.set_powered(true).await?;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        // Set device name
        let device_name = {
            let state = self.state.state.read().await;
            state.config.device_name.clone()
        };
        let _ = adapter.set_alias(&device_name).await;
        let _ = adapter.set_discoverable(true).await;
        let _ = adapter.set_pairable(true).await;

        let addr = adapter.address().await.unwrap_or_default();
        tracing::info!(address = %addr, name = %device_name, "Bluetooth adapter ready");

        {
            let mut state = self.state.state.write().await;
            state.bluetooth_status = BluetoothStatus::Ready;
        }
        self.state.broadcast(SystemEvent::BluetoothStatusChanged {
            status: "ready".to_string(),
        });

        // Load existing paired devices from BlueZ
        self.load_existing_devices(connection).await;

        Ok(())
    }

    /// Load all known devices from BlueZ ObjectManager.
    async fn load_existing_devices(&self, connection: &Connection) {
        let obj_manager = match ObjectManagerProxy::new(connection).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Failed to get ObjectManager: {}", e);
                return;
            }
        };

        let objects = match obj_manager.get_managed_objects().await {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!("Failed to get managed objects: {}", e);
                return;
            }
        };

        for (path, interfaces) in &objects {
            if let Some(device_props) = interfaces.get("org.bluez.Device1") {
                if let Some(device_info) = extract_device_info(path.as_str(), device_props) {
                    let addr = device_info.address.clone();
                    let connected = device_info.state.is_connected();
                    {
                        let mut state = self.state.state.write().await;
                        state.upsert_device(device_info);
                    }
                    if connected {
                        tracing::info!(addr = %addr, "Found already-connected device");
                    }
                }
            }
        }

        self.state.broadcast(SystemEvent::DeviceListUpdated);
    }

    /// Main event monitoring loop — watches for D-Bus signals from BlueZ.
    async fn monitor_events(connection: Connection, state: AppStateHandle, adapter_path: String) {
        tracing::info!("Starting Bluetooth event monitor");

        // Use a periodic poll approach for device property changes.
        // BlueZ emits PropertiesChanged signals on individual device paths,
        // and InterfacesAdded/Removed for new devices.

        let obj_manager = match ObjectManagerProxy::new(&connection).await {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("Failed to get ObjectManager proxy: {}", e);
                return;
            }
        };

        // Subscribe to InterfacesAdded signal
        let mut interfaces_added_stream = match obj_manager.receive_interfaces_added().await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to subscribe to InterfacesAdded: {}", e);
                return;
            }
        };

        // Subscribe to InterfacesRemoved signal
        let mut interfaces_removed_stream = match obj_manager.receive_interfaces_removed().await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to subscribe to InterfacesRemoved: {}", e);
                return;
            }
        };

        // Poll for PropertiesChanged on all device paths
        let conn_for_props = connection.clone();
        let state_for_props = state.clone();
        tokio::spawn(async move {
            Self::poll_device_properties(conn_for_props, state_for_props).await;
        });

        loop {
            tokio::select! {
                Some(signal) = interfaces_added_stream.next() => {
                    if let Ok(args) = signal.args() {
                        let path = args.path.as_str().to_owned();
                        let interfaces = args.interfaces.clone();

                        if let Some(device_props) = interfaces.get("org.bluez.Device1") {
                            if let Some(device_info) = extract_device_info(&path, device_props) {
                                let addr = device_info.address.clone();
                                let name = device_info.name.clone();
                                tracing::info!(addr = %addr, name = %name, "New Bluetooth device discovered");

                                {
                                    let mut app_state = state.state.write().await;
                                    app_state.upsert_device(device_info.clone());
                                }

                                state.broadcast(SystemEvent::DeviceStateChanged {
                                    address: addr,
                                    name,
                                    state: DeviceState::Discovered,
                                });
                                state.broadcast(SystemEvent::DeviceListUpdated);
                            }
                        }
                    }
                }
                Some(signal) = interfaces_removed_stream.next() => {
                    if let Ok(args) = signal.args() {
                        let path = args.path.as_str().to_owned();
                        let removed_ifaces = args.interfaces.clone();
                        if removed_ifaces.contains(&"org.bluez.Device1".to_string()) {
                            if let Some(addr) = super::device::address_from_path(&path) {
                                tracing::info!(addr = %addr, "Bluetooth device removed");
                                {
                                    let mut app_state = state.state.write().await;
                                    app_state.remove_device(&addr);
                                }
                                state.broadcast(SystemEvent::DeviceListUpdated);
                            }
                        }
                        // Adapter removed — handle gracefully
                        if removed_ifaces.contains(&"org.bluez.Adapter1".to_string()) {
                            tracing::error!("Bluetooth adapter removed!");
                            {
                                let mut app_state = state.state.write().await;
                                app_state.bluetooth_status = BluetoothStatus::Unavailable;
                            }
                            state.broadcast(SystemEvent::BluetoothStatusChanged {
                                status: "unavailable".to_string(),
                            });
                        }
                    }
                }
                else => {
                    tracing::warn!("D-Bus event stream ended, reconnecting...");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    break;
                }
            }
        }
    }

    /// Poll device properties periodically to detect connection state changes.
    ///
    /// BlueZ emits PropertiesChanged signals but subscribing to all of them
    /// requires per-path subscriptions. Instead, we poll every 500ms which
    /// is sufficient for UI responsiveness while meeting the architecture's
    /// recovery requirements.
    async fn poll_device_properties(connection: Connection, state: AppStateHandle) {
        let mut interval = tokio::time::interval(Duration::from_millis(500));

        loop {
            interval.tick().await;

            let obj_manager = match ObjectManagerProxy::new(&connection).await {
                Ok(p) => p,
                Err(_) => {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            };

            let objects = match obj_manager.get_managed_objects().await {
                Ok(o) => o,
                Err(_) => continue,
            };

            let mut current_state = state.state.write().await;
            let mut any_changed = false;

            for (path, interfaces) in &objects {
                if let Some(device_props) = interfaces.get("org.bluez.Device1") {
                    if let Some(new_info) = extract_device_info(path.as_str(), device_props) {
                        let addr = new_info.address.clone();

                        if let Some(existing) = current_state.devices.get_mut(&addr) {
                            // Check for state changes
                            let was_connected = existing.state.is_connected();
                            let is_connected = new_info.state.is_connected();

                            if was_connected != is_connected {
                                any_changed = true;
                                if is_connected {
                                    crate::logging::events::bt_device_connected(
                                        &addr,
                                        &new_info.name,
                                    );
                                    existing.transition(DeviceState::Connected);

                                    // Check for A2DP
                                    if new_info.has_a2dp {
                                        existing.has_a2dp = true;
                                        existing.transition(DeviceState::ProfileNegotiated);
                                        current_state.active_device = Some(addr.clone());
                                    }
                                } else {
                                    crate::logging::events::bt_device_disconnected(
                                        &addr,
                                        "connection_lost",
                                    );
                                    existing.transition(DeviceState::Disconnected);
                                    if current_state.active_device.as_deref() == Some(&addr) {
                                        current_state.active_device = None;
                                    }
                                }
                            }

                            // Update RSSI
                            if new_info.rssi != existing.rssi {
                                existing.rssi = new_info.rssi;
                            }
                        } else {
                            // New device found
                            current_state.upsert_device(new_info);
                            any_changed = true;
                        }
                    }
                }
            }

            // Remove devices that are no longer in BlueZ
            let bluez_addrs: Vec<String> = objects
                .iter()
                .filter(|(_, ifaces)| ifaces.contains_key("org.bluez.Device1"))
                .filter_map(|(path, _)| super::device::address_from_path(path.as_str()))
                .collect();

            let to_remove: Vec<String> = current_state
                .devices
                .keys()
                .filter(|addr| !bluez_addrs.contains(addr))
                .cloned()
                .collect();

            for addr in &to_remove {
                current_state.remove_device(addr);
                any_changed = true;
            }

            drop(current_state);

            if any_changed {
                state.broadcast(SystemEvent::DeviceListUpdated);
            }
        }
    }

    /// Handle commands from the API layer.
    async fn handle_commands(
        mut rx: mpsc::Receiver<BluetoothCommand>,
        connection: Connection,
        state: AppStateHandle,
        adapter_path: String,
    ) {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                BluetoothCommand::StartScan => {
                    tracing::info!("Command: StartScan");
                    agent::set_pairing_allowed(true);

                    if let Ok(builder) =
                        Adapter1Proxy::builder(&connection).path(adapter_path.as_str())
                    {
                        match builder.build().await {
                            Ok(a) => {
                                if let Err(e) = a.start_discovery().await {
                                    tracing::warn!("Failed to start discovery: {}", e);
                                } else {
                                    let mut s = state.state.write().await;
                                    s.bluetooth_status = BluetoothStatus::Scanning;
                                    drop(s);
                                    state.broadcast(SystemEvent::BluetoothStatusChanged {
                                        status: "scanning".to_string(),
                                    });
                                }
                            }
                            Err(e) => tracing::warn!("Failed to get adapter: {}", e),
                        }
                    }

                    // Auto-stop scan after 30 seconds
                    let state_clone = state.clone();
                    let conn_clone = connection.clone();
                    let ap_clone = adapter_path.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(30)).await;
                        agent::set_pairing_allowed(false);
                        if let Ok(b) = Adapter1Proxy::builder(&conn_clone).path(ap_clone.as_str()) {
                            if let Ok(a) = b.build().await {
                                let discovering = a.discovering().await.unwrap_or(false);
                                if discovering {
                                    let _ = a.stop_discovery().await;
                                }
                            }
                        }
                        let mut s = state_clone.state.write().await;
                        if matches!(s.bluetooth_status, BluetoothStatus::Scanning) {
                            s.bluetooth_status = BluetoothStatus::Ready;
                            drop(s);
                            state_clone.broadcast(SystemEvent::BluetoothStatusChanged {
                                status: "ready".to_string(),
                            });
                        }
                    });
                }

                BluetoothCommand::StopScan => {
                    tracing::info!("Command: StopScan");
                    agent::set_pairing_allowed(false);

                    if let Ok(b) = Adapter1Proxy::builder(&connection).path(adapter_path.as_str()) {
                        if let Ok(a) = b.build().await {
                            let _ = a.stop_discovery().await;
                        }
                    }

                    let mut s = state.state.write().await;
                    if matches!(s.bluetooth_status, BluetoothStatus::Scanning) {
                        s.bluetooth_status = BluetoothStatus::Ready;
                        drop(s);
                        state.broadcast(SystemEvent::BluetoothStatusChanged {
                            status: "ready".to_string(),
                        });
                    }
                }

                BluetoothCommand::Connect { address } => {
                    tracing::info!(addr = %address, "Command: Connect");
                    let device_path = super::device::path_from_address(&adapter_path, &address);

                    // Update state to pairing
                    {
                        let mut s = state.state.write().await;
                        if let Some(device) = s.devices.get_mut(&address) {
                            device.transition(DeviceState::Pairing);
                        }
                    }
                    state.broadcast(SystemEvent::DeviceStateChanged {
                        address: address.clone(),
                        name: state
                            .state
                            .read()
                            .await
                            .devices
                            .get(&address)
                            .map(|d| d.name.clone())
                            .unwrap_or_default(),
                        state: DeviceState::Pairing,
                    });

                    let conn_clone = connection.clone();
                    let state_clone = state.clone();
                    let addr_clone = address.clone();
                    tokio::spawn(async move {
                        match Device1Proxy::builder(&conn_clone).path(device_path.as_str()) {
                            Ok(b) => match b.build().await {
                                Ok(device) => {
                                    // Trust the device so it auto-reconnects
                                    let _ = device.set_trusted(true).await;

                                    match device.connect().await {
                                        Ok(()) => {
                                            tracing::info!(addr = %addr_clone, "Device connected successfully");
                                        }
                                        Err(e) => {
                                            tracing::warn!(addr = %addr_clone, error = %e, "Failed to connect device");
                                            let mut s = state_clone.state.write().await;
                                            if let Some(dev) = s.devices.get_mut(&addr_clone) {
                                                dev.transition(DeviceState::Discovered);
                                            }
                                            drop(s);
                                            state_clone.broadcast(SystemEvent::Error {
                                                message: format!("Failed to connect: {}", e),
                                            });
                                        }
                                    }
                                }
                                Err(e) => tracing::warn!("Failed to build device proxy: {}", e),
                            },
                            Err(e) => {
                                tracing::warn!("Failed to create device proxy builder: {}", e)
                            }
                        }
                    });
                }

                BluetoothCommand::Disconnect { address } => {
                    tracing::info!(addr = %address, "Command: Disconnect");
                    let device_path = super::device::path_from_address(&adapter_path, &address);

                    let conn_clone = connection.clone();
                    let state_clone = state.clone();
                    let addr_clone = address.clone();
                    tokio::spawn(async move {
                        if let Ok(b) = Device1Proxy::builder(&conn_clone).path(device_path.as_str())
                        {
                            if let Ok(device) = b.build().await {
                                if let Err(e) = device.disconnect().await {
                                    tracing::warn!(addr = %addr_clone, error = %e, "Failed to disconnect");
                                }
                            }
                        }
                        let mut s = state_clone.state.write().await;
                        if let Some(dev) = s.devices.get_mut(&addr_clone) {
                            dev.transition(DeviceState::Disconnected);
                        }
                        if s.active_device.as_deref() == Some(&addr_clone) {
                            s.active_device = None;
                        }
                        drop(s);
                        state_clone.broadcast(SystemEvent::DeviceStateChanged {
                            address: addr_clone.clone(),
                            name: String::new(),
                            state: DeviceState::Disconnected,
                        });
                        crate::logging::events::bt_device_disconnected(
                            &addr_clone,
                            "user_requested",
                        );
                    });
                }

                BluetoothCommand::Remove { address } => {
                    tracing::info!(addr = %address, "Command: Remove");
                    let device_path = super::device::path_from_address(&adapter_path, &address);

                    if let Ok(b) = Adapter1Proxy::builder(&connection).path(adapter_path.as_str()) {
                        if let Ok(adapter) = b.build().await {
                            if let Ok(path) =
                                zbus::zvariant::ObjectPath::try_from(device_path.as_str())
                            {
                                let _ = adapter.remove_device(&path).await;
                            }
                        }
                    }

                    {
                        let mut s = state.state.write().await;
                        s.remove_device(&address);
                    }
                    state.broadcast(SystemEvent::DeviceListUpdated);
                }

                BluetoothCommand::SetName { name } => {
                    tracing::info!(name = %name, "Command: SetName");
                    if let Ok(b) = Adapter1Proxy::builder(&connection).path(adapter_path.as_str()) {
                        if let Ok(adapter) = b.build().await {
                            if let Err(e) = adapter.set_alias(&name).await {
                                tracing::warn!("Failed to set adapter name: {}", e);
                            } else {
                                let mut s = state.state.write().await;
                                s.config.device_name = name.clone();
                                s.config.save();
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Extract a DeviceInfo from a BlueZ Device1 properties map.
fn extract_device_info(_path: &str, props: &HashMap<String, OwnedValue>) -> Option<DeviceInfo> {
    let address = props
        .get("Address")
        .and_then(|v| <&str>::try_from(v).ok())
        .map(|s| s.to_string())?;

    let name = props
        .get("Alias")
        .and_then(|v| <&str>::try_from(v).ok())
        .or_else(|| props.get("Name").and_then(|v| <&str>::try_from(v).ok()))
        .unwrap_or(address.as_str())
        .to_string();

    let connected = props
        .get("Connected")
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(false);

    let paired = props
        .get("Paired")
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(false);

    let trusted = props
        .get("Trusted")
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(false);

    let uuids: Vec<String> = props
        .get("UUIDs")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| Vec::<String>::try_from(v).ok())
        .unwrap_or_default();

    let rssi = props.get("RSSI").and_then(|v| i16::try_from(v).ok());

    let has_a2dp = has_a2dp(&uuids);

    let state = if connected && has_a2dp {
        DeviceState::ProfileNegotiated
    } else if connected {
        DeviceState::Connected
    } else if paired || trusted {
        DeviceState::Paired
    } else {
        DeviceState::Discovered
    };

    let mut device = DeviceInfo::new(address.clone(), name);
    device.state = state;
    device.rssi = rssi;
    device.trusted = trusted;
    device.has_a2dp = has_a2dp;

    Some(device)
}
