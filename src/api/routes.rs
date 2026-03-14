//! REST API routes for SoundSync.
//!
//! Implements the API surface defined in the architecture spec:
//!
//!   GET  /api/status              — overall system status
//!   GET  /api/devices             — list of known Bluetooth devices
//!   POST /api/bluetooth/scan      — start/stop device discovery
//!   POST /api/bluetooth/connect   — connect to a device
//!   POST /api/bluetooth/disconnect — disconnect from a device
//!   DELETE /api/bluetooth/device  — remove a paired device
//!   POST /api/bluetooth/name      — set the Bluetooth speaker name
//!   GET  /api/eq                  — get current EQ settings
//!   POST /api/eq                  — update EQ settings
//!   GET  /api/eq/presets          — list available presets
//!   POST /api/eq/preset           — apply a preset
//!   POST /api/eq/preset/save      — save current EQ as a preset
//!   DELETE /api/eq/preset         — delete a saved preset
//!   /ws/status                    — WebSocket for real-time updates

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Json,
    routing::{delete, get, post},
    Router,
};
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;

use super::websocket::ws_handler;
use crate::bluetooth::manager::BluetoothCommand;
use crate::dsp::{
    eq::{EqBand, Equaliser},
    presets::{EqPreset, PresetManager},
};
use crate::state::{AppStateHandle, DeviceInfo, SystemEvent};

/// Shared API state passed to every route handler.
#[derive(Clone)]
pub struct ApiState {
    pub app: AppStateHandle,
    pub bt_cmd: tokio::sync::mpsc::Sender<BluetoothCommand>,
    pub eq: Arc<Equaliser>,
    pub presets: Arc<Mutex<PresetManager>>,
}

// ── Response types ──────────────────────────────────────────────────────────

#[derive(Serialize)]
struct StatusResponse {
    bluetooth: String,
    pipewire_ready: bool,
    active_device: Option<String>,
    device_count: usize,
    eq_enabled: bool,
    uptime_seconds: u64,
}

#[derive(Serialize)]
struct DevicesResponse {
    devices: Vec<DeviceInfo>,
}

#[derive(Deserialize)]
struct ScanRequest {
    /// true = start scan, false = stop scan
    scanning: bool,
}

#[derive(Deserialize)]
struct ConnectRequest {
    address: String,
}

#[derive(Deserialize)]
struct DisconnectRequest {
    address: String,
}

#[derive(Deserialize)]
struct RemoveRequest {
    address: String,
}

#[derive(Deserialize)]
struct SetNameRequest {
    name: String,
}

#[derive(Serialize, Deserialize)]
struct EqResponse {
    bands: Vec<EqBand>,
    enabled: bool,
}

#[derive(Deserialize)]
struct EqUpdateRequest {
    bands: Vec<EqBandUpdate>,
    enabled: Option<bool>,
}

#[derive(Deserialize)]
struct EqBandUpdate {
    freq: Option<f64>,
    gain_db: f32,
}

#[derive(Deserialize)]
struct ApplyPresetRequest {
    name: String,
}

#[derive(Deserialize)]
struct SavePresetRequest {
    name: String,
}

#[derive(Serialize)]
struct PresetsResponse {
    presets: Vec<String>,
}

#[derive(Deserialize)]
struct StreamQueryParams {
    quality: Option<String>,
    /// `low` for reduced latency (smaller parec buffer, less pre-roll).
    latency: Option<String>,
}

#[derive(Deserialize)]
struct SetQualityRequest {
    quality: String,
}

#[derive(Serialize)]
struct ApiError {
    error: String,
}

impl ApiError {
    fn new(msg: impl Into<String>) -> Json<ApiError> {
        Json(ApiError { error: msg.into() })
    }
}

// ── Router builder ───────────────────────────────────────────────────────────

