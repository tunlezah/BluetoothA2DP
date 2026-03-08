//! Integration tests for the DSP equaliser.

use soundsync::dsp::{
    biquad::{BiquadCoeffs, BiquadState, StereoBiquad},
    eq::{EqBand, Equaliser, EQ_FREQUENCIES, EQ_GAIN_MAX, EQ_GAIN_MIN},
    presets::PresetManager,
};

// ── Biquad tests ─────────────────────────────────────────────────────────────

#[test]
fn biquad_identity_passthrough() {
    let mut f = StereoBiquad::identity();
    let samples = [-0.9, -0.5, 0.0, 0.5, 0.9, 0.3, -0.7];
    for &s in &samples {
        let (l, r) = f.process(s, s);
        assert!((l - s).abs() < 1e-5, "Identity filter must pass through");
        assert!((r - s).abs() < 1e-5, "Identity filter must pass through");
    }
}

#[test]
fn biquad_peaking_boost_increases_level() {
    let coeffs = BiquadCoeffs::peaking_eq(1000.0, 6.0, 1.41, 48000.0);
    // b0 > 1 for a boost filter
    assert!(
        coeffs.b0 > 1.0,
        "6dB boost should increase b0: got {}",
        coeffs.b0
    );
}

#[test]
fn biquad_peaking_cut_decreases_level() {
    let coeffs = BiquadCoeffs::peaking_eq(1000.0, -6.0, 1.41, 48000.0);
    assert!(
        coeffs.b0 < 1.0,
        "6dB cut should decrease b0: got {}",
        coeffs.b0
    );
}

#[test]
fn biquad_zero_gain_is_identity() {
    let coeffs = BiquadCoeffs::peaking_eq(1000.0, 0.0, 1.41, 48000.0);
    assert!((coeffs.b0 - 1.0).abs() < 1e-6);
    assert!(coeffs.b1.abs() < 1e-6);
    assert!(coeffs.b2.abs() < 1e-6);
    assert!(coeffs.a1.abs() < 1e-6);
    assert!(coeffs.a2.abs() < 1e-6);
}

#[test]
fn biquad_gain_clamped_to_range() {
    // Extreme values should not produce NaN/Inf
    let c1 = BiquadCoeffs::peaking_eq(1000.0, 999.0, 1.41, 48000.0);
    let c2 = BiquadCoeffs::peaking_eq(1000.0, -999.0, 1.41, 48000.0);
    assert!(c1.b0.is_finite());
    assert!(c2.b0.is_finite());
}

#[test]
fn biquad_state_reset_clears_delay() {
    let coeffs = BiquadCoeffs::peaking_eq(1000.0, 6.0, 1.41, 48000.0);
    let mut state = BiquadState::new();

    // Process some samples to fill delay elements
    for _ in 0..100 {
        state.process(0.5, &coeffs);
    }

    // Reset and verify output matches fresh state
    state.reset();
    let mut fresh = BiquadState::new();
    let out1 = state.process(0.1, &coeffs);
    let out2 = fresh.process(0.1, &coeffs);
    assert!(
        (out1 - out2).abs() < 1e-6,
        "Reset state should match fresh state"
    );
}

// ── Equaliser tests ──────────────────────────────────────────────────────────

#[test]
fn equaliser_flat_passes_through() {
    let eq = Equaliser::new(48000.0);
    let mut buf = vec![0.5f32, -0.3f32, 0.8f32, -0.8f32, 0.1f32, 0.0f32];
    let original = buf.clone();
    eq.process_interleaved(&mut buf);
    for (a, b) in original.iter().zip(buf.iter()) {
        assert!((a - b).abs() < 0.01, "Flat EQ: {} ≠ {}", a, b);
    }
}

#[test]
fn equaliser_disabled_is_passthrough() {
    let eq = Equaliser::new(48000.0);
    eq.set_band_gain(4, 12.0); // 1kHz big boost
    eq.set_enabled(false);
    let mut buf = vec![0.5f32, -0.3f32, 0.8f32, -0.8f32];
    let original = buf.clone();
    eq.process_interleaved(&mut buf);
    for (a, b) in original.iter().zip(buf.iter()) {
        assert!((a - b).abs() < 1e-6);
    }
}

#[test]
fn equaliser_set_band_gain_persists() {
    let eq = Equaliser::new(48000.0);
    eq.set_band_gain(3, 7.5);
    let bands = eq.get_bands();
    assert_eq!(bands[3].gain_db, 7.5);
}

