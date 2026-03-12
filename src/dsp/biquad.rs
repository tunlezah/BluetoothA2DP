//! Biquad IIR digital filter implementation.
#![allow(dead_code)]
//!
//! Implements peaking EQ biquad filters using the Audio EQ Cookbook
//! formulas by Robert Bristow-Johnson.
//!
//! Transfer function:
//!   H(z) = (b0 + b1*z^-1 + b2*z^-2) / (1 + a1*z^-1 + a2*z^-2)
//!
//! All coefficients are pre-divided by a0 for efficiency.

use libm::{cos, sin};
use std::f64::consts::PI;

/// Biquad filter coefficients.
/// All normalised (divided by a0) for efficient per-sample processing.
#[derive(Debug, Clone, Copy)]
pub struct BiquadCoeffs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl BiquadCoeffs {
    /// Identity filter — passes signal unchanged.
    pub fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }

    /// Compute a low-shelf biquad (Audio EQ Cookbook, S = 1 max slope).
    ///
    /// Boosts or cuts all frequencies below `freq_hz`. Used for the 60 Hz band.
    pub fn low_shelf(freq_hz: f64, gain_db: f64, sample_rate: f64) -> Self {
        let gain_db = gain_db.clamp(-12.0, 12.0);
        if gain_db.abs() < 0.01 {
            return Self::identity();
        }

        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq_hz / sample_rate;
        // alpha for S=1: sin(w0)/2 * sqrt(2) (shelf slope = maximum)
        let alpha = sin(w0) / 2.0 * 2f64.sqrt();
        let cos_w0 = cos(w0);
        let sqrt_a = a.sqrt();

        let b0 = a * ((a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha);
        let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0);
        let b2 = a * ((a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha);
        let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha;
        let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0);
        let a2 = (a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha;

        Self {
            b0: (b0 / a0) as f32,
            b1: (b1 / a0) as f32,
            b2: (b2 / a0) as f32,
            a1: (a1 / a0) as f32,
            a2: (a2 / a0) as f32,
        }
    }

    /// Compute a high-shelf biquad (Audio EQ Cookbook, S = 1 max slope).
    ///
    /// Boosts or cuts all frequencies above `freq_hz`. Used for the 16 kHz band.
    pub fn high_shelf(freq_hz: f64, gain_db: f64, sample_rate: f64) -> Self {
        let gain_db = gain_db.clamp(-12.0, 12.0);
        if gain_db.abs() < 0.01 {
            return Self::identity();
        }

        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq_hz / sample_rate;
        let alpha = sin(w0) / 2.0 * 2f64.sqrt();
        let cos_w0 = cos(w0);
        let sqrt_a = a.sqrt();

        let b0 = a * ((a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha);
        let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0);
        let b2 = a * ((a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha);
        let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha;
        let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos_w0);
        let a2 = (a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha;

        Self {
            b0: (b0 / a0) as f32,
            b1: (b1 / a0) as f32,
            b2: (b2 / a0) as f32,
            a1: (a1 / a0) as f32,
            a2: (a2 / a0) as f32,
        }
    }

    /// Compute peaking EQ biquad coefficients.
    ///
    /// # Arguments
    /// * `freq_hz`    - Centre frequency in Hz
    /// * `gain_db`    - Gain in dB (-12.0 to +12.0)
    /// * `q`          - Q factor (1.41 = √2 for constant-Q graphic EQ)
    /// * `sample_rate` - Sample rate in Hz (typically 48000)
    pub fn peaking_eq(freq_hz: f64, gain_db: f64, q: f64, sample_rate: f64) -> Self {
        // Clamp gain to safe range to prevent clipping
        let gain_db = gain_db.clamp(-12.0, 12.0);

        // If gain is essentially zero, return identity to avoid computation
        if gain_db.abs() < 0.01 {
            return Self::identity();
        }

        // A = sqrt(10^(dBgain/20)) = 10^(dBgain/40)
        let a = 10f64.powf(gain_db / 40.0);

        // w0 = 2*pi*f0/Fs
        let w0 = 2.0 * PI * freq_hz / sample_rate;

        // alpha = sin(w0)/(2*Q)
        let alpha = sin(w0) / (2.0 * q);

        let cos_w0 = cos(w0);

        // Peaking EQ coefficients from Audio EQ Cookbook
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha / a;

        // Normalise by a0
        Self {
            b0: (b0 / a0) as f32,
            b1: (b1 / a0) as f32,
            b2: (b2 / a0) as f32,
            a1: (a1 / a0) as f32,
            a2: (a2 / a0) as f32,
        }
    }
}

/// Biquad filter state (delay elements for one channel).
///
/// Each channel requires its own state. Filters should not share state
/// across channels.
#[derive(Debug, Clone, Copy, Default)]
pub struct BiquadState {
    /// w[n-1]: previous input-side delay element
    w1: f32,
    /// w[n-2]: two-sample delayed input-side element
    w2: f32,
}

impl BiquadState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process a single sample through the biquad filter.
    ///
    /// Uses the Direct Form II transposed structure for numerical stability.
    ///
    /// # Arguments
    /// * `input` - Input sample
    /// * `coeffs` - Filter coefficients
    ///
    /// # Returns
    /// Filtered output sample
    #[inline(always)]
    pub fn process(&mut self, input: f32, coeffs: &BiquadCoeffs) -> f32 {
        // Direct Form II transposed
        let output = coeffs.b0 * input + self.w1;
        self.w1 = coeffs.b1 * input - coeffs.a1 * output + self.w2;
        self.w2 = coeffs.b2 * input - coeffs.a2 * output;
        output
    }

    /// Reset the filter state (flush delay elements).
    pub fn reset(&mut self) {
        self.w1 = 0.0;
        self.w2 = 0.0;
    }
}

