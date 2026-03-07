# SoundSync — Implementation Plan

**Version:** 1.0
**Date:** 2026-03-07
**Status:** Active

---

## 1. System Architecture Summary

SoundSync is a Bluetooth A2DP sink application that:

1. Presents the host machine as a named Bluetooth audio device
2. Receives A2DP audio via BlueZ and PipeWire
3. Processes audio through a 10-band DSP equaliser
4. Routes processed audio to system output
5. Exposes a web UI for control on the local network

```
┌─────────────────────────────────────────────────────────────────┐
│                        SoundSync                                │
│                                                                 │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐    ┌──────────┐ │
│  │  BlueZ   │───▶│ PipeWire │───▶│ DSP EQ   │───▶│  System  │ │
│  │  A2DP    │    │  Graph   │    │ 10-band  │    │  Output  │ │
│  └──────────┘    └──────────┘    └──────────┘    └──────────┘ │
│        ▲                                                        │
│        │                                                        │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │           Axum Web Server (REST + WebSocket)             │  │
│  └──────────────────────────────────────────────────────────┘  │
│        ▲                                                        │
│        │                                                        │
│  ┌──────────┐                                                   │
│  │ Browser  │                                                   │
│  │  Web UI  │                                                   │
│  └──────────┘                                                   │
└─────────────────────────────────────────────────────────────────┘
```

---

## 2. Service Boundaries

| Component | Responsibility |
|-----------|---------------|
| `bluetooth::Manager` | BlueZ D-Bus interaction, device state machine, Agent1 for auto-pairing |
| `pipewire::Manager` | PipeWire graph monitoring, filter node lifecycle |
| `dsp::Equaliser` | Biquad peaking filter implementation, coefficient calculation |
| `api::Router` | Axum HTTP routes, WebSocket handler |
| `state::AppState` | Shared application state, event broadcasting |

---

## 3. PipeWire Integration Approach

The PipeWire integration uses the `pipewire` Rust crate (pipewire-rs) to:

1. **Monitor** the PipeWire graph for `bluez_source.*` nodes via the Registry API
2. **Create** a filter node implementing 10-band biquad EQ processing
3. **Link** the Bluetooth source → EQ filter → system sink

### Audio Pipeline

```
bluez_source.XX_XX_XX (PipeWire node)
    │
    ▼
SoundSync EQ Filter (pw::filter::Filter)
    │   - 10 biquad peaking filters per channel
    │   - Float32 sample processing
    │   - Real-time coefficient updates
    ▼
alsa_output.* or default sink
```

### Filter Implementation

Each EQ band uses a peaking biquad filter with:
- Frequency: 60Hz, 120Hz, 250Hz, 500Hz, 1kHz, 2kHz, 4kHz, 8kHz, 12kHz, 16kHz
- Gain range: -12dB to +12dB
- Q factor: 1.41 (√2 for graphic EQ)
- Processing: Float32 stereo

---

## 4. Bluetooth Stack Implementation

### State Machine

```
DISCONNECTED
    │ (scan discovers device)
    ▼
DISCOVERED
    │ (user initiates connect / auto-pair)
    ▼
PAIRING
    │ (BlueZ Agent1 accepts pairing)
    ▼
PAIRED
    │ (device reconnects)
    ▼
CONNECTED
    │ (A2DP UUID in device UUIDs)
    ▼
PROFILE_NEGOTIATED
    │ (PipeWire creates bluez_source node)
    ▼
PIPEWIRE_SOURCE_READY
    │ (audio frames detected)
    ▼
AUDIO_ACTIVE
```

### BlueZ D-Bus Interfaces Used

| Interface | Purpose |
|-----------|---------|
| `org.bluez.Adapter1` | Power, scan, name |
| `org.bluez.Device1` | Connect, disconnect, remove, UUIDs |
| `org.bluez.Agent1` | Auto-accept pairing |
| `org.bluez.AgentManager1` | Register our agent |

### Error Recovery

| Failure | Detection | Recovery |
|---------|-----------|---------|
| Adapter freeze | D-Bus timeout | rfkill cycle + bluetooth restart |
| PipeWire crash | pw node missing | Restart PipeWire + WirePlumber |
| Source race | No node after 5s | Retry detection every 500ms |
| AVRCP-only | No A2DP UUID | Log + reject |
| USB adapter removal | InterfacesRemoved signal | Pause + wait |

