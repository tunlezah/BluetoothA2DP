//! Line-in audio input management for SoundSync.
#![allow(dead_code)]
//!
//! Provides the ability to capture audio from a hardware audio input (line-in,
//! USB audio, S/PDIF, etc.) and route it to the SoundSync capture sink, where
//! it flows through the same DSP and streaming pipeline as Bluetooth A2DP audio.
//!
//! # Architecture
//!
//! When line-in is activated, a PipeWire/PulseAudio loopback module is loaded
//! that copies audio from the selected hardware source to the `soundsync-capture`
//! null sink. The spectrum analyser and browser stream then capture from that
//! sink's monitor, exactly as they do for Bluetooth audio.
//!
//! ```text
//! Line-in hardware source (ALSA/USB/HDMI)
//!     │
//!     ▼
//! module-loopback (pactl) — 48 kHz, low-latency
//!     │
//!     ▼
//! soundsync-capture (null sink — always present)
//!     │
//!     ▼
//! @DEFAULT_MONITOR@ (parec/pw-cat)
//!     │
//!     ▼
//! Spectrum analyser + browser stream (unchanged pipeline)
//! ```
//!
//! # Switching reliability
//!
//! `LineInManager::enable()` always tears down any existing loopback before
//! loading a new one, ensuring clean state regardless of how many times the
//! user switches between Bluetooth and line-in.  `LineInManager::disable()`
//! is idempotent and safe to call when no loopback is active.

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// A physical or virtual audio input source reported by PulseAudio/PipeWire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineInSource {
    /// PulseAudio/PipeWire source name (e.g. "alsa_input.pci-0000_00_1f.3.analog-stereo")
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Whether this is a sink monitor (playback output), not a physical input.
    /// Monitors are included so they can be used as a passthrough source if needed,
    /// but physical inputs are listed first.
    pub is_monitor: bool,
}

/// Manages the loopback from a hardware line-in source to the capture sink.
///
/// Holds ownership of the PulseAudio loopback module so it can be cleaned up
/// on drop or explicit `disable()`.
pub struct LineInManager {
    /// PulseAudio module ID of the active loopback, if any.
    loopback_module_id: Option<u32>,
    /// Source name used for the active loopback (for informational purposes).
    active_source: Option<String>,
}

impl LineInManager {
    pub fn new() -> Self {
        Self {
            loopback_module_id: None,
            active_source: None,
        }
    }

