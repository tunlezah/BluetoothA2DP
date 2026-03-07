//! BlueZ Agent1 implementation for auto-accepting Bluetooth pairing.
//!
//! The agent handles pairing requests by automatically accepting them
//! when the system is in pairing mode. When not scanning, unknown devices
//! are rejected per the security policy in the architecture spec.
//!
//! Capability: "NoInputNoOutput" — accepts all pairing without PIN entry.

use std::sync::{Arc, Mutex};

use zbus::{interface, Connection, ObjectServer};

/// Whether the agent should accept pairing requests.
/// Controlled by the scan/pairing window.
static PAIRING_ALLOWED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Allow or disallow pairing via the auto-accept agent.
pub fn set_pairing_allowed(allowed: bool) {
    PAIRING_ALLOWED.store(allowed, std::sync::atomic::Ordering::Relaxed);
    tracing::info!(allowed = allowed, "Bluetooth pairing window changed");
}

/// Check if pairing is currently allowed.
pub fn is_pairing_allowed() -> bool {
    PAIRING_ALLOWED.load(std::sync::atomic::Ordering::Relaxed)
}

/// BlueZ Agent1 D-Bus interface implementation.
///
/// Registered at the path `/org/soundsync/agent` on the session bus.
pub struct SoundSyncAgent;

#[interface(name = "org.bluez.Agent1")]
impl SoundSyncAgent {
    /// Called when a device needs a PIN code.
    /// We return "0000" as a default — not used for A2DP connections.
    async fn request_pin_code(&self, device: zbus::zvariant::ObjectPath<'_>) -> zbus::fdo::Result<String> {
        tracing::info!(device = %device, "Agent: RequestPinCode");
        if !is_pairing_allowed() {
            return Err(zbus::fdo::Error::Failed("Pairing not allowed".into()));
        }
        Ok("0000".to_string())
    }

    /// Called to display a PIN code to the user.
    async fn display_pin_code(
        &self,
        device: zbus::zvariant::ObjectPath<'_>,
        pincode: &str,
    ) -> zbus::fdo::Result<()> {
        tracing::info!(device = %device, pincode = pincode, "Agent: DisplayPinCode");
        Ok(())
    }

    /// Called when a device needs a passkey (6-digit number).
    async fn request_passkey(
        &self,
        device: zbus::zvariant::ObjectPath<'_>,
    ) -> zbus::fdo::Result<u32> {
        tracing::info!(device = %device, "Agent: RequestPasskey");
        if !is_pairing_allowed() {
            return Err(zbus::fdo::Error::Failed("Pairing not allowed".into()));
        }
        Ok(0)
    }

    /// Called to display a passkey during pairing.
    async fn display_passkey(
        &self,
        device: zbus::zvariant::ObjectPath<'_>,
        passkey: u32,
        _entered: u16,
    ) -> zbus::fdo::Result<()> {
        tracing::info!(device = %device, passkey = passkey, "Agent: DisplayPasskey");
        Ok(())
    }

    /// Called to confirm a passkey match. We auto-confirm.
    async fn request_confirmation(
        &self,
        device: zbus::zvariant::ObjectPath<'_>,
        passkey: u32,
    ) -> zbus::fdo::Result<()> {
        tracing::info!(device = %device, passkey = passkey, "Agent: RequestConfirmation — auto-confirming");
        if !is_pairing_allowed() {
            return Err(zbus::fdo::Error::Failed("Pairing not allowed".into()));
        }
        Ok(())
    }

    /// Called to authorize a service connection (A2DP, etc.).
    async fn authorize_service(
        &self,
        device: zbus::zvariant::ObjectPath<'_>,
        uuid: &str,
    ) -> zbus::fdo::Result<()> {
        // A2DP sink UUID: 0000110b-0000-1000-8000-00805f9b34fb
        // A2DP source UUID: 0000110a-0000-1000-8000-00805f9b34fb
        // AVRCP: 0000110e-0000-1000-8000-00805f9b34fb
        tracing::info!(device = %device, uuid = uuid, "Agent: AuthorizeService — authorizing");
        Ok(())
    }

    /// Called to authorize a connection attempt.
    async fn request_authorization(
        &self,
        device: zbus::zvariant::ObjectPath<'_>,
    ) -> zbus::fdo::Result<()> {
        tracing::info!(device = %device, "Agent: RequestAuthorization");
        if !is_pairing_allowed() {
            return Err(zbus::fdo::Error::Failed("Pairing not allowed".into()));
        }
        Ok(())
    }

    /// Called when the agent is no longer needed.
    async fn cancel(&self) -> zbus::fdo::Result<()> {
        tracing::info!("Agent: Cancel");
        Ok(())
    }

    /// Called when the agent is released by BlueZ.
    async fn release(&self) -> zbus::fdo::Result<()> {
        tracing::info!("Agent: Release");
        Ok(())
    }
}

/// Register the SoundSync Agent1 with BlueZ.
///
/// Returns the agent object path that should be used when calling
/// AgentManager1.RegisterAgent.
pub async fn register_agent(connection: &Connection) -> anyhow::Result<String> {
    let agent_path = "/org/soundsync/agent";

    connection
        .object_server()
        .at(agent_path, SoundSyncAgent)
        .await?;

    tracing::info!(path = agent_path, "BlueZ Agent1 registered");
    Ok(agent_path.to_string())
}