---

## 5. Web UI Framework

- **Framework:** Vanilla JS + Web Components (no heavy framework)
- **Real-time:** WebSocket (`/ws/status`) for state updates
- **Design:** Apple glassmorphism aesthetic
- **Branding:** SoundSync — Bluetooth Audio (teal/orange/pink palette)
- **Single page:** No scrolling, responsive grid layout

### Color Palette

```css
--bg: #0b0b1a           /* dark navy */
--teal: #1db8c0         /* primary teal */
--orange: #ff8c42       /* accent orange */
--pink: #d946a8         /* accent pink */
--panel: rgba(255,255,255,0.10)  /* glass panel */
--border: rgba(255,255,255,0.15) /* glass border */
--text: #f0f0f0         /* primary text */
--text-dim: #8a8a9a     /* secondary text */
```

---

## 6. Testing Strategy

### Unit Tests

- `dsp::biquad` — coefficient calculation, sample processing
- `dsp::eq` — 10-band gain application
- `dsp::presets` — preset serialization
- `state::AppState` — state transitions

### Integration Tests

- Bluetooth device pairing flow (mock D-Bus)
- PipeWire node detection (mock PipeWire)
- EQ API endpoint — set/get bands
- WebSocket status updates
- Auto-reconnect behavior

### Test Files

```
tests/
  bluetooth_tests.rs  — device lifecycle tests
  eq_tests.rs         — EQ parameter tests
  api_tests.rs        — REST endpoint tests
  state_tests.rs      — state machine tests
```

---

## 7. Deployment Strategy

### Installer Script (`scripts/install.sh`)

1. Check Ubuntu 24.04 LTS
2. Install system packages (bluez, pipewire, wireplumber, ffmpeg)
3. Load snd-aloop kernel module
4. Configure BlueZ for A2DP sink
5. Build Rust binary
6. Install to `~/.local/bin/soundsync`
7. Install systemd user service
8. Enable loginctl linger
9. Configure firewall rules
10. Start service

### Service Configuration

- Runs as **user service** (not root)
- Requires: `DBUS_SESSION_BUS_ADDRESS`, `PULSE_SERVER`
- Config: `~/.config/soundsync/config.toml`
- Presets: `~/.config/soundsync/eq-presets.json`

---

## 8. Performance Targets

| Metric | Target |
|--------|--------|
| Total audio latency | < 150ms |
| DSP processing per quantum | < 2ms |
| Web UI update frequency | 500ms |
| WebSocket message latency | < 10ms |
| Boot to ready | < 5s |

### Latency Budget

| Stage | Budget |
|-------|--------|
| Bluetooth A2DP encoding (source) | ~30ms |
| BlueZ/PipeWire receive buffer | ~20ms |
| DSP EQ processing | < 2ms |
| PipeWire output buffer | ~20ms |
| ALSA output buffer | ~10ms |
| Total | ~82ms (< 150ms target) |

---

## 9. Repository Structure

```
/src
    main.rs                 — entry point, service orchestration
    bluetooth/
        mod.rs
        manager.rs          — BlueZ D-Bus manager
        adapter.rs          — Adapter1 interface
        device.rs           — Device1 interface + state machine
        agent.rs            — Agent1 auto-pairing
        events.rs           — Bluetooth event types
    pipewire/
        mod.rs
        manager.rs          — PipeWire graph monitor + filter lifecycle
    dsp/
        mod.rs
        biquad.rs           — Biquad filter math
        eq.rs               — 10-band EQ
        presets.rs          — Preset profiles
    api/
        mod.rs
        routes.rs           — Axum REST routes
        websocket.rs        — WebSocket handler
    state/
        mod.rs
        app.rs              — AppState + event bus
    logging/
        mod.rs              — Structured logging setup

/web
    index.html              — Single-page app shell
    app.js                  — Application logic + WebSocket
    styles.css              — Apple glass styling
    components/             — Web component definitions

/docs
    implementation_plan.md
    latency_budget.md
    bluetooth_testing_matrix.md
    pipewire_graph_reference.md
    (+ all original docs)

/tests
    bluetooth_tests.rs
    eq_tests.rs
    api_tests.rs

/scripts
    install.sh              — Comprehensive installer

/.github/workflows
    build.yml               — CI/CD Rust build
```
