//! BlueZ Adapter1 D-Bus interface wrapper.
//!
//! Manages the Bluetooth adapter: power state, discovery, and name.

use zbus::{proxy, Connection};

/// Proxy for the `org.bluez.Adapter1` D-Bus interface.
#[proxy(
    interface = "org.bluez.Adapter1",
    default_service = "org.bluez",
    default_path = "/org/bluez/hci0"
)]
trait Adapter1 {
    /// Start device discovery.
    fn start_discovery(&self) -> zbus::Result<()>;
    /// Stop device discovery.
    fn stop_discovery(&self) -> zbus::Result<()>;
    /// Remove a device from the adapter.
    fn remove_device(&self, device: &zbus::zvariant::ObjectPath<'_>) -> zbus::Result<()>;

    /// Adapter name (shown during Bluetooth discovery).
    #[zbus(property)]
    fn alias(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn set_alias(&self, value: &str) -> zbus::Result<()>;

    /// Whether the adapter is powered.
    #[zbus(property)]
    fn powered(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_powered(&self, value: bool) -> zbus::Result<()>;

    /// Whether the adapter is discoverable (visible to other devices).
    #[zbus(property)]
    fn discoverable(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_discoverable(&self, value: bool) -> zbus::Result<()>;

    /// Whether the adapter is pairable.
    #[zbus(property)]
    fn pairable(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_pairable(&self, value: bool) -> zbus::Result<()>;

    /// Whether the adapter is currently discovering.
    #[zbus(property)]
    fn discovering(&self) -> zbus::Result<bool>;

    /// Adapter hardware address.
    #[zbus(property)]
    fn address(&self) -> zbus::Result<String>;
}

/// Proxy for the `org.bluez.AgentManager1` D-Bus interface.
#[proxy(
    interface = "org.bluez.AgentManager1",
    default_service = "org.bluez",
    default_path = "/org/bluez"
)]
trait AgentManager1 {
    /// Register an agent at the given object path with the given capability.
    fn register_agent(
        &self,
        agent: &zbus::zvariant::ObjectPath<'_>,
        capability: &str,
    ) -> zbus::Result<()>;

    /// Unregister an agent.
    fn unregister_agent(&self, agent: &zbus::zvariant::ObjectPath<'_>) -> zbus::Result<()>;

    /// Request that an agent be made the default.
    fn request_default_agent(&self, agent: &zbus::zvariant::ObjectPath<'_>) -> zbus::Result<()>;
}

/// High-level adapter management operations.
pub struct AdapterManager<'a> {
    adapter: Adapter1Proxy<'a>,
    agent_manager: AgentManager1Proxy<'a>,
}

impl<'a> AdapterManager<'a> {
    /// Create a new AdapterManager for the given adapter path.
    pub async fn new(connection: &'a Connection, adapter_path: &'a str) -> anyhow::Result<Self> {
        let adapter = Adapter1Proxy::builder(connection)
            .path(adapter_path)?
            .build()
            .await?;

        let agent_manager = AgentManager1Proxy::new(connection).await?;

        Ok(Self {
            adapter,
            agent_manager,
        })
    }

    /// Ensure the adapter is powered on and configure it for A2DP sink.
    pub async fn initialise(&self, device_name: &str) -> anyhow::Result<()> {
        // Power on the adapter
        if !self.adapter.powered().await? {
            tracing::info!("Powering on Bluetooth adapter");
            self.adapter.set_powered(true).await?;
        }

        // Set the device name as seen by other Bluetooth devices
        let current_alias = self.adapter.alias().await?;
        if current_alias != device_name {
            tracing::info!(name = device_name, "Setting Bluetooth device name");
            self.adapter.set_alias(device_name).await?;
        }

        // Make discoverable so devices can pair with us
        self.adapter.set_discoverable(true).await?;
        // Enable pairing
        self.adapter.set_pairable(true).await?;

        tracing::info!(
            address = %self.adapter.address().await?,
            name = device_name,
            "Bluetooth adapter initialised"
        );

        Ok(())
    }

    /// Register the SoundSync agent with BlueZ as the default agent.
    pub async fn register_agent(&self, agent_path: &str) -> anyhow::Result<()> {
        let path = zbus::zvariant::ObjectPath::try_from(agent_path)?;

        // Capability: NoInputNoOutput — auto-accept all pairing
        self.agent_manager
            .register_agent(&path, "NoInputNoOutput")
            .await?;

        self.agent_manager.request_default_agent(&path).await?;

        tracing::info!(path = agent_path, "BlueZ agent registered as default");
        Ok(())
    }

    /// Start Bluetooth discovery (scanning for devices).
    pub async fn start_scan(&self) -> anyhow::Result<()> {
        self.adapter.start_discovery().await?;
        tracing::info!("Bluetooth scan started");
        Ok(())
    }

    /// Stop Bluetooth discovery.
    pub async fn stop_scan(&self) -> anyhow::Result<()> {
        // Only stop if currently discovering to avoid errors
        if self.adapter.discovering().await.unwrap_or(false) {
            self.adapter.stop_discovery().await?;
            tracing::info!("Bluetooth scan stopped");
        }
        Ok(())
    }

    /// Check if the adapter is powered and available.
    pub async fn is_available(&self) -> bool {
        self.adapter.powered().await.unwrap_or(false)
    }

    /// Get the adapter's Bluetooth address.
    pub async fn address(&self) -> Option<String> {
        self.adapter.address().await.ok()
    }

    /// Remove a device from the adapter.
    pub async fn remove_device(&self, device_path: &str) -> anyhow::Result<()> {
        let path = zbus::zvariant::ObjectPath::try_from(device_path)?;
        self.adapter.remove_device(&path).await?;
        Ok(())
    }
}
