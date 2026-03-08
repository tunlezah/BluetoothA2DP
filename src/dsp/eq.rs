//! 10-band graphic equaliser for SoundSync.
#![allow(dead_code)]
//!
//! Standard audio frequencies from the design spec:
//! 60Hz, 120Hz, 250Hz, 500Hz, 1kHz, 2kHz, 4kHz, 8kHz, 12kHz, 16kHz
//!
//! Each band is implemented as a peaking biquad IIR filter.
//! Gain range: -12dB to +12dB per band.
//! Processing: Float32 stereo interleaved.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::biquad::{BiquadCoeffs, StereoBiquad};

/// Standard 10-band EQ centre frequencies (Hz).
pub const EQ_FREQUENCIES: [f64; 10] = [
    60.0, 120.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 12000.0, 16000.0,
];

/// Q factor for graphic EQ bands (constant-Q, √2).
pub const EQ_Q_FACTOR: f64 = std::f64::consts::SQRT_2;

/// Gain range limits in dB.
pub const EQ_GAIN_MIN: f32 = -12.0;
pub const EQ_GAIN_MAX: f32 = 12.0;

/// A single EQ band with serializable parameters.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct EqBand {
    /// Centre frequency in Hz
    pub freq: f64,
    /// Gain in dB (-12.0 to +12.0)
    pub gain_db: f32,
}

impl EqBand {
    pub fn new(freq: f64, gain_db: f32) -> Self {
        Self {
            freq,
            gain_db: gain_db.clamp(EQ_GAIN_MIN, EQ_GAIN_MAX),
        }
    }

    /// Return the 10 default bands (all flat at 0 dB).
    pub fn default_bands() -> Vec<EqBand> {
        EQ_FREQUENCIES
            .iter()
            .map(|&freq| EqBand::new(freq, 0.0))
            .collect()
    }

    /// Compute the biquad coefficients for this band at the given sample rate.
    pub fn coefficients(&self, sample_rate: f64) -> BiquadCoeffs {
        BiquadCoeffs::peaking_eq(self.freq, self.gain_db as f64, EQ_Q_FACTOR, sample_rate)
    }
}

/// 10-band graphic equaliser.
///
/// Thread-safe: the filter array is protected by a Mutex so coefficients
/// can be updated from the API thread while audio is processed in the
/// PipeWire callback thread.
pub struct Equaliser {
    /// The 10 stereo biquad filters, one per EQ band.
    filters: Mutex<[StereoBiquad; 10]>,
    /// Current band gains (for state queries).
    bands: Mutex<Vec<EqBand>>,
    /// Whether EQ processing is enabled.
    enabled: AtomicBool,
    /// Sample rate in Hz.
    sample_rate: f64,
}

impl Equaliser {
    /// Create a new flat (all-pass) equaliser.
    pub fn new(sample_rate: f64) -> Self {
        let filters = std::array::from_fn(|_| StereoBiquad::identity());
        let bands = EqBand::default_bands();
        Self {
            filters: Mutex::new(filters),
            bands: Mutex::new(bands),
            enabled: AtomicBool::new(true),
            sample_rate,
        }
    }

    /// Update a single band's gain.
    ///
    /// Coefficients are updated without resetting filter state, providing
    /// smooth transitions without clicks.
    pub fn set_band_gain(&self, band_index: usize, gain_db: f32) {
        if band_index >= 10 {
            tracing::warn!("EQ band index {} out of range", band_index);
            return;
        }
        let gain_db = gain_db.clamp(EQ_GAIN_MIN, EQ_GAIN_MAX);

        let mut bands = self.bands.lock().unwrap();
        bands[band_index].gain_db = gain_db;
        let band = bands[band_index];
        drop(bands);

        let coeffs = band.coefficients(self.sample_rate);
        let mut filters = self.filters.lock().unwrap();
        filters[band_index].update_coeffs(coeffs);

        tracing::debug!(
            band = band_index,
            freq = band.freq,
            gain_db = gain_db,
            "EQ band updated"
        );
    }

    /// Apply a full set of EQ bands at once.
    pub fn set_bands(&self, bands: &[EqBand]) {
        if bands.len() != 10 {
            tracing::warn!("Expected 10 EQ bands, got {}", bands.len());
            return;
        }

        let mut new_filters = [
            StereoBiquad::identity(),
            StereoBiquad::identity(),
            StereoBiquad::identity(),
            StereoBiquad::identity(),
            StereoBiquad::identity(),
            StereoBiquad::identity(),
            StereoBiquad::identity(),
            StereoBiquad::identity(),
            StereoBiquad::identity(),
            StereoBiquad::identity(),
        ];

        for (i, band) in bands.iter().enumerate() {
            let gain_db = band.gain_db.clamp(EQ_GAIN_MIN, EQ_GAIN_MAX);
            let clamped_band = EqBand::new(band.freq, gain_db);
            new_filters[i] = StereoBiquad::new(clamped_band.coefficients(self.sample_rate));
        }

        {
            let mut filters = self.filters.lock().unwrap();
            *filters = new_filters;
        }
        {
            let mut current_bands = self.bands.lock().unwrap();
            *current_bands = bands
                .iter()
                .map(|b| EqBand::new(b.freq, b.gain_db.clamp(EQ_GAIN_MIN, EQ_GAIN_MAX)))
                .collect();
        }

        tracing::info!("EQ bands updated");
    }

