//! Real-time audio spectrum analysis for the web UI visualiser.
//!
//! # How it works
//!
//! 1. When audio becomes active (`StreamStarted`), this module spawns a system
//!    thread that runs `parec` (or `pw-cat` as a fallback) to capture a mono
//!    downmix of the default audio sink monitor at 44 100 Hz / float32.
//!
//! 2. The thread reads 2048-sample blocks, applies a Hanning window, runs a
//!    2048-point FFT (via `rustfft`), maps the resulting magnitudes onto 64
//!    log-spaced frequency bands (20 Hz – 20 kHz), and normalises each band
//!    to a 0.0–1.0 dB scale where 1.0 = 0 dBFS and 0.0 = −80 dBFS.
//!
//! 3. An exponential moving average (α = 0.35) is applied for visual
//!    smoothness, and then `SpectrumData { bands }` is broadcast on the
//!    application event bus so the WebSocket layer can forward it to all
//!    connected browsers.
//!
//! 4. When `StreamStopped` is received the capture thread is signalled to
//!    exit, the subprocess is killed, and a zero-filled spectrum is broadcast
//!    so the UI returns to its idle state.
//!
//! # Fallback
//!
//! If neither `parec` nor `pw-cat` is available the task logs a warning and
//! continues without spectrum data. The UI gracefully shows "No Signal".

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rustfft::{num_complex::Complex, FftPlanner};

use crate::state::{AppStateHandle, SystemEvent};

/// Number of output frequency bands sent to the browser.
pub const NUM_BANDS: usize = 64;

/// FFT window size in samples.
const FFT_SIZE: usize = 2048;

/// Audio capture sample rate.
const SAMPLE_RATE: f32 = 44_100.0;

/// Exponential moving-average smoothing factor.
const SMOOTH_ALPHA: f32 = 0.35;

/// Minimum frequency of the first band (Hz).
const F_MIN: f32 = 20.0;

/// Maximum frequency of the last band (Hz).
const F_MAX: f32 = 20_000.0;

// ── Public entry point ────────────────────────────────────────────────────────

/// Async manager task — starts/stops the capture thread based on stream events.
pub async fn run_spectrum_analyzer(state: AppStateHandle) {
    let mut rx = state.subscribe();
    // `stop` flag shared with the current capture thread (if any).
    let mut current_stop: Option<Arc<AtomicBool>> = None;

    tracing::info!("Spectrum analyzer task started");

    // If audio is already active when we start, launch capture immediately.
    {
        let s = state.state.read().await;
        let streaming = s.devices.values().any(|d| d.state.is_streaming());
        if streaming {
            current_stop = Some(start_capture_thread(state.clone()));
        }
    }

    loop {
        match rx.recv().await {
            Ok(SystemEvent::StreamStarted { .. }) => {
                // Stop any running capture
                if let Some(flag) = current_stop.take() {
                    flag.store(true, Ordering::SeqCst);
                    // Give the thread a moment to clean up
                    tokio::time::sleep(Duration::from_millis(120)).await;
                }
                current_stop = Some(start_capture_thread(state.clone()));
            }

            Ok(SystemEvent::StreamStopped { .. }) => {
                if let Some(flag) = current_stop.take() {
                    flag.store(true, Ordering::SeqCst);
                }
                // Push a silent/zeroed spectrum so the UI clears
                state.broadcast(SystemEvent::SpectrumData {
                    bands: vec![0.0f32; NUM_BANDS],
                });
            }

            Err(_) => break, // channel closed — application is shutting down

            _ => {}
        }
    }
}

// ── Thread management ─────────────────────────────────────────────────────────

fn start_capture_thread(state: AppStateHandle) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    std::thread::Builder::new()
        .name("spectrum-capture".into())
        .spawn(move || capture_and_analyze(state, stop_clone))
        .expect("failed to spawn spectrum capture thread");

    stop
}

// ── Blocking capture + FFT loop ───────────────────────────────────────────────

