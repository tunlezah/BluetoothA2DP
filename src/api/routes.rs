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
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get, post},
    Router,
};
use futures_util::stream;
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

/// Stream live audio from the PipeWire/PulseAudio default sink to the browser.
///
/// Tries two strategies in order:
///   1. `ffmpeg -f pulse` — reads directly from the PulseAudio default sink.
///   2. Shell pipeline: `parec | ffmpeg` — parec captures raw PCM, ffmpeg encodes.
///
/// Output: Ogg/Opus stream at 128 kbps, delivered as chunked HTTP with
/// `Content-Type: audio/ogg` so browsers can play it via an `<audio>` element.
async fn get_audio_stream() -> impl axum::response::IntoResponse {
    // Strategy 1: ffmpeg reading directly from PulseAudio
    let child = tokio::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "quiet",
            "-f",
            "pulse",
            "-i",
            "default",
            "-acodec",
            "libopus",
            "-b:a",
            "128k",
            "-f",
            "ogg",
            "pipe:1",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();

    // Strategy 2: parec capturing raw PCM piped through ffmpeg for encoding
    let mut child = match child {
        Ok(c) => c,
        Err(_) => {
            match tokio::process::Command::new("sh")
                .args([
                    "-c",
                    "parec --format=s16le --rate=44100 --channels=2 --latency-msec=200 \
                     | ffmpeg -hide_banner -loglevel quiet \
                              -f s16le -ar 44100 -ac 2 -i pipe:0 \
                              -acodec libopus -b:a 128k -f ogg pipe:1",
                ])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Audio stream: failed to spawn encoder: {}", e);
                    return axum::response::Response::builder()
                        .status(StatusCode::SERVICE_UNAVAILABLE)
                        .body(axum::body::Body::empty())
                        .unwrap();
                }
            }
        }
    };

    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let _ = child.kill().await;
            return axum::response::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(axum::body::Body::empty())
                .unwrap();
        }
    };

    tracing::info!(
        "Audio stream: client connected, encoder pid {}",
        child.id().unwrap_or(0)
    );

    // Wrap the child process stdout as an async byte stream.
    // The child is carried in the unfold state so it gets killed when the
    // stream ends (client disconnects or error).
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
        .header("Content-Type", "audio/ogg")
        .header("Cache-Control", "no-cache, no-store")
        .header("X-Content-Type-Options", "nosniff")
        .body(axum::body::Body::from_stream(audio_stream))
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
        .body(axum::body::Body::from(content))
        .unwrap()
}