/// Build the complete Axum router with all API routes and static file serving.
pub fn build_router(
    app: AppStateHandle,
    bt_cmd: tokio::sync::mpsc::Sender<BluetoothCommand>,
    eq: Arc<Equaliser>,
    presets: Arc<Mutex<PresetManager>>,
) -> Router {
    let api_state = ApiState {
        app: app.clone(),
        bt_cmd,
        eq,
        presets,
    };

    Router::new()
        // Status
        .route("/api/status", get(get_status))
        // Device management
        .route("/api/devices", get(get_devices))
        .route("/api/bluetooth/scan", post(post_scan))
        .route("/api/bluetooth/connect", post(post_connect))
        .route("/api/bluetooth/disconnect", post(post_disconnect))
        .route("/api/bluetooth/device", delete(delete_device))
        .route("/api/bluetooth/name", post(post_set_name))
        // EQ
        .route("/api/eq", get(get_eq))
        .route("/api/eq", post(post_eq))
        .route("/api/eq/presets", get(get_presets))
        .route("/api/eq/preset", post(post_apply_preset))
        .route("/api/eq/preset/save", post(post_save_preset))
        .route("/api/eq/preset/:name", delete(delete_preset))
        // Audio stream
        .route("/audio/stream", get(get_audio_stream))
        .route("/api/stream/info", get(get_stream_info))
        .route("/api/stream/qualities", get(get_stream_qualities))
        .route("/api/stream/quality", post(post_stream_quality))
        // WebSocket
        .route("/ws/status", get(ws_handler))
        // Static files — serve web/ directory
        .fallback(get(serve_static))
        .with_state(api_state)
}

// ── Route handlers ───────────────────────────────────────────────────────────

async fn get_status(State(state): State<ApiState>) -> Json<StatusResponse> {
    let s = state.app.state.read().await;
    Json(StatusResponse {
        bluetooth: s.bluetooth_status_str(),
        pipewire_ready: s.pipewire_ready,
        active_device: s.active_device.clone(),
        device_count: s.devices.len(),
        eq_enabled: s.eq_enabled,
        uptime_seconds: s.started_at.elapsed().as_secs(),
    })
}

async fn get_devices(State(state): State<ApiState>) -> Json<DevicesResponse> {
    let s = state.app.state.read().await;
    Json(DevicesResponse {
        devices: s.device_list(),
    })
}

async fn post_scan(
    State(state): State<ApiState>,
    Json(body): Json<ScanRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let cmd = if body.scanning {
        BluetoothCommand::StartScan
    } else {
        BluetoothCommand::StopScan
    };

    state.bt_cmd.send(cmd).await.map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::new("Failed to send command"),
        )
    })?;

    Ok(Json(
        serde_json::json!({ "ok": true, "scanning": body.scanning }),
    ))
}

async fn post_connect(
    State(state): State<ApiState>,
    Json(body): Json<ConnectRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    if body.address.is_empty() {
        return Err((StatusCode::BAD_REQUEST, ApiError::new("address required")));
    }

    state
        .bt_cmd
        .send(BluetoothCommand::Connect {
            address: body.address.clone(),
        })
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiError::new("Failed to send command"),
            )
        })?;

    Ok(Json(
        serde_json::json!({ "ok": true, "address": body.address }),
    ))
}

async fn post_disconnect(
    State(state): State<ApiState>,
    Json(body): Json<DisconnectRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    if body.address.is_empty() {
        return Err((StatusCode::BAD_REQUEST, ApiError::new("address required")));
    }

    state
        .bt_cmd
        .send(BluetoothCommand::Disconnect {
            address: body.address.clone(),
        })
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiError::new("Failed to send command"),
            )
        })?;

    Ok(Json(
        serde_json::json!({ "ok": true, "address": body.address }),
    ))
}

async fn delete_device(
    State(state): State<ApiState>,
    Json(body): Json<RemoveRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    if body.address.is_empty() {
        return Err((StatusCode::BAD_REQUEST, ApiError::new("address required")));
    }

    state
        .bt_cmd
        .send(BluetoothCommand::Remove {
            address: body.address.clone(),
        })
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiError::new("Failed to send command"),
            )
        })?;

    Ok(Json(
        serde_json::json!({ "ok": true, "address": body.address }),
    ))
}

async fn post_set_name(
    State(state): State<ApiState>,
    Json(body): Json<SetNameRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            ApiError::new("name cannot be empty"),
        ));
    }
    if name.len() > 64 {
        return Err((
            StatusCode::BAD_REQUEST,
            ApiError::new("name too long (max 64 chars)"),
        ));
    }

    state
        .bt_cmd
        .send(BluetoothCommand::SetName { name: name.clone() })
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiError::new("Failed to send command"),
            )
        })?;

    Ok(Json(serde_json::json!({ "ok": true, "name": name })))
}

