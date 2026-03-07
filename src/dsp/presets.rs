//! EQ preset profiles for SoundSync.
//!
//! Presets are stored in `~/.config/soundsync/eq-presets.json`.
//! Built-in presets are always available regardless of the file.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use super::eq::{EqBand, EQ_FREQUENCIES};

/// A named EQ preset with 10 band gain values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqPreset {
    pub name: String,
    pub bands: Vec<EqBand>,
}

impl EqPreset {
    pub fn new(name: &str, gains: [f32; 10]) -> Self {
        let bands = EQ_FREQUENCIES
            .iter()
            .zip(gains.iter())
            .map(|(&freq, &gain_db)| EqBand::new(freq, gain_db))
            .collect();
        Self {
            name: name.to_string(),
            bands,
        }
    }
}

/// All available EQ presets (built-in + user-saved).
pub struct PresetManager {
    presets: HashMap<String, EqPreset>,
    config_path: PathBuf,
}

impl PresetManager {
    /// Create a preset manager, loading user presets from disk if available.
    pub fn new() -> Self {
        let config_path = dirs::config_dir()
            .map(|d| d.join("soundsync").join("eq-presets.json"))
            .unwrap_or_else(|| PathBuf::from("/tmp/soundsync/eq-presets.json"));

        let mut manager = Self {
            presets: HashMap::new(),
            config_path,
        };

        // Load built-in presets first
        manager.load_builtins();
        // Then load user presets (can override built-ins)
        manager.load_from_disk();

        manager
    }

    /// Load the built-in preset profiles.
    fn load_builtins(&mut self) {
        // Flat — all bands at 0 dB
        self.presets.insert(
            "flat".to_string(),
            EqPreset::new("flat", [0.0; 10]),
        );

        // Bass Boost — boost lows, slight high cut
        self.presets.insert(
            "bass_boost".to_string(),
            EqPreset::new(
                "bass_boost",
                [6.0, 5.0, 3.0, 1.0, 0.0, 0.0, -1.0, -1.0, -2.0, -2.0],
            ),
        );

        // Treble Boost — boost highs
        self.presets.insert(
            "treble_boost".to_string(),
            EqPreset::new(
                "treble_boost",
                [-1.0, -1.0, 0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 5.0],
            ),
        );

        // Vinyl Warm — warm analog emulation
        self.presets.insert(
            "vinyl_warm".to_string(),
            EqPreset::new(
                "vinyl_warm",
                [3.0, 4.0, 3.0, 1.0, 0.0, -1.0, -2.0, -3.0, -4.0, -5.0],
            ),
        );

        // Speech — boost voice clarity
        self.presets.insert(
            "speech".to_string(),
            EqPreset::new(
                "speech",
                [-4.0, -3.0, 0.0, 3.0, 5.0, 5.0, 3.0, 1.0, 0.0, -1.0],
            ),
        );

        // Rock — classic V-shape
        self.presets.insert(
            "rock".to_string(),
            EqPreset::new(
                "rock",
                [5.0, 4.0, 2.0, 0.0, -2.0, -1.0, 1.0, 3.0, 4.0, 5.0],
            ),
        );

        // Classical — gentle boost at extremes
        self.presets.insert(
            "classical".to_string(),
            EqPreset::new(
                "classical",
                [4.0, 3.0, 2.0, 1.0, 0.0, 0.0, 1.0, 2.0, 3.0, 4.0],
            ),
        );

        // Electronic — enhanced bass and highs
        self.presets.insert(
            "electronic".to_string(),
            EqPreset::new(
                "electronic",
                [6.0, 5.0, 1.0, -1.0, -2.0, -1.0, 2.0, 4.0, 5.0, 6.0],
            ),
        );
    }

    /// Load user-saved presets from disk.
    fn load_from_disk(&mut self) {
        if !self.config_path.exists() {
            return;
        }

        match std::fs::read_to_string(&self.config_path) {
            Ok(content) => match serde_json::from_str::<Vec<EqPreset>>(&content) {
                Ok(user_presets) => {
                    for preset in user_presets {
                        if preset.bands.len() == 10 {
                            self.presets.insert(preset.name.clone(), preset);
                        }
                    }
                    tracing::info!("Loaded user EQ presets from disk");
                }
                Err(e) => tracing::warn!("Failed to parse EQ presets: {}", e),
            },
            Err(e) => tracing::warn!("Failed to read EQ presets file: {}", e),
        }
    }

    /// Save all user presets to disk (excluding built-ins).
    pub fn save_to_disk(&self) {
        let builtins = ["flat", "bass_boost", "treble_boost", "vinyl_warm", "speech", "rock", "classical", "electronic"];

        let user_presets: Vec<&EqPreset> = self
            .presets
            .values()
            .filter(|p| !builtins.contains(&p.name.as_str()))
            .collect();

        if user_presets.is_empty() {
            return;
        }

        if let Some(parent) = self.config_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("Failed to create presets directory: {}", e);
                return;
            }
        }

        match serde_json::to_string_pretty(&user_presets) {
            Ok(content) => {
                if let Err(e) = std::fs::write(&self.config_path, content) {
                    tracing::warn!("Failed to write EQ presets: {}", e);
                }
            }
            Err(e) => tracing::warn!("Failed to serialise EQ presets: {}", e),
        }
    }

    /// Get a preset by name.
    pub fn get(&self, name: &str) -> Option<&EqPreset> {
        self.presets.get(name)
    }

    /// Save a custom preset.
    pub fn save_preset(&mut self, preset: EqPreset) {
        self.presets.insert(preset.name.clone(), preset);
        self.save_to_disk();
    }

    /// Delete a user-saved preset. Built-in presets cannot be deleted.
    pub fn delete_preset(&mut self, name: &str) -> bool {
        let builtins = ["flat", "bass_boost", "treble_boost", "vinyl_warm", "speech", "rock", "classical", "electronic"];
        if builtins.contains(&name) {
            return false; // Cannot delete built-in presets
        }
        let removed = self.presets.remove(name).is_some();
        if removed {
            self.save_to_disk();
        }
        removed
    }

    /// List all preset names.
    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self.presets.keys().cloned().collect();
        names.sort();
        names
    }
}

impl Default for PresetManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_loaded() {
        let manager = PresetManager::new();
        let names = manager.list();
        assert!(names.contains(&"flat".to_string()));
        assert!(names.contains(&"bass_boost".to_string()));
        assert!(names.contains(&"vinyl_warm".to_string()));
        assert!(names.contains(&"speech".to_string()));
    }

    #[test]
    fn flat_preset_all_zero() {
        let manager = PresetManager::new();
        let flat = manager.get("flat").unwrap();
        assert_eq!(flat.bands.len(), 10);
        for band in &flat.bands {
            assert_eq!(band.gain_db, 0.0);
        }
    }

    #[test]
    fn cannot_delete_builtin() {
        let mut manager = PresetManager::new();
        assert!(!manager.delete_preset("flat"));
        assert!(manager.get("flat").is_some());
    }
}