fn capture_and_analyze(state: AppStateHandle, stop: Arc<AtomicBool>) {
    // Pre-compute Hanning window coefficients (reused across retries)
    let window: Vec<f32> = (0..FFT_SIZE)
        .map(|i| {
            0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos())
        })
        .collect();

    let band_bins = precompute_band_bins(NUM_BANDS);

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let scratch_len = fft.get_inplace_scratch_len();
    let mut scratch: Vec<Complex<f32>> = vec![Complex::new(0.0, 0.0); scratch_len];
    let mut fft_buf: Vec<Complex<f32>> = vec![Complex::new(0.0, 0.0); FFT_SIZE];
    let mut raw = vec![0u8; FFT_SIZE * 4];
    let mut smoothed = vec![0.0f32; NUM_BANDS];

    // Retry loop: if parec fails (e.g. PipeWire not ready yet), wait and try again.
    let mut retry_delay = Duration::from_secs(2);
    for attempt in 0..=5 {
        if stop.load(Ordering::SeqCst) {
            return;
        }

        if attempt > 0 {
            tracing::info!(
                attempt,
                delay_secs = retry_delay.as_secs(),
                "Spectrum: retrying parec after failure"
            );
            std::thread::sleep(retry_delay);
            retry_delay = (retry_delay * 2).min(Duration::from_secs(30));
            if stop.load(Ordering::SeqCst) {
                return;
            }
        }

        let mut child = match spawn_capture_process() {
            Some(c) => c,
            None => {
                tracing::warn!(
                    "Spectrum: no capture tool available \
                     (install pulseaudio-utils or pipewire-audio)"
                );
                return; // No tools installed — no point retrying
            }
        };

        tracing::info!(
            pid = child.id(),
            attempt,
            "Spectrum: capture process started"
        );

        let mut stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                let _ = child.kill();
                continue;
            }
        };
        // Keep stderr so we can log the failure reason when parec exits early.
        let mut stderr_pipe = child.stderr.take();

        // ── FFT capture loop ─────────────────────────────────────────────────
        loop {
            if stop.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }

            if stdout.read_exact(&mut raw).is_err() {
                // parec exited — log its stderr to help diagnose the cause.
                if let Some(ref mut sp) = stderr_pipe {
                    let mut err_msg = String::new();
                    let _ = std::io::Read::read_to_string(sp, &mut err_msg);
                    if !err_msg.is_empty() {
                        tracing::warn!(
                            stderr = %err_msg.trim(),
                            "Spectrum: parec exited — stderr output above may explain why"
                        );
                    }
                }
                break; // Break inner loop → retry outer loop
            }

            if stop.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }

            // Decode f32-LE samples and apply Hanning window
            for (i, chunk) in raw.chunks_exact(4).enumerate() {
                let s = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                fft_buf[i] = Complex::new(s * window[i], 0.0);
            }

            fft.process_with_scratch(&mut fft_buf, &mut scratch);

            let normalization = 2.0 / FFT_SIZE as f32;
            let magnitudes: Vec<f32> = fft_buf[..FFT_SIZE / 2]
                .iter()
                .map(|c| (c.re * c.re + c.im * c.im).sqrt() * normalization)
                .collect();

            let mut bands = vec![0.0f32; NUM_BANDS];
            for (band_idx, &(bin_lo, bin_hi)) in band_bins.iter().enumerate() {
                let peak = magnitudes[bin_lo..bin_hi]
                    .iter()
                    .cloned()
                    .fold(0.0_f32, f32::max);
                let db = 20.0 * peak.max(1e-10).log10();
                bands[band_idx] = ((db.clamp(-80.0, 0.0) + 80.0) / 80.0).max(0.0);
            }

            for (s, b) in smoothed.iter_mut().zip(bands.iter()) {
                if *b > *s {
                    *s = SMOOTH_ALPHA * b + (1.0 - SMOOTH_ALPHA) * *s;
                } else {
                    *s = (SMOOTH_ALPHA * 0.6) * b + (1.0 - SMOOTH_ALPHA * 0.6) * *s;
                }
            }

            state.broadcast(SystemEvent::SpectrumData {
                bands: smoothed.clone(),
            });
        }

        let _ = child.kill();
        let _ = child.wait();
        tracing::warn!(attempt, "Spectrum: capture process stopped unexpectedly");
    }

    tracing::error!(
        "Spectrum: parec failed after all retries. \
         Check that pipewire-pulse is running and XDG_RUNTIME_DIR is set correctly."
    );
}

// ── Helper functions ──────────────────────────────────────────────────────────

/// Pre-compute (bin_low, bin_high_exclusive) for each of the `n` log bands.
fn precompute_band_bins(n: usize) -> Vec<(usize, usize)> {
    let f_max_clamped = F_MAX.min(SAMPLE_RATE / 2.0);
    let log_ratio = (f_max_clamped / F_MIN).ln();
    let bin_width = SAMPLE_RATE / FFT_SIZE as f32; // Hz per bin
    let nyquist_bins = FFT_SIZE / 2;

    (0..n)
        .map(|i| {
            let t_lo = i as f32 / n as f32;
            let t_hi = (i + 1) as f32 / n as f32;
            let f_lo = F_MIN * (t_lo * log_ratio).exp();
            let f_hi = F_MIN * (t_hi * log_ratio).exp();

            let bin_lo = ((f_lo / bin_width) as usize).min(nyquist_bins - 1);
            let bin_hi = ((f_hi / bin_width).ceil() as usize)
                .min(nyquist_bins)
                .max(bin_lo + 1); // always at least one bin

            (bin_lo, bin_hi)
        })
        .collect()
}