async fn get_eq(State(state): State<ApiState>) -> Json<EqResponse> {
    Json(EqResponse {
        bands: state.eq.get_bands(),
        enabled: state.eq.is_enabled(),
    })
}

async fn post_eq(
    State(state): State<ApiState>,
    Json(body): Json<EqUpdateRequest>,
) -> Result<Json<EqResponse>, (StatusCode, Json<ApiError>)> {
    if body.bands.len() != 10 {
        return Err((
            StatusCode::BAD_REQUEST,
            ApiError::new(format!("Expected 10 bands, got {}", body.bands.len())),
        ));
    }

    // Build EqBand vec from the update request
    let current_bands = state.eq.get_bands();
    let new_bands: Vec<EqBand> = body
        .bands
        .iter()
        .enumerate()
        .map(|(i, update)| {
            let freq = update.freq.unwrap_or(current_bands[i].freq);
            EqBand::new(freq, update.gain_db)
        })
        .collect();

    state.eq.set_bands(&new_bands);

    // Update shared state for WebSocket snapshot
    {
        let mut s = state.app.state.write().await;
        s.eq_bands = state.eq.get_bands();
    }

    // Enable/disable if requested
    if let Some(enabled) = body.enabled {
        state.eq.set_enabled(enabled);
        let mut s = state.app.state.write().await;
        s.eq_enabled = enabled;
    }

    state.app.broadcast(SystemEvent::EqChanged);

    Ok(Json(EqResponse {
        bands: state.eq.get_bands(),
        enabled: state.eq.is_enabled(),
    }))
}

async fn get_presets(State(state): State<ApiState>) -> Json<PresetsResponse> {
    let presets = state.presets.lock().await;
    Json(PresetsResponse {
        presets: presets.list(),
    })
}

async fn post_apply_preset(
    State(state): State<ApiState>,
    Json(body): Json<ApplyPresetRequest>,
) -> Result<Json<EqResponse>, (StatusCode, Json<ApiError>)> {
    let presets = state.presets.lock().await;
    let preset = presets.get(&body.name).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            ApiError::new(format!("Preset '{}' not found", body.name)),
        )
    })?;

    let bands = preset.bands.clone();
    drop(presets);

    state.eq.set_bands(&bands);

    {
        let mut s = state.app.state.write().await;
        s.eq_bands = state.eq.get_bands();
    }

    state.app.broadcast(SystemEvent::EqChanged);
    crate::logging::events::eq_preset_changed(&body.name);

    Ok(Json(EqResponse {
        bands: state.eq.get_bands(),
        enabled: state.eq.is_enabled(),
    }))
}

async fn post_save_preset(
    State(state): State<ApiState>,
    Json(body): Json<SavePresetRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            ApiError::new("preset name cannot be empty"),
        ));
    }

    let current_bands = state.eq.get_bands();
    let preset = EqPreset {
        name: name.clone(),
        bands: current_bands,
    };

    let mut presets = state.presets.lock().await;
    presets.save_preset(preset);

    Ok(Json(serde_json::json!({ "ok": true, "name": name })))
}

async fn delete_preset(
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let mut presets = state.presets.lock().await;
    if presets.delete_preset(&name) {
        Ok(Json(serde_json::json!({ "ok": true, "name": name })))
    } else {
        Err((
            StatusCode::NOT_FOUND,
            ApiError::new(format!("Preset '{}' not found or is a built-in", name)),
        ))
    }
}