    /// Get current band settings.
    pub fn get_bands(&self) -> Vec<EqBand> {
        self.bands.lock().unwrap().clone()
    }

    /// Enable or disable EQ processing.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        tracing::info!(enabled = enabled, "EQ enabled state changed");
    }

    /// Whether EQ processing is currently enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Reset all bands to flat (0 dB gain).
    pub fn reset(&self) {
        let flat_bands = EqBand::default_bands();
        self.set_bands(&flat_bands);
    }

    /// Process a buffer of stereo interleaved float32 samples in-place.
    ///
    /// Buffer format: [L, R, L, R, ...]
    ///
    /// This is the hot path, called from the PipeWire audio callback.
    /// The Mutex lock ensures thread safety, but the lock contention
    /// should be minimal as coefficient updates are infrequent.
    pub fn process_interleaved(&self, buffer: &mut [f32]) {
        if !self.is_enabled() {
            return;
        }

        let mut filters = self.filters.lock().unwrap();

        let mut i = 0;
        while i + 1 < buffer.len() {
            let mut left = buffer[i];
            let mut right = buffer[i + 1];

            // Apply each of the 10 EQ bands in series
            for filter in filters.iter_mut() {
                (left, right) = filter.process(left, right);
            }

            buffer[i] = left;
            buffer[i + 1] = right;
            i += 2;
        }
    }

    /// Process separate left and right channel buffers in-place.
    pub fn process_planar(&self, left: &mut [f32], right: &mut [f32]) {
        if !self.is_enabled() {
            return;
        }

        let len = left.len().min(right.len());
        let mut filters = self.filters.lock().unwrap();

        for i in 0..len {
            let mut l = left[i];
            let mut r = right[i];

            for filter in filters.iter_mut() {
                (l, r) = filter.process(l, r);
            }

            left[i] = l;
            right[i] = r;
        }
    }
}

/// Arc-wrapped equaliser for sharing between threads.
pub type EqualiserHandle = Arc<Equaliser>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_eq_passes_through() {
        let eq = Equaliser::new(48000.0);
        let mut buffer = vec![0.5f32, -0.3f32, 0.8f32, -0.1f32];
        let original = buffer.clone();
        eq.process_interleaved(&mut buffer);
        for (a, b) in original.iter().zip(buffer.iter()) {
            assert!(
                (a - b).abs() < 0.01,
                "Flat EQ should pass through: {} vs {}",
                a,
                b
            );
        }
    }

    #[test]
    fn disabled_eq_passes_through() {
        let eq = Equaliser::new(48000.0);
        // Set non-zero gain
        eq.set_band_gain(4, 6.0);
        eq.set_enabled(false);

        let mut buffer = vec![0.5f32, -0.3f32, 0.8f32, -0.1f32];
        let original = buffer.clone();
        eq.process_interleaved(&mut buffer);
        for (a, b) in original.iter().zip(buffer.iter()) {
            assert!((a - b).abs() < 1e-6, "Disabled EQ should not modify signal");
        }
    }

    #[test]
    fn set_bands_applies_all_gains() {
        let eq = Equaliser::new(48000.0);
        let mut bands = EqBand::default_bands();
        bands[4].gain_db = 6.0; // 1kHz boost
        eq.set_bands(&bands);

        let retrieved = eq.get_bands();
        assert_eq!(retrieved[4].gain_db, 6.0);
        assert_eq!(retrieved[0].gain_db, 0.0);
    }

    #[test]
    fn gain_clamped_in_set_band() {
        let eq = Equaliser::new(48000.0);
        eq.set_band_gain(0, 100.0); // Should clamp to 12.0
        let bands = eq.get_bands();
        assert_eq!(bands[0].gain_db, 12.0);
    }

    #[test]
    fn reset_flattens_all_bands() {
        let eq = Equaliser::new(48000.0);
        eq.set_band_gain(3, 8.0);
        eq.set_band_gain(7, -5.0);
        eq.reset();
        let bands = eq.get_bands();
        for band in &bands {
            assert_eq!(band.gain_db, 0.0, "All bands should be flat after reset");
        }
    }
}