/// Try to spawn a raw-PCM capture process.
///
/// On PipeWire/WirePlumber systems `@DEFAULT_MONITOR@` may not resolve to
/// `soundsync-capture.monitor` because WirePlumber manages the default sink
/// independently and can override what `pactl set-default-sink` sets.
/// We therefore probe for `soundsync-capture.monitor` first and use that
/// directly when it exists.  This avoids the @DEFAULT_MONITOR@ resolution
/// entirely and targets the exact device we control.
///
/// Attempts `parec` (pulseaudio-utils) first, then `pw-cat` (pipewire-audio).
/// Returns `None` if neither tool is available.
fn spawn_capture_process() -> Option<Child> {
    // Resolve which monitor to capture from.
    // Prefer the explicit soundsync-capture monitor we create at startup;
    // fall back to @DEFAULT_MONITOR@ for systems where soundsync-capture may
    // not have been registered yet.
    let device = resolve_capture_device();
    tracing::debug!(device = %device, "Spectrum capture device selected");

    // parec: capture raw float32 mono PCM from the resolved monitor.
    let parec = Command::new("parec")
        .args([
            "--raw",
            &format!("--device={}", device),
            "--channels=1",
            "--rate=44100",
            "--format=float32le",
            "--latency-msec=20",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped()) // captured so callers can log failure reason
        .spawn();

    match parec {
        Ok(child) => return Some(child),
        Err(e) => {
            tracing::warn!(error = %e, "Spectrum: parec spawn failed — trying pw-cat fallback");
        }
    }

    // pw-cat fallback: PipeWire native record.
    let pw_target = if device == "soundsync-capture.monitor" {
        "soundsync-capture.monitor".to_string()
    } else {
        "@DEFAULT_SINK@.monitor".to_string()
    };

    let pw_cat = Command::new("pw-cat")
        .args([
            "--record",
            &format!("--target={}", pw_target),
            "--channels=1",
            "--rate=44100",
            "--format=f32",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    pw_cat.ok()
}

/// Determine which PulseAudio/PipeWire source device to capture from.
///
/// Strategy (in priority order):
///
/// 1. **BT source node directly** — when a `bluez_input.*` / `bluez_source.*`
///    source appears in `pactl list short sources` we capture from it directly.
///    This bypasses WirePlumber's sink-routing decisions entirely: audio comes
///    straight off the Bluetooth adapter into parec, so it is always present
///    regardless of whether soundsync-capture or snd_aloop is the WirePlumber
///    default sink.
///
/// 2. **soundsync-capture.monitor** — fallback for when no BT source is
///    visible yet (e.g., the device connected before PipeWire registered its
///    node) or for setups that explicitly route BT through the null sink.
///
/// 3. **@DEFAULT_MONITOR@** — last resort; resolves to whatever WirePlumber
///    considers the default sink monitor at the time parec starts.
fn resolve_capture_device() -> String {
    let sources = std::process::Command::new("pactl")
        .args(["list", "short", "sources"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();

    // Priority 1: live BT source node (direct capture, no routing dependency).
    // `pactl list short sources` columns: <idx> <name> <driver> <spec> <state>
    const BT_PREFIXES: &[&str] = &["bluez_input.", "bluez_source.", "api.bluez5."];
    for line in sources.lines() {
        if let Some(name) = line.split_whitespace().nth(1) {
            if BT_PREFIXES.iter().any(|pfx| name.starts_with(pfx)) {
                tracing::debug!(source = %name, "Spectrum: capturing directly from BT source node");
                return name.to_string();
            }
        }
    }

    // Priority 2: soundsync-capture.monitor (EQ-processed audio when routing works).
    if sources.contains("soundsync-capture.monitor") {
        tracing::debug!("Spectrum: capturing from soundsync-capture.monitor");
        return "soundsync-capture.monitor".to_string();
    }

    tracing::warn!(
        "Spectrum: no BT source node or soundsync-capture.monitor found; \
         falling back to @DEFAULT_MONITOR@"
    );
    "@DEFAULT_MONITOR@".to_string()
}