/// Return the codec and quality parameters for the currently configured stream quality.
async fn get_stream_info(State(state): State<ApiState>) -> Json<serde_json::Value> {
    let s = state.app.state.read().await;
    let quality = s.config.stream_quality.clone();
    let aac_encoder = s.config.aac_encoder.clone();
    drop(s);

    match quality.as_str() {
        "aac" => Json(serde_json::json!({
            "codec": "aac",
            "bitrate_kbps": 192,
            "sample_rate": 44100,
            "channels": 2,
            "label": format!("AAC 192k ({})", aac_encoder),
            "encoder": aac_encoder,
        })),
        "heaac" => Json(serde_json::json!({
            "codec": "aac",
            "bitrate_kbps": 64,
            "sample_rate": 44100,
            "channels": 2,
            "label": "HE-AAC 64k (libfdk_aac)",
            "encoder": "libfdk_aac",
        })),
        "wav" => Json(serde_json::json!({
            "codec": "pcm",
            "bitrate_kbps": 1411,
            "sample_rate": 44100,
            "channels": 2,
            "label": "Lossless WAV",
        })),
        _ => Json(serde_json::json!({
            "codec": "mp3",
            "bitrate_kbps": 128,
            "sample_rate": 44100,
            "channels": 2,
            "label": "MP3 128k",
        })),
    }
}

/// Return the list of available stream qualities and the currently selected one.
async fn get_stream_qualities(State(state): State<ApiState>) -> Json<serde_json::Value> {
    let s = state.app.state.read().await;
    let current = s.config.stream_quality.clone();
    let aac_encoder = s.config.aac_encoder.clone();
    drop(s);

    let heaac_available = aac_encoder == "libfdk_aac";
    Json(serde_json::json!({
        "qualities": [
            {
                "id": "mp3",
                "label": "MP3 128k",
                "codec": "mp3",
                "bitrate_kbps": 128,
                "description": "Compatible with all browsers"
            },
            {
                "id": "aac",
                "label": "AAC 192k",
                "codec": "aac",
                "bitrate_kbps": 192,
                "encoder": aac_encoder,
                "description": "Higher quality · Safari & Chrome"
            },
            {
                "id": "heaac",
                "label": "HE-AAC 64k",
                "codec": "aac",
                "bitrate_kbps": 64,
                "encoder": "libfdk_aac",
                "available": heaac_available,
                "description": "Efficient · requires libfdk_aac · Chrome & Safari"
            },
            {
                "id": "wav",
                "label": "Lossless WAV",
                "codec": "pcm",
                "bitrate_kbps": 1411,
                "description": "Uncompressed · LAN recommended"
            }
        ],
        "current": current
    }))
}

/// Set the default stream quality and persist it to config.toml.
async fn post_stream_quality(
    State(state): State<ApiState>,
    Json(body): Json<SetQualityRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let quality = body.quality.trim().to_string();
    if !matches!(quality.as_str(), "mp3" | "aac" | "heaac" | "wav") {
        return Err((
            StatusCode::BAD_REQUEST,
            ApiError::new("quality must be one of: mp3, aac, heaac, wav"),
        ));
    }

    {
        let mut s = state.app.state.write().await;
        s.config.stream_quality = quality.clone();
        s.config.save();
    }

    tracing::info!("Stream quality set to: {}", quality);
    Ok(Json(serde_json::json!({ "ok": true, "quality": quality })))
}