/// A stereo biquad filter (one state per channel).
#[derive(Debug, Clone)]
pub struct StereoBiquad {
    pub coeffs: BiquadCoeffs,
    state_l: BiquadState,
    state_r: BiquadState,
}

impl StereoBiquad {
    pub fn new(coeffs: BiquadCoeffs) -> Self {
        Self {
            coeffs,
            state_l: BiquadState::new(),
            state_r: BiquadState::new(),
        }
    }

    pub fn identity() -> Self {
        Self::new(BiquadCoeffs::identity())
    }

    /// Update coefficients without resetting state (for smooth transitions).
    pub fn update_coeffs(&mut self, coeffs: BiquadCoeffs) {
        self.coeffs = coeffs;
    }

    /// Process a stereo sample pair.
    #[inline(always)]
    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let out_l = self.state_l.process(left, &self.coeffs);
        let out_r = self.state_r.process(right, &self.coeffs);
        (out_l, out_r)
    }

    /// Reset both channel states.
    pub fn reset(&mut self) {
        self.state_l.reset();
        self.state_r.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_passes_through() {
        let mut filter = StereoBiquad::identity();
        for i in 0..100 {
            let sample = (i as f32) * 0.01;
            let (l, r) = filter.process(sample, sample);
            assert!(
                (l - sample).abs() < 1e-6,
                "Identity filter should pass through"
            );
            assert!(
                (r - sample).abs() < 1e-6,
                "Identity filter should pass through"
            );
        }
    }

    #[test]
    fn peaking_eq_boost_increases_gain() {
        let coeffs = BiquadCoeffs::peaking_eq(1000.0, 6.0, 1.41, 48000.0);
        // b0 should be > 1 for boost
        assert!(coeffs.b0 > 1.0, "Boost should increase b0 coefficient");
    }

    #[test]
    fn peaking_eq_cut_decreases_gain() {
        let coeffs = BiquadCoeffs::peaking_eq(1000.0, -6.0, 1.41, 48000.0);
        // b0 should be < 1 for cut
        assert!(coeffs.b0 < 1.0, "Cut should decrease b0 coefficient");
    }

    #[test]
    fn zero_gain_returns_identity() {
        let coeffs = BiquadCoeffs::peaking_eq(1000.0, 0.0, 1.41, 48000.0);
        assert!((coeffs.b0 - 1.0).abs() < 1e-6);
        assert!(coeffs.b1.abs() < 1e-6);
        assert!(coeffs.b2.abs() < 1e-6);
        assert!(coeffs.a1.abs() < 1e-6);
        assert!(coeffs.a2.abs() < 1e-6);
    }

    #[test]
    fn gain_clamped_to_safe_range() {
        // Should not panic with extreme gain values
        let coeffs_high = BiquadCoeffs::peaking_eq(1000.0, 100.0, 1.41, 48000.0);
        let coeffs_low = BiquadCoeffs::peaking_eq(1000.0, -100.0, 1.41, 48000.0);
        assert!(coeffs_high.b0.is_finite());
        assert!(coeffs_low.b0.is_finite());
    }
}