    /// Return all available audio input sources, excluding the SoundSync capture
    /// sink monitor (which is the internal output bus, not a real input).
    ///
    /// Sources are sorted with physical inputs first, sink monitors last.
    pub fn list_sources() -> Vec<LineInSource> {
        let output = std::process::Command::new("pactl")
            .args(["list", "sources", "short"])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout);
                parse_sources_short(&text)
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                tracing::warn!("pactl list sources short failed: {}", err.trim());
                Vec::new()
            }
            Err(e) => {
                tracing::warn!("pactl not available, cannot list sources: {}", e);
                Vec::new()
            }
        }
    }

    /// Enable line-in by loading a loopback module from `source_name`
    /// (or the system default audio input source if `None`) to `soundsync-capture`.
    ///
    /// - Tears down any existing loopback first, guaranteeing clean state.
    /// - Records the loopback module ID so it can be unloaded later.
    /// - Uses 48 kHz / stereo for maximum quality; PipeWire's built-in
    ///   high-quality resampler handles any rate conversion transparently.
    pub fn enable(&mut self, source_name: Option<&str>) -> anyhow::Result<()> {
        // Always tear down existing loopback for clean switching
        self.disable();

        let source = source_name.unwrap_or("@DEFAULT_SOURCE@");

        tracing::info!(source = %source, "Enabling line-in loopback → soundsync-capture");

        let output = std::process::Command::new("pactl")
            .args([
                "load-module",
                "module-loopback",
                &format!("source={}", source),
                "sink=soundsync-capture",
                // 40 ms is a good balance between latency and stability for live audio
                "latency_msec=40",
                // Prevent WirePlumber from moving the loopback to another sink
                "sink_dont_move=true",
                "source_dont_move=true",
                // Allow the clock-rate adjustment to keep the loopback from drifting
                "adjust_time=1",
            ])
            .output()
            .context("Failed to run pactl load-module")?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "pactl load-module module-loopback failed (source={}): {}",
                source,
                err.trim()
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let module_id: u32 = stdout
            .trim()
            .parse()
            .with_context(|| {
                format!(
                    "Failed to parse module ID from pactl output: '{}'",
                    stdout.trim()
                )
            })?;

        tracing::info!(
            module_id = module_id,
            source = %source,
            "Line-in loopback active"
        );

        self.loopback_module_id = Some(module_id);
        self.active_source = Some(source.to_string());

        Ok(())
    }

    /// Disable line-in by unloading the loopback module.
    ///
    /// Safe to call when no loopback is active (no-op in that case).
    pub fn disable(&mut self) {
        if let Some(id) = self.loopback_module_id.take() {
            tracing::info!(module_id = id, "Disabling line-in loopback");
            let result = std::process::Command::new("pactl")
                .args(["unload-module", &id.to_string()])
                .status();

            match result {
                Ok(status) if status.success() => {
                    tracing::debug!(module_id = id, "Line-in loopback unloaded cleanly");
                }
                Ok(status) => {
                    // The module may have already been unloaded (e.g. if PipeWire restarted).
                    // This is not a fatal error.
                    tracing::warn!(
                        module_id = id,
                        exit_code = ?status.code(),
                        "pactl unload-module returned non-zero (module may already be gone)"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        module_id = id,
                        error = %e,
                        "Failed to run pactl unload-module"
                    );
                }
            }

            self.active_source = None;
        }
    }

    /// Whether a line-in loopback is currently active.
    pub fn is_active(&self) -> bool {
        self.loopback_module_id.is_some()
    }

    /// The source name used for the active loopback, if any.
    pub fn active_source(&self) -> Option<&str> {
        self.active_source.as_deref()
    }
}

impl Drop for LineInManager {
    fn drop(&mut self) {
        // Ensure the loopback module is unloaded when the manager is dropped
        // (e.g. on application shutdown).
        self.disable();
    }
}

// ── Source parsing ────────────────────────────────────────────────────────────

/// Parse `pactl list sources short` output into `LineInSource` entries.
///
/// The format is tab-separated:
/// ```text
/// <id>  <name>  <module>  <sample_spec>  <state>
/// ```
fn parse_sources_short(text: &str) -> Vec<LineInSource> {
    let mut sources: Vec<LineInSource> = text
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(5, '\t').collect();
            if parts.len() < 2 {
                return None;
            }
            let name = parts[1].trim();

            // Always skip the soundsync capture sink monitor — that is our own
            // internal output bus, never a valid line-in source.
            if name.contains("soundsync-capture") {
                return None;
            }

            let is_monitor = name.ends_with(".monitor") || name.contains(".monitor");
            let description = source_description(name);

            Some(LineInSource {
                name: name.to_string(),
                description,
                is_monitor,
            })
        })
        .collect();

    // Physical inputs first, monitors last
    sources.sort_by(|a, b| a.is_monitor.cmp(&b.is_monitor));
    sources
}

/// Build a human-readable description from a PulseAudio source name.
fn source_description(name: &str) -> String {
    let lower = name.to_ascii_lowercase();

    if lower.contains("alsa_input") {
        if lower.contains("usb") {
            return "USB Audio Input".to_string();
        }
        if lower.contains("hdmi") {
            return "HDMI Input".to_string();
        }
        if lower.contains("iec958") || lower.contains("spdif") || lower.contains("digital") {
            return "Digital Input (S/PDIF)".to_string();
        }
        if lower.contains("analog") || lower.contains("line") {
            return "Analog Line In".to_string();
        }
        return "Audio Input".to_string();
    }

    if lower.ends_with(".monitor") {
        let base = name.trim_end_matches(".monitor");
        return format!("Monitor: {}", base);
    }

    if lower.contains("monitor") {
        return format!("Monitor: {}", name);
    }

    name.to_string()
}