/// Stream live audio from the PipeWire/PulseAudio default sink to the browser.
///
/// Quality is selected via the `?quality=` query parameter (mp3 | aac | heaac | wav).
/// If omitted, the server uses the stored config quality.  If the stored quality
/// is also absent, the server performs Accept-header negotiation: it inspects the
/// client's `Accept` header and selects the highest-quality codec the browser
/// advertises (aac > mp3 > wav).  This replaces the need for unreliable
/// client-side `canPlayType` detection.
///
/// All strategies capture from `@DEFAULT_MONITOR@` (the sink monitor) so they
/// match the spectrum analyser capture path.
async fn get_audio_stream(
    State(state): State<ApiState>,
    Query(params): Query<StreamQueryParams>,
    headers: HeaderMap,
) -> impl axum::response::IntoResponse {
    // Resolve quality: per-request param > stored config > Accept-header negotiation.
    let low_latency = params
        .latency
        .as_deref()
        .map(|l| l.eq_ignore_ascii_case("low"))
        .unwrap_or(false);

    let is_safari = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|ua| ua.contains("Safari") && !ua.contains("Chrome") && !ua.contains("Chromium"))
        .unwrap_or(false);

    let (quality, aac_encoder) = {
        let s = state.app.state.read().await;
        let config_quality = s.config.stream_quality.clone();
        let enc = s.config.aac_encoder.clone();
        let q = params
            .quality
            .clone()
            .filter(|q| matches!(q.as_str(), "mp3" | "aac" | "heaac" | "wav"))
            .unwrap_or_else(|| {
                // Fall back to Accept-header negotiation when no explicit quality
                // is stored in config.  Browsers advertise their supported codecs,
                // letting the server pick the best available option.
                if config_quality.is_empty() || config_quality == "auto" {
                    negotiate_quality_from_accept(&headers, &enc)
                } else {
                    config_quality
                }
            });
        (q, enc)
    };

    // parec latency: 20ms for low-latency mode, 50ms normal
    let latency_msec = if low_latency { 20 } else { 50 };

    match quality.as_str() {
        "heaac" => {
            // HE-AAC (AAC with Spectral Band Replication) at 64 kbps.
            // Requires libfdk_aac — produces equivalent perceptual quality to
            // AAC-LC at 192 kbps while halving network bandwidth.  Falls back
            // to standard AAC if libfdk_aac is not available.
            if aac_encoder != "libfdk_aac" {
                tracing::warn!(
                    "Audio stream: HE-AAC requested but libfdk_aac not available \
                     (encoder = {}) — falling back to AAC 192k",
                    aac_encoder
                );
                let cmd = format!(
                    "parec --device=@DEFAULT_MONITOR@ --format=s16le --rate=44100 --channels=2 --latency-msec={} \
                     | ffmpeg -hide_banner -loglevel quiet \
                              -fflags +nobuffer \
                              -f s16le -ar 44100 -ac 2 -i pipe:0 \
                              -acodec {} -b:a 192k -f adts -flush_packets 1 pipe:1",
                    latency_msec, aac_encoder
                );
                if let Some(resp) = try_ffmpeg_stream(cmd, "audio/aac").await {
                    return resp;
                }
                return wav_stream_response(low_latency, is_safari).await;
            }
            let cmd = format!(
                "parec --device=@DEFAULT_MONITOR@ --format=s16le --rate=44100 --channels=2 --latency-msec={} \
                 | ffmpeg -hide_banner -loglevel quiet \
                          -fflags +nobuffer \
                          -f s16le -ar 44100 -ac 2 -i pipe:0 \
                          -acodec libfdk_aac -profile:a aac_he -b:a 64k -f adts -flush_packets 1 pipe:1",
                latency_msec
            );
            if let Some(resp) = try_ffmpeg_stream(cmd, "audio/aac").await {
                tracing::info!("Audio stream: HE-AAC 64k (libfdk_aac) pipeline started");
                return resp;
            }
            tracing::warn!("Audio stream: HE-AAC pipeline failed, falling back to WAV");
            wav_stream_response(low_latency, is_safari).await
        }
        "aac" => {
            // AAC 192 kbps via ADTS container — streamable, no seeking required.
            // Safari and Chrome both decode audio/aac ADTS streams natively.
            let cmd = format!(
                "parec --device=@DEFAULT_MONITOR@ --format=s16le --rate=44100 --channels=2 --latency-msec={} \
                 | ffmpeg -hide_banner -loglevel quiet \
                          -fflags +nobuffer \
                          -f s16le -ar 44100 -ac 2 -i pipe:0 \
                          -acodec {} -b:a 192k -f adts -flush_packets 1 pipe:1",
                latency_msec, aac_encoder
            );
            if let Some(resp) = try_ffmpeg_stream(cmd, "audio/aac").await {
                tracing::info!("Audio stream: AAC 192k ({}) pipeline started", aac_encoder);
                return resp;
            }
            tracing::warn!("Audio stream: AAC ffmpeg pipeline failed, falling back to WAV");
            wav_stream_response(low_latency, is_safari).await
        }
        "wav" => {
            // Lossless PCM — highest quality, fine on LAN (~1.4 Mbps).
            tracing::info!("Audio stream: lossless WAV requested");
            wav_stream_response(low_latency, is_safari).await
        }
        _ => {
            // MP3 128 kbps — default, universally supported.
            let cmd = format!(
                "parec --device=@DEFAULT_MONITOR@ --format=s16le --rate=44100 --channels=2 --latency-msec={} \
                 | ffmpeg -hide_banner -loglevel quiet \
                          -fflags +nobuffer \
                          -f s16le -ar 44100 -ac 2 -i pipe:0 \
                          -acodec libmp3lame -b:a 128k -f mp3 -flush_packets 1 pipe:1",
                latency_msec
            );
            if let Some(resp) = try_ffmpeg_stream(cmd, "audio/mpeg").await {
                tracing::info!("Audio stream: MP3 128k pipeline started");
                return resp;
            }
            tracing::warn!("Audio stream: MP3 ffmpeg pipeline failed, falling back to WAV");
            wav_stream_response(low_latency, is_safari).await
        }
    }
}