#[test]
fn equaliser_gain_clamped() {
    let eq = Equaliser::new(48000.0);
    eq.set_band_gain(0, EQ_GAIN_MAX + 5.0);
    assert_eq!(eq.get_bands()[0].gain_db, EQ_GAIN_MAX);

    eq.set_band_gain(9, EQ_GAIN_MIN - 5.0);
    assert_eq!(eq.get_bands()[9].gain_db, EQ_GAIN_MIN);
}

#[test]
fn equaliser_set_all_bands() {
    let eq = Equaliser::new(48000.0);
    let gains = [3.0, 2.0, 1.0, 0.0, -1.0, -2.0, -1.0, 0.0, 1.0, 2.0];
    let bands: Vec<EqBand> = EQ_FREQUENCIES
        .iter()
        .zip(gains.iter())
        .map(|(&freq, &gain_db)| EqBand::new(freq, gain_db))
        .collect();
    eq.set_bands(&bands);
    let retrieved = eq.get_bands();
    for (i, (&expected, got)) in gains.iter().zip(retrieved.iter()).enumerate() {
        assert_eq!(got.gain_db, expected, "Band {} gain mismatch", i);
    }
}

#[test]
fn equaliser_reset_flattens() {
    let eq = Equaliser::new(48000.0);
    eq.set_band_gain(0, 10.0);
    eq.set_band_gain(9, -10.0);
    eq.reset();
    for band in eq.get_bands() {
        assert_eq!(band.gain_db, 0.0);
    }
}

#[test]
fn equaliser_planar_processing() {
    let eq = Equaliser::new(48000.0);
    let mut left = vec![0.5f32; 64];
    let mut right = vec![-0.3f32; 64];
    let orig_l = left.clone();
    let _orig_r = right.clone();

    // Flat EQ should preserve signal
    eq.process_planar(&mut left, &mut right);
    for (a, b) in orig_l.iter().zip(left.iter()) {
        assert!((a - b).abs() < 0.01);
    }
}

#[test]
fn equaliser_band_count_validation() {
    let eq = Equaliser::new(48000.0);
    // set_bands with wrong count should be ignored (no panic)
    let bad_bands: Vec<EqBand> = (0..5).map(|i| EqBand::new(1000.0, i as f32)).collect();
    eq.set_bands(&bad_bands); // should not panic, just warn
                              // Bands remain at default
    assert_eq!(eq.get_bands().len(), 10);
}

// ── Preset tests ─────────────────────────────────────────────────────────────

#[test]
fn presets_builtins_present() {
    let manager = PresetManager::new();
    let list = manager.list();
    let required = [
        "flat",
        "bass_boost",
        "vinyl_warm",
        "speech",
        "rock",
        "classical",
        "electronic",
    ];
    for name in &required {
        assert!(list.contains(&name.to_string()), "Missing preset: {}", name);
    }
}

#[test]
fn preset_flat_all_zero() {
    let manager = PresetManager::new();
    let flat = manager.get("flat").expect("flat preset must exist");
    assert_eq!(flat.bands.len(), 10);
    for band in &flat.bands {
        assert_eq!(
            band.gain_db, 0.0,
            "Flat preset should have 0dB gain on all bands"
        );
    }
}

#[test]
fn preset_bass_boost_has_low_freq_boost() {
    let manager = PresetManager::new();
    let preset = manager.get("bass_boost").expect("bass_boost must exist");
    // 60Hz and 120Hz should be boosted
    assert!(
        preset.bands[0].gain_db > 0.0,
        "Bass boost should boost 60Hz"
    );
    assert!(
        preset.bands[1].gain_db > 0.0,
        "Bass boost should boost 120Hz"
    );
}

#[test]
fn preset_bands_within_limits() {
    let manager = PresetManager::new();
    for name in manager.list() {
        let preset = manager.get(&name).unwrap();
        for band in &preset.bands {
            assert!(
                band.gain_db >= EQ_GAIN_MIN && band.gain_db <= EQ_GAIN_MAX,
                "Preset '{}' band {} out of range: {}",
                name,
                band.freq,
                band.gain_db
            );
        }
    }
}

#[test]
fn preset_correct_frequencies() {
    let manager = PresetManager::new();
    let flat = manager.get("flat").unwrap();
    let expected_freqs = [
        60.0, 120.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 12000.0, 16000.0,
    ];
    for (i, (&expected, band)) in expected_freqs.iter().zip(flat.bands.iter()).enumerate() {
        assert_eq!(band.freq, expected, "Band {} frequency mismatch", i);
    }
}

#[test]
fn cannot_delete_builtin_preset() {
    let mut manager = PresetManager::new();
    assert!(
        !manager.delete_preset("flat"),
        "Should not be able to delete flat preset"
    );
    assert!(
        manager.get("flat").is_some(),
        "Flat preset must still exist"
    );
}
