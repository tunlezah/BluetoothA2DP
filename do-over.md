# SoundSync: Complete Rebuild Specification

> Everything needed to rebuild the Bluetooth A2DP Sink with DSP EQ and Web UI from scratch.
> Based on analysis of 88 commits across 34 PRs from the original implementation.
> Designed for AI agent parallelization — each work unit is independent and well-bounded.

---

## Table of Contents

1. [What Went Wrong: Post-Mortem](#1-what-went-wrong-post-mortem)
2. [System Architecture Overview](#2-system-architecture-overview)
3. [Bluetooth A2DP Subsystem Specification](#3-bluetooth-a2dp-subsystem-specification)
4. [PipeWire Audio Pipeline Specification](#4-pipewire-audio-pipeline-specification)
5. [DSP Equalizer Specification](#5-dsp-equalizer-specification)
6. [WebRTC Audio Streaming Specification](#6-webrtc-audio-streaming-specification)
7. [Web UI & API Specification](#7-web-ui--api-specification)
8. [AVRCP Media Controls Specification](#8-avrcp-media-controls-specification)
9. [Codec Support Specification](#9-codec-support-specification)
10. [Agent Work Units & Parallelization](#10-agent-work-units--parallelization)
11. [Implementation Priorities](#11-implementation-priorities)
12. [Pitfalls to Avoid](#12-pitfalls-to-avoid)
13. [Competitive Landscape & Resources](#13-competitive-landscape--resources)

---

## 1. What Went Wrong: Post-Mortem

### 1.1 The Numbers

| Metric | Value |
|--------|-------|
| Total PRs | 34 |
| Total Commits | 88 |
| PRs that were pure fixes for previous PRs | ~20 |
| Full reverts | 1 (PR #21 reverted to v1.3.0) |
| Version bumps in 10 days | v1.0 to v1.6.1 |
| Longest fix chain | 9 PRs for one feature (audio spectrum + EQ pipeline) |

Only **14 of 34 PRs** introduced new functionality. The rest were fix-ups.

### 1.2 Root Causes

**1. No Local Testing Before Merge.** The initial implementation was 7,696 lines across 34 files that **did not compile**. Six fix commits were needed just to build. Every subsequent feature had the same pattern: merge, discover CI failure, fix, merge fix.

**2. zbus 4.x API Misunderstood.** The zbus/zvariant 4.x API caused compiler errors across 4 separate PR chains. Proxy builder syntax, `ConnectionBuilder` paths, and lifetime rules were never validated with a proof-of-concept.

**3. PipeWire Audio Routing Had No Design.** The EQ pipeline saga was 9 PRs including a full revert. Problems spanned PipeWire filter-chains, PulseAudio modules, systemd ordering, environment variables, and Bluetooth state — all mixed together.

**4. Browser Compatibility Was an Afterthought.** Safari audio broke three separate times, each requiring a different fix approach.

**5. No Stable Baseline.** Features were stacked on untested foundations. The project went from v1.4.0 to v1.5.7 in 2 days — seven fix releases trying to get audio working.

### 1.3 What Went Right

- The codebase audit (PR #16): Clean single commit addressing 14 findings.
- Line-in source (PRs #33-34): Smooth addition with one formatting fix.
- The design document (PR #1): Clean, well-structured.
- The web UI frontend (Preact + TypeScript) was relatively stable throughout.

### 1.4 Original PR Timeline

| PR | Date | Feature | Fix Commits | Verdict |
|----|------|---------|-------------|---------|
| #1 | Mar 7 | Design document | 0 | Clean |
| #2 | Mar 7 | Initial implementation (34 files) | 0 | Did not compile |
| #3-4 | Mar 7 | Build fixes | 6 | Rough |
| #5-13 | Mar 8-9 | Spectrum visualizer + audio streaming | 13 across 9 PRs | Very rough |
| #14 | Mar 9 | Media responsiveness + AVRCP | 4 | Rough |
| #15 | Mar 9 | Codec quality selector | 2 | Moderate |
| #16-17 | Mar 12-13 | Codebase audit | 1 | Smooth |
| #18-26 | Mar 14-16 | EQ/audio pipeline fix | 14 across 9 PRs (1 revert) | Catastrophic |
| #29-34 | Mar 16-17 | Safari/installer/line-in | 3 across 6 PRs | Mostly smooth |

---

## 2. System Architecture Overview

### 2.1 High-Level Signal Flow

```
┌─────────────────────────────────────────────────────────────────┐
│ BLUETOOTH LAYER                                                  │
│                                                                  │
│  Phone/Tablet ─── A2DP (SBC/AAC/LDAC/aptX) ──→ BlueZ D-Bus    │
│                                                                  │
│  Codecs negotiated via org.bluez.MediaEndpoint1:                │
│    - SBC  (mandatory, UUID 0x0003)                              │
│    - AAC  (optional, UUID 0x0002)                               │
│    - LDAC (optional, vendor-specific)                           │
│    - aptX (optional, vendor-specific)                           │
│    - aptX HD (optional, vendor-specific)                        │
└──────────────────────────┬──────────────────────────────────────┘
                           │ PCM audio via PipeWire/WirePlumber
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│ PIPEWIRE AUDIO GRAPH                                             │
│                                                                  │
│  bluez_input.* (Audio/Source)                                    │
│       │                                                          │
│       ▼                                                          │
│  soundsync-eq (filter-chain: 10-band parametric EQ)             │
│       │                                                          │
│       ├──▶ soundsync-capture (null sink for monitoring)          │
│       │        │                                                 │
│       │        ├──▶ Spectrum Analyzer (FFT 2048, 64 bands)      │
│       │        └──▶ WebRTC Audio Source (Opus 128kbps)          │
│       │                                                          │
│       └──▶ Default System Output (speakers/headphones)          │
└─────────────────────────────────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│ WEB LAYER (Axum + Preact)                                        │
│                                                                  │
│  HTTP REST API (/api/*)          ← Device control, EQ, config   │
│  WebSocket (/ws/status)          ← Real-time state + spectrum   │
│  WebRTC (STUN/ICE + Opus)        ← Live audio to browser       │
│  Static Files (webui/dist/)      ← Preact SPA                  │
└─────────────────────────────────────────────────────────────────┘
```

### 2.2 Event-Driven Architecture

All components communicate via a central event bus using `tokio::sync::broadcast`:

```rust
pub enum SystemEvent {
    // Bluetooth events
    BluetoothStatusChanged { status: BluetoothStatus },
    DeviceDiscovered { address: String, name: String, rssi: Option<i16> },
    DeviceStateChanged { address: String, name: String, state: DeviceState },
    DeviceRemoved { address: String },
    StreamStarted { address: String, codec: AudioCodec },
    StreamStopped { address: String },

    // Audio events
    EqChanged { bands: Vec<EqBand>, enabled: bool },
    SpectrumData { bands: Vec<f32> },  // 64 bands, 0.0-1.0

    // AVRCP events
    TrackChanged { track: Option<TrackInfo> },
    PlaybackStatusChanged { status: PlaybackStatus },

    // Line-in events
    LineInActivated,
    LineInDeactivated,

    // System events
    Error { message: String },
    ServiceStopping,
    StateSnapshot { state: AppState },
}
```

**Device State Machine:**
```
Disconnected → Discovered → Pairing → Paired → Connected → ProfileNegotiated → PipewireSourceReady → AudioActive
```

### 2.3 Core State

```rust
pub struct AppState {
    pub bluetooth_status: BluetoothStatus,       // Ready | Scanning | Unavailable | Error(String)
    pub devices: HashMap<String, DeviceInfo>,     // MAC → DeviceInfo
    pub active_device: Option<String>,            // MAC of streaming device
    pub eq_bands: Vec<EqBand>,                    // 10 bands
    pub eq_enabled: bool,
    pub config: Config,
    pub track_info: Option<TrackInfo>,
    pub playback_status: PlaybackStatus,          // Playing | Paused | Stopped | Unknown
    pub line_in_active: bool,
    pub line_in_source: Option<String>,
    pub pipewire_ready: bool,
    pub started_at: Instant,
}

pub struct AppStateHandle {
    pub state: Arc<RwLock<AppState>>,
    pub events: broadcast::Sender<SystemEvent>,
}
```

### 2.4 Dependencies

```toml
[dependencies]
# Bluetooth
bluer = { version = "0.17", features = ["full"] }
zbus = { version = "4", features = ["tokio"] }

# Audio
pipewire = "0.9"

# DSP
biquad = "0.4"
rustfft = "6.2"

# WebRTC
webrtc = "0.12"          # Pure Rust WebRTC stack
opus = "0.3"             # Opus encoding for WebRTC audio

# Web
axum = { version = "0.7", features = ["ws", "macros"] }
tokio = { version = "1", features = ["full"] }
tower-http = { version = "0.5", features = ["cors", "fs", "trace"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"

# Utilities
tracing = "0.1"
tracing-subscriber = "0.3"
clap = { version = "4", features = ["derive"] }
dirs = "5"
chrono = { version = "0.4", features = ["serde"] }
```

### 2.5 Configuration

```toml
# ~/.config/soundsync/config.toml
port = 8080
adapter = "hci0"
device_name = "SoundSync"
auto_pair = true
max_devices = 1
```

Layered loading: `/etc/soundsync/config.toml` → `~/.config/soundsync/config.toml` → `./config.toml` (dev override).

### 2.6 Startup Sequence

1. Load config (or defaults)
2. Initialize logging (tracing-subscriber)
3. Set `XDG_RUNTIME_DIR` if unset (derive from `id -u`)
4. Create null sink via `pactl load-module module-null-sink sink_name=soundsync-capture`
5. Detect line-in via `pactl list short sources` (look for `alsa_input.*`)
6. Spawn concurrent tasks:
   - Bluetooth manager (async)
   - PipeWire graph monitor (blocking thread)
   - Spectrum analyzer (async)
   - AVRCP monitor (async)
   - WebRTC signaling server (async)
7. Start Axum web server
8. Wait for shutdown (Ctrl+C / SIGTERM)

---

## 3. Bluetooth A2DP Subsystem Specification

### 3.1 BlueZ D-Bus Interfaces

| Interface | Service | Path | Purpose |
|-----------|---------|------|---------|
| `org.bluez.Adapter1` | `org.bluez` | `/org/bluez/hci0` | Adapter power, discovery, alias |
| `org.bluez.Device1` | `org.bluez` | `/org/bluez/hci0/dev_XX_XX_XX_XX_XX_XX` | Device connect/disconnect/pair |
| `org.bluez.AgentManager1` | `org.bluez` | `/org/bluez` | Register pairing agent |
| `org.bluez.Agent1` | (our impl) | `/org/soundsync/agent` | Handle pairing requests |
| `org.bluez.Media1` | `org.bluez` | `/org/bluez/hci0` | Register A2DP endpoints |
| `org.bluez.MediaEndpoint1` | (our impl) | `/org/soundsync/a2dp/sbc` etc. | Codec negotiation |
| `org.bluez.MediaPlayer1` | `org.bluez` | `/org/bluez/hci0/dev_.../player0` | AVRCP track/status |
| `org.freedesktop.DBus.ObjectManager` | `org.bluez` | `/` | Device discovery signals |

### 3.2 Profile UUIDs

```
A2DP_SINK_UUID   = "0000110b-0000-1000-8000-00805f9b34fb"
A2DP_SOURCE_UUID = "0000110a-0000-1000-8000-00805f9b34fb"
AVRCP_TARGET     = "0000110c-0000-1000-8000-00805f9b34fb"
AVRCP_CONTROLLER = "0000110e-0000-1000-8000-00805f9b34fb"
```

### 3.3 Device Path Encoding

```rust
// BlueZ encodes MAC addresses in paths with underscores
// /org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF

pub fn address_from_path(path: &str) -> Option<String> {
    path.split('/').next_back()
        .and_then(|last| last.strip_prefix("dev_"))
        .map(|s| s.replace('_', ":"))
}

pub fn path_from_address(adapter_path: &str, address: &str) -> String {
    format!("{}/dev_{}", adapter_path, address.replace(':', "_"))
}
```

### 3.4 Pairing Agent

- Capability: `"NoInputNoOutput"` (headless)
- Auto-accepts all pairing if `auto_pair = true` in config
- `RequestPinCode()` → `"0000"`
- `RequestPasskey()` → `0u32`
- `RequestConfirmation()` → auto-confirm
- `RequestAuthorization()` → auto-authorize

### 3.5 Bluetooth Commands (from Web API)

```rust
pub enum BluetoothCommand {
    StartScan,
    StopScan,
    Connect { address: String },
    Disconnect { address: String },
    Remove { address: String },
    SetName { name: String },
}
```

### 3.6 Device Discovery

- Listen for `InterfacesAdded` signal on `org.freedesktop.DBus.ObjectManager`
- When a new device appears, check its `UUIDs` array for A2DP_SINK_UUID or A2DP_SOURCE_UUID
- Monitor `PropertiesChanged` signals for connection state (500ms poll fallback)
- Auto-transition: when device reaches `Connected` and has A2DP UUID → `ProfileNegotiated`

### 3.7 PipeWire Node Detection for Bluetooth

When BlueZ+WirePlumber create a PipeWire node for A2DP audio:
- Watch PipeWire registry for `ObjectType::Node`
- Match `node.name` against prefixes: `["bluez_input.", "bluez_source.", "api.bluez5."]`
- On match: emit `SystemEvent::StreamStarted`
- Device transitions: `ProfileNegotiated` → `PipewireSourceReady` → `AudioActive`

### 3.8 Key Constants

| Constant | Value |
|----------|-------|
| `AGENT_PATH` | `/org/soundsync/agent` |
| `BLUEZ_NODE_PREFIXES` | `["bluez_input.", "bluez_source.", "api.bluez5."]` |
| `DEVICE_PROPS_POLL` | 500 ms |

---

## 4. PipeWire Audio Pipeline Specification

### 4.1 Audio Graph

```
bluez_input.XX_XX_XX_XX_XX_XX.a2dp_sink  (Audio/Source, created by WirePlumber)
    │
    ▼
effect_input.soundsync-eq  (Audio/Sink, our filter-chain)
    │
    │  [10 × biquad EQ bands — see Section 5]
    │
    ▼
effect_output.soundsync-eq  (Stream/Output/Audio)
    │
    ├──▶ soundsync-capture  (null sink for monitoring/streaming)
    │        │
    │        ├──▶ parec/pw-cat → Spectrum FFT → WebSocket
    │        └──▶ WebRTC audio source → Opus → Browser
    │
    └──▶ System default output (speakers)
```

### 4.2 Null Sink Creation

```bash
pactl load-module module-null-sink \
  sink_name=soundsync-capture \
  sink_properties=device.description=SoundSync-Capture
```

The null sink name is `soundsync-capture`. Its monitor source (`soundsync-capture.monitor`) is used for spectrum analysis and WebRTC audio capture.

### 4.3 Filter-Chain Configuration

Dynamically generated and written to `$XDG_RUNTIME_DIR/soundsync/filter-chain.conf`:

```
# PipeWire filter-chain for 10-band parametric EQ
context.modules = [
    { name = libpipewire-module-filter-chain
        args = {
            node.name = "soundsync-eq"
            node.description = "SoundSync Equalizer"
            capture.props = {
                node.name = "effect_input.soundsync-eq"
                media.class = "Audio/Sink"
                audio.rate = 48000
                audio.channels = 2
                audio.position = "FL,FR"
            }
            playback.props = {
                node.name = "effect_output.soundsync-eq"
                node.target = "soundsync-capture"
            }
            filter.graph = {
                nodes = [
                    { type = builtin  label = bq_lowshelf  name = eq_band_0
                      control = { "Freq" = 60.0  "Q" = 0.707  "Gain" = 0.0 } }
                    { type = builtin  label = bq_peaking  name = eq_band_1
                      control = { "Freq" = 120.0  "Q" = 1.414  "Gain" = 0.0 } }
                    { type = builtin  label = bq_peaking  name = eq_band_2
                      control = { "Freq" = 250.0  "Q" = 1.414  "Gain" = 0.0 } }
                    { type = builtin  label = bq_peaking  name = eq_band_3
                      control = { "Freq" = 500.0  "Q" = 1.414  "Gain" = 0.0 } }
                    { type = builtin  label = bq_peaking  name = eq_band_4
                      control = { "Freq" = 1000.0  "Q" = 1.414  "Gain" = 0.0 } }
                    { type = builtin  label = bq_peaking  name = eq_band_5
                      control = { "Freq" = 2000.0  "Q" = 1.414  "Gain" = 0.0 } }
                    { type = builtin  label = bq_peaking  name = eq_band_6
                      control = { "Freq" = 4000.0  "Q" = 1.414  "Gain" = 0.0 } }
                    { type = builtin  label = bq_peaking  name = eq_band_7
                      control = { "Freq" = 8000.0  "Q" = 1.820  "Gain" = 0.0 } }
                    { type = builtin  label = bq_peaking  name = eq_band_8
                      control = { "Freq" = 12000.0  "Q" = 2.870  "Gain" = 0.0 } }
                    { type = builtin  label = bq_highshelf  name = eq_band_9
                      control = { "Freq" = 16000.0  "Q" = 0.707  "Gain" = 0.0 } }
                ]
                links = [
                    { output = "eq_band_0:Out"  input = "eq_band_1:In" }
                    { output = "eq_band_1:Out"  input = "eq_band_2:In" }
                    { output = "eq_band_2:Out"  input = "eq_band_3:In" }
                    { output = "eq_band_3:Out"  input = "eq_band_4:In" }
                    { output = "eq_band_4:Out"  input = "eq_band_5:In" }
                    { output = "eq_band_5:Out"  input = "eq_band_6:In" }
                    { output = "eq_band_6:Out"  input = "eq_band_7:In" }
                    { output = "eq_band_7:Out"  input = "eq_band_8:In" }
                    { output = "eq_band_8:Out"  input = "eq_band_9:In" }
                ]
            }
        }
    }
]
```

### 4.4 Filter-Chain Process Management

- Spawn: `pipewire-filter-chain --config <path>`
- On EQ update: kill old process, write new config, spawn new process (~200ms audio dropout)
- On shutdown: kill process, cleanup config file

### 4.5 Key Principle

**Let WirePlumber manage the graph.** Do not manually create PipeWire links. Instead:
1. Create named nodes with appropriate `media.class`
2. Set `node.target` properties to direct routing
3. WirePlumber handles the actual link creation

---

## 5. DSP Equalizer Specification

### 5.1 Band Configuration

10-band parametric EQ with fixed center frequencies:

| Band | Frequency | Type | Q Factor | Gain Range |
|------|-----------|------|----------|------------|
| 0 | 60 Hz | Low Shelf | 0.707 | -12 to +12 dB |
| 1 | 120 Hz | Peaking | 1.414 | -12 to +12 dB |
| 2 | 250 Hz | Peaking | 1.414 | -12 to +12 dB |
| 3 | 500 Hz | Peaking | 1.414 | -12 to +12 dB |
| 4 | 1,000 Hz | Peaking | 1.414 | -12 to +12 dB |
| 5 | 2,000 Hz | Peaking | 1.414 | -12 to +12 dB |
| 6 | 4,000 Hz | Peaking | 1.414 | -12 to +12 dB |
| 7 | 8,000 Hz | Peaking | 1.820 | -12 to +12 dB |
| 8 | 12,000 Hz | Peaking | 2.870 | -12 to +12 dB |
| 9 | 16,000 Hz | High Shelf | 0.707 | -12 to +12 dB |

### 5.2 Biquad Filter Math (from Audio EQ Cookbook)

Sample rate: **48,000 Hz** (Bluetooth A2DP standard).

**Transfer function:**
```
H(z) = (b0 + b1·z⁻¹ + b2·z⁻²) / (1 + a1·z⁻¹ + a2·z⁻²)
```

**Per-sample processing (Direct Form II Transposed):**
```
y[n] = b0·x[n] + b1·x[n-1] + b2·x[n-2] - a1·y[n-1] - a2·y[n-2]
```

**Peaking EQ coefficients:**
```
A     = 10^(gain_dB / 40)
w0    = 2π × freq / sample_rate
alpha = sin(w0) / (2 × Q)

b0 = 1 + alpha × A
b1 = -2 × cos(w0)
b2 = 1 - alpha × A
a0 = 1 + alpha / A
a1 = -2 × cos(w0)
a2 = 1 - alpha / A

All coefficients divided by a0.
```

**Low-shelf coefficients** (band 0, 60 Hz):
```
S = 1 (maximum slope)
alpha = sin(w0)/2 × √2
[Standard Audio EQ Cookbook low-shelf formula]
```

**High-shelf coefficients** (band 9, 16 kHz):
```
S = 1 (maximum slope)
alpha = sin(w0)/2 × √2
[Standard Audio EQ Cookbook high-shelf formula]
```

### 5.3 Rust Structs

```rust
pub struct EqBand {
    pub freq: f64,
    pub gain_db: f32,  // Clamped to [-12.0, 12.0]
}

pub struct BiquadCoefficients {
    pub b0: f64, pub b1: f64, pub b2: f64,
    pub a1: f64, pub a2: f64,
}

pub struct BiquadState {
    pub x1: f32, pub x2: f32,  // Input delay line
    pub y1: f32, pub y2: f32,  // Output delay line
}

pub struct StereoBiquad {
    pub left: BiquadState,
    pub right: BiquadState,
    pub coeffs: BiquadCoefficients,
}

pub struct Equalizer {
    pub filters: Mutex<[StereoBiquad; 10]>,
    pub bands: Mutex<Vec<EqBand>>,
    pub enabled: AtomicBool,
    pub sample_rate: f64,  // 48000.0
}
```

### 5.4 Preset Management

Built-in presets: Flat, Bass Boost, Vocal, Classical, Rock, Electronic, Podcast.

Custom presets saved to `~/.config/soundsync/presets/` as individual TOML files:
```toml
# ~/.config/soundsync/presets/my_preset.toml
name = "My Preset"
bands = [0.0, 2.0, 1.0, 0.0, -1.0, 0.0, 2.0, 3.0, 1.0, 0.0]
```

---

## 6. WebRTC Audio Streaming Specification

### 6.1 Why WebRTC (Not WebSocket)

The original implementation used WebSocket + raw audio chunks which caused:
- Safari `AbortError` on audio contexts
- Silent audio from broken ffmpeg/PulseAudio capture strategies
- 3 separate PRs to fix browser compatibility

WebRTC solves these problems:
- **Native browser support** in Chrome, Firefox, Safari, Edge — no polyfills
- **Built-in Opus codec** — efficient, low-latency, universally supported
- **Automatic jitter buffering** and packet loss concealment
- **No MIME type issues** (the Safari WAV MIME type bug is eliminated)
- **ICE/STUN handles NAT traversal** for remote access

### 6.2 Architecture

```
┌─────────────────┐     ┌──────────────────────┐     ┌─────────────┐
│ PipeWire        │     │ SoundSync Server      │     │ Browser     │
│ Monitor Source  │────▶│                       │     │             │
│ (soundsync-    │ PCM │ 1. Capture PCM        │     │             │
│  capture.      │     │ 2. Encode Opus 128k   │     │             │
│  monitor)      │     │ 3. RTP packetize      │────▶│ WebRTC      │
│                │     │ 4. DTLS-SRTP encrypt   │ ICE │ Audio       │
│                │     │                       │◀────│ Player      │
│                │     │ Signaling via WS      │ SDP │             │
└─────────────────┘     └──────────────────────┘     └─────────────┘
```

### 6.3 Signaling Protocol (over existing WebSocket)

The WebSocket at `/ws/status` handles both state events AND WebRTC signaling:

**Client → Server:**
```json
{ "type": "webrtc_offer", "data": { "sdp": "v=0\r\n..." } }
{ "type": "webrtc_ice_candidate", "data": { "candidate": "candidate:...", "sdpMid": "0", "sdpMLineIndex": 0 } }
{ "type": "webrtc_start", "data": {} }
{ "type": "webrtc_stop", "data": {} }
```

**Server → Client:**
```json
{ "type": "webrtc_answer", "data": { "sdp": "v=0\r\n..." } }
{ "type": "webrtc_ice_candidate", "data": { "candidate": "candidate:...", "sdpMid": "0", "sdpMLineIndex": 0 } }
```

### 6.4 Audio Parameters

| Parameter | Value |
|-----------|-------|
| Codec | Opus |
| Bitrate | 128 kbps |
| Sample Rate | 48,000 Hz |
| Channels | 2 (stereo) |
| Frame Size | 20 ms (960 samples) |
| RTP Payload Type | 111 |
| Expected Latency | 100-300 ms |

### 6.5 Implementation Notes

- Use the `webrtc` crate (pure Rust WebRTC stack) — no native libwebrtc dependency
- Audio capture from `soundsync-capture.monitor` via `pw-cat --format f32 --channels 2 --rate 48000`
- Encode to Opus using the `opus` crate
- Each browser client gets its own PeerConnection
- Server is always the offerer (sends audio track, no receive)
- STUN server: use Google's public STUN (`stun:stun.l.google.com:19302`) for local network, or configure a custom TURN server for remote access
- On `StreamStopped` event: send silence frames to prevent WebRTC timeout, or close the peer connection

### 6.6 Browser-Side Implementation

```javascript
// Simplified WebRTC client
const pc = new RTCPeerConnection({
    iceServers: [{ urls: 'stun:stun.l.google.com:19302' }]
});

pc.ontrack = (event) => {
    const audio = document.getElementById('audio-player');
    audio.srcObject = event.streams[0];
    audio.play();
};

// Receive offer from server via WebSocket
ws.onmessage = (msg) => {
    const data = JSON.parse(msg.data);
    if (data.type === 'webrtc_answer') {
        pc.setRemoteDescription(new RTCSessionDescription(data.data));
    } else if (data.type === 'webrtc_ice_candidate') {
        pc.addIceCandidate(new RTCIceCandidate(data.data));
    }
};

// Send offer to server
async function startAudio() {
    pc.addTransceiver('audio', { direction: 'recvonly' });
    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    ws.send(JSON.stringify({ type: 'webrtc_offer', data: { sdp: offer.sdp } }));
}

pc.onicecandidate = (event) => {
    if (event.candidate) {
        ws.send(JSON.stringify({
            type: 'webrtc_ice_candidate',
            data: event.candidate.toJSON()
        }));
    }
};
```

---

## 7. Web UI & API Specification

### 7.1 REST API Endpoints

| Endpoint | Method | Body | Response | Purpose |
|----------|--------|------|----------|---------|
| `/api/status` | GET | - | `{ status, device_count, uptime_secs }` | System health |
| `/api/devices` | GET | - | `[{ address, name, state, rssi, has_a2dp, ... }]` | All known devices |
| `/api/bluetooth/scan` | POST | `{ "scanning": bool }` | `{ "ok": true }` | Start/stop discovery |
| `/api/bluetooth/connect` | POST | `{ "address": "AA:BB:..." }` | `{ "ok": true }` | Connect to device |
| `/api/bluetooth/disconnect` | POST | `{ "address": "AA:BB:..." }` | `{ "ok": true }` | Disconnect device |
| `/api/bluetooth/device` | DELETE | `{ "address": "AA:BB:..." }` | `{ "ok": true }` | Remove paired device |
| `/api/bluetooth/name` | POST | `{ "name": "..." }` | `{ "ok": true }` | Set adapter name |
| `/api/eq` | GET | - | `{ bands: [...], enabled: bool }` | Get EQ state |
| `/api/eq` | POST | `{ bands: [{freq?, gain_db}], enabled? }` | `{ "ok": true }` | Update EQ |
| `/api/eq/presets` | GET | - | `["Flat", "Rock", ...]` | List presets |
| `/api/eq/preset` | POST | `{ "name": "Rock" }` | `{ "ok": true }` | Apply preset |
| `/api/eq/preset/save` | POST | `{ "name": "My EQ" }` | `{ "ok": true }` | Save current as preset |
| `/api/eq/preset/:name` | DELETE | - | `{ "ok": true }` | Delete preset |
| `/api/line-in/status` | GET | - | `{ available, active, source_name }` | Line-in state |
| `/api/line-in/activate` | POST | - | `{ "ok": true }` | Switch to line-in |
| `/api/line-in/deactivate` | POST | - | `{ "ok": true }` | Switch to Bluetooth |
| `/api/avrcp/play` | POST | - | `{ "ok": true }` | AVRCP play |
| `/api/avrcp/pause` | POST | - | `{ "ok": true }` | AVRCP pause |
| `/api/avrcp/next` | POST | - | `{ "ok": true }` | AVRCP next track |
| `/api/avrcp/previous` | POST | - | `{ "ok": true }` | AVRCP previous track |

### 7.2 WebSocket Protocol

**Endpoint:** `/ws/status`

**On connect:** Server sends full state snapshot:
```json
{
  "type": "state_snapshot",
  "data": {
    "status": "ready",
    "devices": [
      {
        "address": "AA:BB:CC:DD:EE:FF",
        "name": "iPhone",
        "state": "audio_active",
        "rssi": -45,
        "trusted": true,
        "has_a2dp": true,
        "codec": "ldac",
        "last_seen": "2026-03-21T10:30:00Z",
        "pipewire_node": "bluez_input.AA_BB_CC_DD_EE_FF.a2dp_sink"
      }
    ],
    "eq": {
      "bands": [
        { "freq": 60.0, "gain_db": 0.0 },
        { "freq": 120.0, "gain_db": 0.0 }
      ],
      "enabled": true
    },
    "active_device": "AA:BB:CC:DD:EE:FF",
    "track_info": {
      "title": "Song Name",
      "artist": "Artist",
      "album": "Album",
      "duration_ms": 180000
    },
    "playback_status": "playing",
    "line_in_active": false,
    "line_in_available": true
  }
}
```

**Real-time events (pushed as they occur):**
```json
{ "type": "device_state_changed", "data": { "address": "...", "name": "...", "state": "audio_active" } }
{ "type": "eq_changed", "data": { "bands": [...], "enabled": true } }
{ "type": "track_changed", "data": { "title": "...", "artist": "...", "album": "...", "duration_ms": 0 } }
{ "type": "playback_status_changed", "data": { "status": "playing" } }
{ "type": "spectrum_data", "data": { "bands": [0.0, 0.15, 0.42, ...] } }
{ "type": "bluetooth_status_changed", "data": { "status": "scanning" } }
```

**Spectrum data:** 64 float values (0.0-1.0), sent at ~60 Hz when audio is active.

**Serialization:** `#[serde(tag = "type", content = "data", rename_all = "snake_case")]`

### 7.3 Frontend Stack

- **Framework:** Preact (lightweight React alternative)
- **Language:** TypeScript
- **Build:** Vite
- **Spectrum Visualizer:** `audioMotion-analyzer` (zero-dependency, 240+ bands, LED/radial modes)
- **Styling:** CSS modules or Tailwind
- **Static serving:** Axum serves `webui/dist/` via `tower-http::services::ServeDir`

### 7.4 Spectrum Analyzer Backend

**Audio capture:**
```bash
parec --raw --format=float32 --channels=1 --rate=44100 --monitor-stream
# or fallback:
pw-cat --format f32 --channels 1 --rate 44100 -r
```

**Processing:**
- FFT Size: 2048 samples
- Sample Rate: 44,100 Hz
- Window: Hanning
- Output: 64 logarithmically-spaced frequency bands (20 Hz – 20 kHz)
- Smoothing: Exponential moving average (α = 0.35)
- Scale: 0.0 = −80 dBFS, 1.0 = 0 dBFS

---

## 8. AVRCP Media Controls Specification

### 8.1 D-Bus Interface

```rust
#[proxy(interface = "org.bluez.MediaPlayer1", default_service = "org.bluez")]
trait MediaPlayer1 {
    fn play(&self) -> zbus::Result<()>;
    fn pause(&self) -> zbus::Result<()>;
    fn stop(&self) -> zbus::Result<()>;
    fn next(&self) -> zbus::Result<()>;
    fn previous(&self) -> zbus::Result<()>;

    #[zbus(property)]
    fn status(&self) -> zbus::Result<String>;
    // "playing" | "stopped" | "paused" | "forward-seek" | "reverse-seek"

    #[zbus(property)]
    fn track(&self) -> zbus::Result<HashMap<String, OwnedValue>>;
}
```

### 8.2 Track Metadata Keys

| Key | Type | Description |
|-----|------|-------------|
| `Title` | String | Track title |
| `Artist` | String | Artist name |
| `Album` | String | Album name |
| `Duration` | u32 | Duration in milliseconds |
| `TrackNumber` | u32 | Track number |
| `NumberOfTracks` | u32 | Total tracks |

### 8.3 Polling Strategy

- Active device connected: **250 ms** poll interval
- Idle (no device): **2,000 ms** poll interval
- Cache the MediaPlayer1 proxy per device (avoid per-poll D-Bus overhead)
- AVRCP player path: `/org/bluez/hci0/dev_XX_XX_XX_XX_XX_XX/player0`

---

## 9. Codec Support Specification

### 9.1 Bluetooth Audio Codecs (Tier 1 — Ship at Launch)

| Codec | UUID/ID | Max Bitrate | Latency | Priority |
|-------|---------|-------------|---------|----------|
| **SBC** | 0x0003 | 345 kbps | ~150ms | Mandatory (fallback) |
| **AAC** | 0x0002 | 256 kbps | ~100ms | High (iOS default) |
| **LDAC** | Vendor (Sony) | 990 kbps | ~200ms | High (Android HQ) |
| **aptX** | Vendor (Qualcomm) | 352 kbps | ~40ms | Medium (low latency) |
| **aptX HD** | Vendor (Qualcomm) | 576 kbps | ~80ms | Medium (HQ + low latency) |

### 9.2 Codec Negotiation via BlueZ

Register separate `MediaEndpoint1` for each supported codec:

```
/org/soundsync/a2dp/sbc       → SBC endpoint
/org/soundsync/a2dp/aac       → AAC endpoint
/org/soundsync/a2dp/ldac      → LDAC endpoint
/org/soundsync/a2dp/aptx      → aptX endpoint
/org/soundsync/a2dp/aptx_hd   → aptX HD endpoint
```

Each endpoint registers via `org.bluez.Media1.RegisterEndpoint()` with:
- `UUID`: A2DP Sink UUID
- `Codec`: Codec identifier byte
- `Capabilities`: Codec-specific capability blob (sample rates, channel modes, bitrates)

BlueZ handles capability exchange with the remote device and selects the best mutually-supported codec. The selected codec is reported via the `MediaTransport1` interface.

### 9.3 Codec Libraries

| Codec | Rust Crate / Library | Notes |
|-------|---------------------|-------|
| SBC | `libsbc` (C FFI) or BlueZ built-in | BlueZ handles SBC decoding internally |
| AAC | `fdk-aac` (C FFI) via `fdk-aac-sys` | Or use PipeWire's built-in AAC support |
| LDAC | `libldac` (C FFI) | Sony's open-source LDAC encoder/decoder |
| aptX | `libopenaptx` (C FFI) | Open-source aptX implementation |
| aptX HD | `libopenaptx` (C FFI) | Same library, HD profile |

**Note:** In practice, PipeWire+WirePlumber+BlueZ handle codec negotiation and decoding transparently. The application receives decoded PCM via the PipeWire graph. The codec registration is needed so BlueZ advertises support to connecting devices. The actual decoding happens in the PipeWire Bluetooth plugin (`spa-bluez5`).

### 9.4 WebRTC Output Codec

The browser audio stream uses **Opus at 128 kbps** exclusively (see Section 6.4). This is:
- Natively supported by all WebRTC implementations
- Superior quality to MP3/AAC at equivalent bitrate
- Zero transcoding cost (Opus is the WebRTC default audio codec)

---

## 10. Agent Work Units & Parallelization

This section defines independent work units that can be executed by parallel AI agents. Each unit has clear inputs, outputs, and boundaries.

### 10.1 Parallelization Map

```
PHASE 1 (Can all run in parallel — no dependencies between them):
┌─────────────────────┐  ┌──────────────────────┐  ┌─────────────────────┐
│ WU-1: Project       │  │ WU-2: Bluetooth      │  │ WU-3: PipeWire      │
│ Scaffold + CI       │  │ D-Bus Research       │  │ Audio Research       │
│                     │  │                      │  │                     │
│ Output: Cargo.toml, │  │ Output: Working      │  │ Output: Working     │
│ CI yaml, pre-commit │  │ zbus 4.x proxy       │  │ filter-chain config,│
│ hooks, project      │  │ examples, BlueR      │  │ null sink setup,    │
│ structure           │  │ adapter code         │  │ pw-cat capture      │
└─────────────────────┘  └──────────────────────┘  └─────────────────────┘

┌─────────────────────┐  ┌──────────────────────┐  ┌─────────────────────┐
│ WU-4: DSP EQ        │  │ WU-5: WebRTC         │  │ WU-6: Frontend      │
│ Implementation      │  │ Research + Prototype │  │ Scaffold            │
│                     │  │                      │  │                     │
│ Output: biquad.rs,  │  │ Output: Working      │  │ Output: Preact app, │
│ equalizer.rs with   │  │ webrtc crate example │  │ Vite config,        │
│ unit tests          │  │ sending Opus audio   │  │ audioMotion setup,  │
│                     │  │ to browser           │  │ component stubs     │
└─────────────────────┘  └──────────────────────┘  └─────────────────────┘

PHASE 2 (Depends on Phase 1 outputs):
┌─────────────────────┐  ┌──────────────────────┐  ┌─────────────────────┐
│ WU-7: Bluetooth     │  │ WU-8: Audio Pipeline │  │ WU-9: Web API       │
│ Manager             │  │ Integration          │  │ + WebSocket         │
│                     │  │                      │  │                     │
│ Deps: WU-1, WU-2   │  │ Deps: WU-1, WU-3,   │  │ Deps: WU-1, WU-6   │
│                     │  │       WU-4           │  │                     │
│ Output: Full BT     │  │ Output: BT→EQ→       │  │ Output: All REST    │
│ manager with agent, │  │ speakers pipeline,   │  │ endpoints, WS       │
│ discovery, connect  │  │ spectrum analyzer    │  │ protocol, state mgmt│
└─────────────────────┘  └──────────────────────┘  └─────────────────────┘

┌─────────────────────┐  ┌──────────────────────┐
│ WU-10: AVRCP        │  │ WU-11: Codec         │
│ Media Controls      │  │ Registration         │
│                     │  │                      │
│ Deps: WU-7          │  │ Deps: WU-7           │
│                     │  │                      │
│ Output: Track info, │  │ Output: SBC/AAC/LDAC/│
│ play/pause/skip     │  │ aptX endpoints       │
│ from web UI         │  │ registered with BlueZ│
└─────────────────────┘  └──────────────────────┘

PHASE 3 (Integration — depends on Phase 2):
┌─────────────────────┐  ┌──────────────────────┐
│ WU-12: WebRTC       │  │ WU-13: Frontend      │
│ Audio Server        │  │ Integration          │
│                     │  │                      │
│ Deps: WU-5, WU-8,  │  │ Deps: WU-6, WU-9,   │
│       WU-9          │  │       WU-12          │
│                     │  │                      │
│ Output: Full WebRTC │  │ Output: Complete UI  │
│ signaling + audio   │  │ with spectrum, EQ,   │
│ stream to browser   │  │ controls, audio      │
└─────────────────────┘  └──────────────────────┘

PHASE 4 (Polish — depends on Phase 3):
┌─────────────────────┐  ┌──────────────────────┐  ┌─────────────────────┐
│ WU-14: Line-In      │  │ WU-15: Installer     │  │ WU-16: E2E Tests    │
│ Audio Source         │  │ Script               │  │ & Documentation     │
│                     │  │                      │  │                     │
│ Deps: WU-8          │  │ Deps: WU-1           │  │ Deps: All           │
└─────────────────────┘  └──────────────────────┘  └─────────────────────┘
```

### 10.2 Work Unit Details

#### WU-1: Project Scaffold + CI

**Input:** This document (sections 2.4, 2.5)
**Output:** Complete project skeleton that builds and passes CI

- Create `Cargo.toml` with all dependencies from section 2.4
- Create module structure: `src/{bluetooth,pipewire,dsp,web,audio,state}/mod.rs`
- Create `.github/workflows/ci.yml`:
  ```yaml
  name: CI
  on: [push, pull_request]
  jobs:
    check:
      runs-on: ubuntu-latest
      steps:
        - uses: actions/checkout@v4
        - uses: dtolnay/rust-toolchain@stable
          with: { components: clippy, rustfmt }
        - run: sudo apt-get update && sudo apt-get install -y libdbus-1-dev libpipewire-0.3-dev libspa-0.2-dev libclang-dev
        - run: cargo fmt --check
        - run: cargo clippy -- -D warnings
        - run: cargo build
        - run: cargo test --test-threads=1
    frontend:
      runs-on: ubuntu-latest
      defaults: { run: { working-directory: webui } }
      steps:
        - uses: actions/checkout@v4
        - uses: actions/setup-node@v4
          with: { node-version: '22' }
        - run: npm ci && npm run lint && npm run build
  ```
- Create pre-commit hook (`.githooks/pre-commit`)
- Create `webui/package.json` with Preact + Vite + TypeScript
- All placeholder modules should compile with `cargo build`

#### WU-2: Bluetooth D-Bus Research

**Input:** This document (section 3)
**Output:** Validated zbus 4.x proxy code that compiles

- Create and validate `Adapter1Proxy` with zbus 4.x syntax
- Create and validate `Device1Proxy` with all properties
- Create and validate `ObjectManagerProxy` for discovery
- Create `Agent1` interface implementation
- Write unit tests that mock D-Bus responses
- **Critical:** Validate proxy builder patterns compile before writing application logic

#### WU-3: PipeWire Audio Research

**Input:** This document (section 4)
**Output:** Working filter-chain config, null sink setup, audio capture

- Validate PipeWire filter-chain config format (section 4.3)
- Write and test null sink creation script
- Validate `parec`/`pw-cat` capture from monitor source
- Test filter-chain process spawn/kill lifecycle
- Document any PipeWire version-specific behaviors

#### WU-4: DSP EQ Implementation

**Input:** This document (section 5)
**Output:** `dsp/biquad.rs` + `dsp/equalizer.rs` with comprehensive unit tests

- Implement biquad coefficient calculation (peaking, low-shelf, high-shelf)
- Implement stereo biquad filter processing
- Implement 10-band equalizer
- Write unit tests:
  - Known-signal frequency response validation
  - Coefficient calculation against reference values
  - Gain range clamping
  - Enable/disable bypass

#### WU-5: WebRTC Research + Prototype

**Input:** This document (section 6)
**Output:** Minimal working example of Rust WebRTC sending Opus audio to browser

- Evaluate `webrtc` crate API (PeerConnection, Track, ICE)
- Build minimal server: create PeerConnection, add audio track, send Opus
- Build minimal HTML page: receive audio, play through `<audio>` element
- Test in Chrome, Firefox, Safari
- Document any browser-specific quirks

#### WU-6: Frontend Scaffold

**Input:** This document (sections 7.3, 7.2)
**Output:** Preact app with component stubs, WebSocket hook, audioMotion setup

- Initialize Preact + TypeScript + Vite project in `webui/`
- Create component stubs: DeviceList, EQControls, SpectrumVisualizer, MediaControls, AudioPlayer
- Implement WebSocket connection hook with auto-reconnect
- Integrate `audioMotion-analyzer` for spectrum display
- Implement WebRTC audio player component
- Add responsive layout (mobile-friendly)

#### WU-7: Bluetooth Manager

**Input:** WU-1 (scaffold) + WU-2 (validated proxies)
**Output:** Complete Bluetooth subsystem

- Implement `BluetoothManager` with command channel (section 3.5)
- Implement device discovery via ObjectManager signals
- Implement device state machine (section 2.2)
- Implement pairing agent (section 3.4)
- Implement PipeWire node detection for BT audio (section 3.7)
- Wire events to `SystemEvent` broadcast channel

#### WU-8: Audio Pipeline Integration

**Input:** WU-1 (scaffold) + WU-3 (PipeWire research) + WU-4 (DSP EQ)
**Output:** Working audio pipeline: BT → EQ → speakers + spectrum

- Integrate filter-chain management (spawn/kill on EQ change)
- Implement spectrum analyzer (FFT capture → 64 bands → broadcast)
- Wire `StreamStarted`/`StreamStopped` events to pipeline lifecycle
- Test end-to-end: audio in → EQ → spectrum data out

#### WU-9: Web API + WebSocket

**Input:** WU-1 (scaffold) + WU-6 (frontend scaffold)
**Output:** All REST endpoints + WebSocket server

- Implement all REST routes (section 7.1)
- Implement WebSocket with state snapshot on connect (section 7.2)
- Implement event broadcasting to all connected clients
- Wire `BluetoothCommand` channel from API to BT manager
- Implement static file serving for frontend
- Add CORS middleware

#### WU-10: AVRCP Media Controls

**Input:** WU-7 (Bluetooth manager)
**Output:** Track info + playback controls

- Implement `MediaPlayer1Proxy` (section 8.1)
- Implement polling with adaptive interval (section 8.3)
- Wire play/pause/next/previous to REST API
- Emit `TrackChanged` and `PlaybackStatusChanged` events

#### WU-11: Codec Registration

**Input:** WU-7 (Bluetooth manager)
**Output:** All codec endpoints registered with BlueZ

- Register SBC endpoint (mandatory)
- Register AAC endpoint
- Register LDAC endpoint (vendor-specific capabilities)
- Register aptX/aptX HD endpoints (vendor-specific capabilities)
- Verify codec selection via `MediaTransport1` properties
- Document which codecs require additional system libraries

#### WU-12: WebRTC Audio Server

**Input:** WU-5 (WebRTC prototype) + WU-8 (audio pipeline) + WU-9 (WebSocket)
**Output:** Full WebRTC audio streaming

- Integrate WebRTC signaling into existing WebSocket handler
- Capture from `soundsync-capture.monitor` → Opus encode → RTP
- Handle multiple concurrent browser clients
- Handle stream start/stop lifecycle
- Test in Chrome, Firefox, Safari

#### WU-13: Frontend Integration

**Input:** WU-6 (frontend scaffold) + WU-9 (API) + WU-12 (WebRTC)
**Output:** Complete, polished web UI

- Wire all components to real API/WebSocket
- Implement device list with connect/disconnect/remove
- Implement interactive EQ with preset selector
- Implement spectrum visualizer with audioMotion-analyzer
- Implement media controls (play/pause/skip, track info display)
- Implement WebRTC audio player with play/stop button
- Test responsive layout on mobile

#### WU-14: Line-In Audio Source

**Input:** WU-8 (audio pipeline)
**Output:** Line-in as selectable audio source

- Detect `alsa_input.*` via `pactl list short sources`
- Switch PipeWire routing between BT and line-in
- Auto-disconnect BT when line-in activated
- API endpoints for activate/deactivate

#### WU-15: Installer Script

**Input:** WU-1 (scaffold)
**Output:** `install.sh` that configures a fresh Linux system

- Install system dependencies (PipeWire, BlueZ, build tools)
- Configure `/etc/bluetooth/main.conf` (device class `0x24043C`, discoverable)
- Build from source (or use pre-built binary)
- Create systemd service
- Detect and warn about conflicting BT agents

#### WU-16: E2E Tests & Documentation

**Input:** All previous WUs
**Output:** Integration tests + README

- Write integration tests for API endpoints
- Write integration tests for WebSocket protocol
- Write integration tests for EQ pipeline (signal → FFT → spectrum)
- Update README with setup, build, and usage instructions

---

## 11. Implementation Priorities

### Tier 1: Core Product (Must ship together)

1. Project scaffold + CI (WU-1)
2. Bluetooth A2DP sink with SBC/AAC/LDAC/aptX codec support (WU-7, WU-11)
3. PipeWire audio output with 10-band parametric EQ (WU-8, WU-4)
4. AVRCP media controls — play/pause/skip/track info (WU-10)
5. Web UI with device management, EQ controls, spectrum analyzer (WU-9, WU-13)
6. WebRTC audio streaming to browser with Opus (WU-12)
7. Preset management (built-in + custom EQ presets)

### Tier 2: Enhanced Features (Post-launch)

8. Snapcast integration (write PCM to `/tmp/snapfifo` for multi-room audio)
9. LUFS metering and True Peak display
10. Audio recording to FLAC/WAV files
11. Device priority and auto-connect
12. Line-in audio source (WU-14)

### Tier 3: Future Considerations

13. Chromecast / AirPlay output
14. Room correction / auto-EQ via microphone
15. LC3 codec support (Bluetooth LE Audio)
16. Extract `bluetooth-a2dp-sink` as standalone Rust crate

---

## 12. Pitfalls to Avoid

These are the specific mistakes made in the original implementation. Each one wasted multiple PRs.

### 12.1 Build & CI

| Pitfall | What Happened | Prevention |
|---------|---------------|------------|
| Code merged without compiling | PR #2: 7,696 lines that didn't build | `cargo build && cargo test` before every commit |
| `--test-thread` typo | CI used `--test-thread` (singular) | Copy-paste from Rust docs: `--test-threads=1` |
| CI only builds on main | Feature branch PRs had no CI | `on: [push, pull_request]` in CI config |
| clang stdbool.h error | Installer failed on some systems | Add `libclang-dev` to CI dependencies |
| rustfmt violations merged | Multiple fix PRs just for formatting | `cargo fmt --check` in pre-commit hook |

### 12.2 zbus 4.x

| Pitfall | What Happened | Prevention |
|---------|---------------|------------|
| Wrong proxy builder syntax | `ConnectionBuilder::session()` vs `connection::Builder::session()` | Build a minimal proof-of-concept first |
| Lifetime errors (E0597, E0506) | AVRCP proxy references outlived their scope | Cache proxies in the struct, don't create per-call |
| `#[dbus_proxy]` vs `#[proxy]` | Used zbus 3.x macro name with 4.x | Read the zbus 4.x migration guide |

### 12.3 PipeWire / PulseAudio

| Pitfall | What Happened | Prevention |
|---------|---------------|------------|
| Invalid `media.class` arg to pactl | `pactl load-module` doesn't accept media.class | Test pactl commands manually first |
| `@DEFAULT_MONITOR@` didn't resolve | PulseAudio default monitor wasn't the BT source | Target `soundsync-capture.monitor` explicitly |
| Filter-chain output went nowhere | EQ output wasn't routed to capture sink | Set `node.target` in filter-chain config |
| `pipewire-pulse` not running | PulseAudio compatibility layer wasn't started | Check and start in startup sequence |
| `XDG_RUNTIME_DIR` unset | PipeWire couldn't find its socket | Set it explicitly from `id -u` |
| snd-aloop conflicts | Kernel loopback module interfered | Detect and warn, don't auto-remove |

### 12.4 Browser Audio

| Pitfall | What Happened | Prevention |
|---------|---------------|------------|
| Safari AbortError | AudioContext creation failed | WebRTC eliminates this entirely |
| Wrong WAV MIME type | Used `audio/wav` instead of `audio/x-wav` | WebRTC uses Opus natively, no MIME issues |
| Chrome pause error | Audio context suspended on tab switch | WebRTC handles this automatically |
| ffmpeg capture produced silence | Wrong PulseAudio source target | WebRTC captures from PipeWire directly |

### 12.5 General

| Pitfall | What Happened | Prevention |
|---------|---------------|------------|
| Full revert needed (PR #21) | Incremental fixes couldn't recover | Validate each layer independently before integrating |
| 9 PRs for one feature | Audio spectrum + streaming | Break into independent work units (this document) |
| Version churn (v1.5.0 to v1.5.7 in 2 days) | Fix → release → discover next bug | Don't release until feature works end-to-end |

---

## 13. Competitive Landscape & Resources

### 13.1 Competitor Comparison

| Project | Language | Codecs | Web UI | EQ | Multi-Room | Maturity |
|---------|----------|--------|--------|-----|------------|----------|
| **SoundSync** | Rust | SBC/AAC/LDAC/aptX | Yes | Yes | No (Snapcast planned) | Rebuild |
| **BlueALSA** | C | SBC/AAC/aptX/LDAC/LC3 | No | No | No | Mature |
| **BT-Speaker** | Python | SBC | No | No | No | Stable |
| **PipeWire built-in** | C | SBC/AAC/aptX/LDAC | No | Via filter-chain | Via Snapcast module | Mature |

### 13.2 SoundSync Differentiators

No other project offers the combination of:
1. Web-based remote control and visualization
2. Built-in DSP equalizer with presets
3. Real-time spectrum analyzer
4. WebRTC audio streaming to browser
5. AVRCP media controls from web UI

### 13.3 Projects to Study

| Project | What to Learn |
|---------|---------------|
| [BlueALSA](https://github.com/arkq/bluez-alsa) | Codec architecture, multi-profile support |
| [BlueR](https://github.com/bluez/bluer) | Clean Rust abstractions for BlueZ |
| [BT-Speaker](https://github.com/lukasjapan/bt-speaker) | Minimal A2DP sink implementation |
| [Snapcast](https://github.com/badaix/snapcast) | Multi-room synchronized audio |
| [audioMotion-analyzer](https://github.com/hvianna/audioMotion-analyzer) | Browser spectrum visualization |
| [biquad crate](https://github.com/korken89/biquad-rs) | Tested Rust biquad filter |
| [FunDSP](https://github.com/SamiPerttu/fundsp) | Graph-based DSP in Rust |
| [spectrum-analyzer](https://github.com/phip1611/spectrum-analyzer) | FFT with windowing in Rust |
| [HydraPlay](https://github.com/mariolukas/HydraPlay) | Audio + web UI integration pattern |

### 13.4 Key Documentation

- [zbus 4.x Documentation](https://docs.rs/zbus/latest/zbus/)
- [PipeWire Filter-Chain Docs](https://docs.pipewire.org/page_module_filter_chain.html)
- [BlueZ D-Bus API](https://git.kernel.org/pub/scm/bluetooth/bluez.git/tree/doc)
- [Audio EQ Cookbook](https://www.w3.org/2011/audio/audio-eq-cookbook.html)
- [WebRTC for Rust (webrtc crate)](https://docs.rs/webrtc/latest/webrtc/)
- [Headless A2DP Setup Guide](https://gist.github.com/mill1000/74c7473ee3b4a5b13f6325e9994ff84c)
- [Streaming Audio with Axum](https://xd009642.github.io/2025/01/20/streaming-audio-APIs-the-axum-server.html)

### 13.5 Future Option: Chromecast / AirPlay Output

For later consideration:
- **Chromecast**: Use `rust-cast` crate or HTTP streaming to Cast receiver
- **AirPlay**: Use `raop` protocol implementation
- Both would complement Snapcast for mixed-ecosystem multi-room audio