/// Attempt to start a `sh -c <cmd>` pipeline and return a streaming response.
/// Returns `None` if the process could not be spawned or produced no stdout.
async fn try_ffmpeg_stream(
    cmd: String,
    content_type: &'static str,
) -> Option<axum::response::Response> {
    let mut child = tokio::process::Command::new("sh")
        .args(["-c", &cmd])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;

    let stdout = child.stdout.take()?;
    Some(compressed_stream_response(stdout, child, content_type))
}

/// Build a streaming WAV response from a fresh `parec` process.
///
/// Uses a streaming WAV header with both RIFF and data chunk sizes set to
/// `0xFFFF_FFFF` so the browser never considers the download complete.
///
/// # Arguments
/// * `low_latency` — when true, use a smaller parec buffer (20 ms) and reduced
///   pre-roll (~50 ms instead of ~250 ms).
/// * `is_safari` — when true, serve as `audio/x-wav` for better Safari compat.
async fn wav_stream_response(low_latency: bool, is_safari: bool) -> axum::response::Response {
    let latency_arg = if low_latency {
        "--latency-msec=20"
    } else {
        "--latency-msec=50"
    };

    let parec = tokio::process::Command::new("parec")
        .args([
            "--device=@DEFAULT_MONITOR@",
            "--format=s16le",
            "--rate=44100",
            "--channels=2",
            latency_arg,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn();

    match parec {
        Ok(mut child) => {
            let mut stdout = match child.stdout.take() {
                Some(s) => s,
                None => {
                    let _ = child.kill().await;
                    return error_response(StatusCode::INTERNAL_SERVER_ERROR);
                }
            };

            tracing::info!(
                low_latency = low_latency,
                safari = is_safari,
                "Audio stream: parec WAV started"
            );

            // Pre-roll buffer: collect PCM before sending the first byte to the
            // browser.  Normal mode: ~250 ms (44 100 bytes) for WiFi resilience.
            // Low-latency mode: ~50 ms (8 820 bytes) to minimise startup delay.
            let pre_roll_bytes: usize = if low_latency { 8_820 } else { 44_100 };
            let pre_roll = {
                let mut buf = vec![0u8; pre_roll_bytes];
                let mut filled = 0usize;
                let _ = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                    while filled < pre_roll_bytes {
                        match stdout.read(&mut buf[filled..]).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => filled += n,
                        }
                    }
                })
                .await;
                buf.truncate(filled);
                buf
            };

            // Minimal 44-byte WAV header for endless streaming.
            // Both RIFF chunk size and data chunk size use the 0xFFFFFFFF sentinel
            // directly — this avoids a u32 overflow that occurred when computing
            // (data_size + 36) and signals "unknown length" to all parsers.
            const CHANNELS: u16 = 2;
            const SAMPLE_RATE: u32 = 44_100;
            const BITS: u16 = 16;
            let byte_rate = SAMPLE_RATE * CHANNELS as u32 * BITS as u32 / 8;
            let block_align = CHANNELS * BITS / 8;

            let mut wav_header = Vec::with_capacity(44);
            wav_header.extend_from_slice(b"RIFF");
            wav_header.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // RIFF chunk size
            wav_header.extend_from_slice(b"WAVE");
            wav_header.extend_from_slice(b"fmt ");
            wav_header.extend_from_slice(&16u32.to_le_bytes());
            wav_header.extend_from_slice(&1u16.to_le_bytes()); // PCM
            wav_header.extend_from_slice(&CHANNELS.to_le_bytes());
            wav_header.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
            wav_header.extend_from_slice(&byte_rate.to_le_bytes());
            wav_header.extend_from_slice(&block_align.to_le_bytes());
            wav_header.extend_from_slice(&BITS.to_le_bytes());
            wav_header.extend_from_slice(b"data");
            wav_header.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // data chunk size

            let header_chunk = Ok::<Vec<u8>, std::io::Error>(wav_header);
            let pre_roll_chunk = Ok::<Vec<u8>, std::io::Error>(pre_roll);
            let pcm_stream = stream::unfold((stdout, child), |(mut reader, mut proc)| async move {
                let mut buf = vec![0u8; 8192];
                match reader.read(&mut buf).await {
                    Ok(0) | Err(_) => {
                        let _ = proc.kill().await;
                        None
                    }
                    Ok(n) => {
                        buf.truncate(n);
                        Some((Ok::<Vec<u8>, std::io::Error>(buf), (reader, proc)))
                    }
                }
            });

            let body_stream = stream::once(async move { header_chunk })
                .chain(stream::once(async move { pre_roll_chunk }))
                .chain(pcm_stream);

            // Safari handles `audio/x-wav` more reliably than `audio/wav`.
            let content_type = if is_safari {
                "audio/x-wav"
            } else {
                "audio/wav"
            };

            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", content_type)
                .header("Cache-Control", "no-cache, no-store")
                .body(axum::body::Body::from_stream(body_stream))
                .unwrap()
        }
        Err(e) => {
            tracing::error!("Audio stream: no capture tool available: {}", e);
            error_response(StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

/// Build a streaming compressed-audio response from an already-spawned child process.
fn compressed_stream_response(
    stdout: tokio::process::ChildStdout,
    child: tokio::process::Child,
    content_type: &'static str,
) -> axum::response::Response {
    let audio_stream = stream::unfold((stdout, child), |(mut reader, mut proc)| async move {
        let mut buf = vec![0u8; 8192];
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => {
                let _ = proc.kill().await;
                None
            }
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<Vec<u8>, std::io::Error>(buf), (reader, proc)))
            }
        }
    });

    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", content_type)
        .header("Cache-Control", "no-cache, no-store")
        .body(axum::body::Body::from_stream(audio_stream))
        .unwrap()
}

