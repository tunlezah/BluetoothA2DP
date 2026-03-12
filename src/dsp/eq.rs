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

/// Q factor for interior graphic EQ bands (constant-Q, √2 for 1-octave bands).
pub const EQ_Q_FACTOR: f64 = std::f64::consts::SQRT_2;

/// Per-band Q factors, calculated from the octave spacing between adjacent bands.
///
/// Bands 0 (60 Hz) and 9 (16 kHz) use shelving filters; the Q value here is
/// unused for them but kept for indexing consistency.
/// Bands 7 (8 kHz) and 8 (12 kHz) are spaced closer than 1 octave, so their
/// Q is raised to narrow the bell accordingly.
pub const EQ_BAND_Q: [f64; 10] = [
    0.707, // 60 Hz  — low shelf (Q unused, included for alignment)
    1.414, // 120 Hz — ~1-octave band
    1.414, // 250 Hz — ~1-octave band
    1.414, // 500 Hz — 1-octave band
    1.414, // 1 kHz  — 1-octave band
    1.414, // 2 kHz  — 1-octave band
    1.414, // 4 kHz  — 1-octave band
    1.820, // 8 kHz  — 0.78-octave band (8→12 kHz)
    2.870, // 12 kHz — 0.5-octave band  (8→16 kHz split)
    0.707, // 16 kHz — high shelf (Q unused, included for alignment)
];

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
    /// Uses a peaking filter with √2 Q (kept for backward-compatible call sites).
    pub fn coefficients(&self, sample_rate: f64) -> BiquadCoeffs {
        BiquadCoeffs::peaking_eq(self.freq, self.gain_db as f64, EQ_Q_FACTOR, sample_rate)
    }
}

/// Compute per-band coefficients using the correct filter type and Q.
///
/// - Band 0 (60 Hz)  → low shelf
/// - Band 9 (16 kHz) → high shelf
/// - All others      → peaking biquad with per-band Q from EQ_BAND_Q
pub fn make_band_coeffs(band_index: usize, band: &EqBand, sample_rate: f64) -> BiquadCoeffs {
    let gain = band.gain_db as f64;
    match band_index {
        0 => BiquadCoeffs::low_shelf(band.freq, gain, sample_rate),
        9 => BiquadCoeffs::high_shelf(band.freq, gain, sample_rate),
        i => {
            let q = EQ_BAND_Q.get(i).copied().unwrap_or(EQ_Q_FACTOR);
            BiquadCoeffs::peaking_eq(band.freq, gain, q, sample_rate)
        }
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
    /// Coefficients are updated without resetting filter state so the audio
    /// thread never sees a discontinuity. Uses the correct filter type
    /// (low shelf / high shelf / peaking) and per-band Q factor for the band.
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

        let coeffs = make_band_coeffs(band_index, &band, self.sample_rate);
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
    ///
    /// Coefficients are updated in-place via `update_coeffs` so filter state
    /// (delay elements w1/w2) is preserved for each band, preventing the
    /// click/zipper noise that a full filter replacement would cause.
    pub fn set_bands(&self, bands: &[EqBand]) {
        if bands.len() != 10 {
            tracing::warn!("Expected 10 EQ bands, got {}", bands.len());
            return;
        }

        {
            let mut filters = self.filters.lock().unwrap();
            for (i, band) in bands.iter().enumerate() {
                let gain_db = band.gain_db.clamp(EQ_GAIN_MIN, EQ_GAIN_MAX);
                let clamped = EqBand::new(band.freq, gain_db);
                let coeffs = make_band_coeffs(i, &clamped, self.sample_rate);
                filters[i].update_coeffs(coeffs);
            }
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
