//! SoundSync library — public API for integration testing.
//!
//! This crate is primarily a binary, but exposes a lib target so that
//! integration tests can access internal modules without duplication.

pub mod bluetooth;
pub mod dsp;
pub mod logging;
pub mod state;

// Re-export commonly used types for integration tests
pub use state::{AppStateHandle, BluetoothStatus, Config, DeviceInfo, DeviceState, SystemEvent};