/// Select the best quality the browser supports based on its Accept header.
///
/// Browsers typically include `audio/aac`, `audio/mpeg`, etc. in their Accept
/// header when opening an `<audio>` src.  Prefer AAC > MP3 > WAV, matching the
/// quality order.  If `libfdk_aac` is available, HE-AAC is also eligible.
fn negotiate_quality_from_accept(headers: &HeaderMap, aac_encoder: &str) -> String {
    let accept = headers
        .get("accept")
        .or_else(|| headers.get("Accept"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if accept.contains("audio/aac") || accept.contains("audio/mp4") {
        if aac_encoder == "libfdk_aac" && accept.contains("audio/aac") {
            // libfdk_aac available — offer HE-AAC as the best option
            return "heaac".to_string();
        }
        return "aac".to_string();
    }
    if accept.contains("audio/mpeg") || accept.contains("audio/mp3") {
        return "mp3".to_string();
    }
    // Default to MP3 — universal browser support
    "mp3".to_string()
}

fn error_response(status: StatusCode) -> axum::response::Response {
    axum::response::Response::builder()
        .status(status)
        .body(axum::body::Body::empty())
        .unwrap()
}

/// Serve static files from the embedded web assets.
///
/// The web UI files are embedded at compile time using include_str!/include_bytes!
/// so no external file system access is needed at runtime.
async fn serve_static(
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
) -> impl axum::response::IntoResponse {
    let path = uri.path().trim_start_matches('/');

    // Route to the appropriate embedded file
    let (content, content_type) = match path {
        "" | "index.html" => (
            include_str!("../../web/index.html"),
            "text/html; charset=utf-8",
        ),
        "app.js" => (include_str!("../../web/app.js"), "application/javascript"),
        "styles.css" => (include_str!("../../web/styles.css"), "text/css"),
        _ => (
            include_str!("../../web/index.html"),
            "text/html; charset=utf-8",
        ),
    };

    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", content_type)
        .header("Cache-Control", "no-cache, no-store")
        .body(axum::body::Body::from(content))
        .unwrap()
}
