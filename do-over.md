# SoundSync: The Do-Over Guide

> A comprehensive analysis of what went wrong, how to rebuild it better, and what features to add.
> Based on analysis of 88 commits across 34 PRs spanning March 7-17, 2026.

---

## Table of Contents

1. [What Went Wrong: Post-Mortem](#1-what-went-wrong-post-mortem)
2. [The Rebuild Plan: How to Do It Right](#2-the-rebuild-plan-how-to-do-it-right)
3. [Architecture & Technology Recommendations](#3-architecture--technology-recommendations)
4. [Feature Suggestions & Enhancements](#4-feature-suggestions--enhancements)
5. [Competitive Landscape](#5-competitive-landscape)
6. [Implementation Priorities](#6-implementation-priorities)

---

## 1. What Went Wrong: Post-Mortem

### 1.1 The Numbers Tell the Story

| Metric | Value |
|--------|-------|
| Total PRs | 34 |
| Total Commits | 88 |
| PRs that were pure fixes for previous PRs | ~20 |
| Full reverts | 1 (PR #21 reverted to v1.3.0) |
| Version bumps in 10 days | v1.0 to v1.6.1 |
| Longest fix chain | 9 PRs for one feature (audio spectrum + EQ pipeline) |

Only about **14 of 34 PRs** introduced new functionality. The remaining ~20 were fix-ups for code that didn't compile, didn't pass formatting, or didn't work at runtime.

### 1.2 Root Cause #1: No Local Testing Before Merge

**This was the single biggest source of churn.**

The initial implementation (PR #2) was a massive 7,696-line, 34-file commit that **did not compile**. It took PRs #3-4 (6 fix commits) just to get a green CI build. Errors included:

- Wrong SPA crate dependency
- Broken zbus 4.x proxy builder API usage
- zvariant 4.x API incompatibilities
- Clippy `ptr_arg` warnings and dead code
- CI typo (`--test-thread` instead of `--test-threads`)
- Missing clang `stdbool.h` in installer build

**Every single one of these would have been caught by running `cargo build` locally.**

The pattern repeated throughout the project:
- PR #6: Build error from PR #5 (missing import)
- PR #9: Compile error from PR #8 (missing `StreamExt` import)
- PR #14: Four consecutive Rust compiler errors (zbus lifetime issues)
- PR #15: rustfmt violations + test breakage from new Config fields

### 1.3 Root Cause #2: The zbus 4.x API Was Never Properly Understood

The zbus/zvariant 4.x API was a recurring pain point across **4 separate PR chains**:

- **PRs #2-4**: Initial proxy builder syntax wrong
- **PR #14**: `E0597` and `E0506` lifetime errors with AVRCP proxies
- **PR #14 again**: More lifetime fixes after the first attempt failed
- Multiple instances of `ConnectionBuilder::session()` vs `connection::Builder::session()` confusion

**Lesson**: When adopting a major version upgrade of a critical dependency (zbus 3.x → 4.x), read the migration guide thoroughly and build a small proof-of-concept before writing 7,000 lines of code against it.

### 1.4 Root Cause #3: PipeWire Audio Routing Was the Hardest Problem (And Got No Upfront Design)

The EQ/audio pipeline fix saga (PRs #18-26) was the most troubled sequence in the entire project:

| PR | What Happened |
|----|---------------|
| #18 | Remove `-flags +low_delay` from ffmpeg; fix listener leak |
| #19 | Add Cache-Control headers (unrelated tangent) |
| #20 | Validate audio pipeline produces data; fmt fix |
| **#21** | **FULL REVERT to v1.3.0** — gave up on incremental fixes |
| #22 | Remove invalid `media.class` arg from `pactl load-module` |
| #23 | Bypass `@DEFAULT_MONITOR@`, target `soundsync-capture.monitor` directly |
| #24 | Pin filter-chain EQ output to `soundsync-capture` sink |
| #25 | Start pipewire-pulse, fix service ordering, clean snd-aloop |
| #26 | Rebuild from source, fix env vars, retry pactl; fix BT state machine |

**9 PRs. 15 commits. 1 full revert. The audio pipeline was never tested end-to-end before merging.**

The problems spanned multiple layers simultaneously:
- PipeWire filter-chain configuration
- PulseAudio module arguments
- systemd service ordering
- Environment variables (`XDG_RUNTIME_DIR`)
- Bluetooth state machine logic
- Conflicting Bluetooth agent detection

**Lesson**: Audio routing on Linux is complex. It needs a dedicated design document, a test environment, and incremental validation at each layer — not a "merge and see what breaks" approach.

### 1.5 Root Cause #4: Browser Compatibility Was an Afterthought

Safari audio issues appeared in **three separate PRs**, each requiring a different fix:

- **PR #10**: Safari no-audio + Chrome pause error
- **PR #29**: Safari WAV playback fix (wrong MIME type)
- **PR #32**: Web Audio API fallback for Safari WAV (second attempt)

**Lesson**: Test in multiple browsers from day one. Safari's Web Audio API and media type handling differs significantly from Chrome/Firefox.

### 1.6 Root Cause #5: No Clear Separation Between "Ship New Feature" and "Fix Broken Feature"

The version history tells the story:
- v1.0 → v1.4.0: Feature additions (10 days)
- v1.4.0 → v1.5.0: **Full revert** (gave up)
- v1.5.0 → v1.5.7: **Seven fix releases in 2 days** trying to get audio working
- v1.5.8 → v1.6.1: Stabilization and minor features

The project never had a stable baseline that was validated end-to-end. Features were stacked on top of untested foundations.

### 1.7 What Went Right

Not everything was bad:

- **PR #16 (Codebase audit)**: Clean single commit addressing 14 audit findings. One of the best PRs.
- **PRs #33-34 (Line-in source)**: Smooth feature addition with only one formatting fix.
- **The design document (PR #1)**: Clean, well-structured, merged without issues.
- **The web UI**: Despite backend struggles, the frontend (Preact + TypeScript) was relatively stable.

---

## 2. The Rebuild Plan: How to Do It Right

### 2.1 Development Process Rules

**Rule 1: Nothing merges without `cargo build && cargo test && cargo clippy && cargo fmt --check` passing locally.**

Set up a pre-commit hook:
```bash
#!/bin/sh
cargo fmt --check || exit 1
cargo clippy -- -D warnings || exit 1
cargo test || exit 1
```

**Rule 2: One feature per PR. One PR at a time. Validate before moving on.**

The original project stacked features on untested foundations. The rebuild should follow:
1. Write the code
2. Build and test locally
3. Test the actual runtime behavior (not just compilation)
4. Open PR only when it works
5. Move to the next feature only after the previous one is confirmed working

**Rule 3: Audio pipeline changes require a real audio device or a test harness.**

The biggest source of churn was audio routing. Either:
- Test on a real Raspberry Pi / Linux box with Bluetooth hardware
- Build a mock PipeWire environment in Docker for CI
- At minimum, have integration tests that validate the pipeline produces audio data

**Rule 4: Pin and understand your dependencies before writing against them.**

For zbus 4.x specifically:
- Read the [zbus 4.0 migration guide](https://docs.rs/zbus/latest/zbus/)
- Build a minimal D-Bus proxy example first
- Validate the proxy builder pattern works before using it in 10 files

### 2.2 Phased Rebuild Order

The original project tried to ship everything at once (34 files in the first real PR). The rebuild should follow this order:

#### Phase 1: Core Bluetooth A2DP Sink (Week 1)

**Goal**: Accept Bluetooth connections and receive A2DP audio. No web UI, no EQ, no streaming.

Files to build:
- `src/bluetooth/mod.rs` — BlueZ D-Bus adapter management
- `src/bluetooth/agent.rs` — Pairing agent (auto-accept)
- `src/bluetooth/media.rs` — A2DP endpoint registration
- `src/bluetooth/device.rs` — Device connection tracking
- `src/main.rs` — CLI entry point

**Validation**: Pair a phone, play music, confirm audio arrives at the PipeWire graph (use `pw-top` or `pw-dump` to verify).

**Key change from original**: Use the `bluer` crate for high-level adapter/device management instead of raw zbus proxies where possible. Fall back to raw zbus only for A2DP-specific operations that BlueR doesn't support yet.

#### Phase 2: PipeWire Audio Output (Week 1-2)

**Goal**: Route received Bluetooth audio to the system's default audio output.

Files to build:
- `src/pipewire/mod.rs` — PipeWire connection management
- `src/pipewire/manager.rs` — Node/link management
- `src/audio/pipeline.rs` — Audio pipeline (BT source → system sink)

**Validation**: Play music from phone, hear it through speakers. Use `pw-top` to confirm the audio path.

**Key change from original**: Use `wireplumber.rs` for node routing policy instead of direct PipeWire graph manipulation. This prevents conflicts with the system's WirePlumber instance.

#### Phase 3: DSP Equalizer (Week 2)

**Goal**: Insert a parametric EQ between the BT source and system output.

Files to build:
- `src/dsp/mod.rs` — DSP pipeline framework
- `src/dsp/biquad.rs` — Biquad filter implementation
- `src/dsp/equalizer.rs` — Multi-band parametric EQ

**Validation**: Apply EQ presets, confirm audible difference. Write a unit test that processes a known signal through the EQ and validates the frequency response.

**Key change from original**: Consider using the `biquad` crate (well-tested, supports DF1 and DF2T forms) or `FunDSP` for a graph-based DSP approach, rather than a fully custom implementation.

#### Phase 4: Web UI — Basic Controls (Week 2-3)

**Goal**: Serve a web UI that shows connection status and EQ controls.

Files to build:
- `src/web/mod.rs` — Axum web server
- `src/web/api.rs` — REST API endpoints
- `src/web/ws.rs` — WebSocket for real-time updates
- `webui/src/` — Preact frontend (connection status, EQ sliders)

**Validation**: Open browser, see connected device, adjust EQ, hear the change.

#### Phase 5: Spectrum Analyzer (Week 3)

**Goal**: Add real-time audio spectrum visualization to the web UI.

Files to build:
- `src/pipewire/spectrum.rs` — FFT-based spectrum analysis
- Frontend spectrum component

**Validation**: See spectrum move in sync with music.

**Key change from original**: Use `audioMotion-analyzer` on the frontend (zero-dependency, battle-tested, supports 240+ frequency bands, multiple visual modes). Consider using the `spectrum-analyzer` crate on the backend to simplify FFT windowing.

#### Phase 6: Audio Streaming to Browser (Week 3-4)

**Goal**: Stream live audio to the web UI for remote listening.

**Validation**: Hear audio in the browser matching what's playing through speakers. Test in Chrome, Firefox, AND Safari.

**Key change from original**: Send Opus-encoded chunks over WebSocket and decode with Web Audio API's `decodeAudioData()`. This avoids MSE's AAC requirement and works in Safari. Test Safari from day one.

#### Phase 7: Advanced Features (Week 4+)

- Codec quality selector (SBC, AAC, LDAC if possible)
- AVRCP media controls
- Line-in audio source
- Installer script

### 2.3 CI Pipeline (Set Up Before Writing Any Code)

```yaml
# .github/workflows/ci.yml
name: CI
on: [push, pull_request]

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - name: Install system dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y libdbus-1-dev libpipewire-0.3-dev libspa-0.2-dev libclang-dev
      - name: Check formatting
        run: cargo fmt --check
      - name: Clippy
        run: cargo clippy -- -D warnings
      - name: Build
        run: cargo build
      - name: Test
        run: cargo test --test-threads=1

  frontend:
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: webui
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: '22'
      - run: npm ci
      - run: npm run lint
      - run: npm run build
```

---

## 3. Architecture & Technology Recommendations

### 3.1 Dependency Upgrades

| Current | Recommended | Why |
|---------|-------------|-----|
| `pipewire = "0.8"` | `pipewire = "0.9"` | Latest is 0.9.2 with API improvements and fixes |
| Raw `zbus` proxies | `bluer` crate + raw zbus for A2DP | BlueR provides clean adapter/device lifecycle; use raw zbus only for A2DP endpoint registration |
| Direct PipeWire graph manipulation | `wireplumber.rs` | Prevents conflicts with system WirePlumber; uses policy engine for node routing |
| Custom `dsp/biquad.rs` | Consider `biquad` crate or `FunDSP` | Well-tested, supports DF1/DF2T, extensible DSP graph |
| Raw `rustfft` in spectrum.rs | Consider `spectrum-analyzer` crate | Built-in windowing (Hann, Hamming, Flat Top) and frequency-to-magnitude mapping |
| Custom frontend spectrum | `audioMotion-analyzer` | Zero-dependency, 240+ bands, LED/radial modes, A/B/C/D weighting |

### 3.2 Crate Recommendations

```toml
[dependencies]
# Bluetooth
bluer = { version = "0.17", features = ["full"] }  # High-level BlueZ bindings
zbus = { version = "4", features = ["tokio"] }       # For A2DP-specific D-Bus calls

# Audio
pipewire = "0.9"                  # PipeWire bindings
# wireplumber = "0.1"             # When stable enough for production

# DSP
biquad = "0.4"                    # EQ filters
rustfft = "6.2"                   # FFT for spectrum
# spectrum-analyzer = "1.5"       # Optional: simplify spectrum code

# Web
axum = { version = "0.7", features = ["ws"] }
tokio = { version = "1", features = ["full"] }
tower-http = { version = "0.5", features = ["fs", "cors"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Utilities
tracing = "0.1"
tracing-subscriber = "0.3"
clap = { version = "4", features = ["derive"] }
```

### 3.3 Architecture Changes

#### 3.3.1 Separate the Bluetooth Module into a Standalone Crate

There is **no Rust A2DP sink library** in the ecosystem. BlueR covers BLE/GATT but not classic Bluetooth audio profiles. SoundSync's `bluetooth/` module could become a standalone crate (`bluetooth-a2dp-sink`) that others can reuse.

Structure:
```
bluetooth-a2dp-sink/     # Standalone crate
  src/
    lib.rs               # Public API
    adapter.rs           # BlueZ adapter management
    agent.rs             # Pairing agent
    endpoint.rs          # A2DP endpoint registration
    device.rs            # Device tracking
    error.rs             # Error types

soundsync/               # Main application
  Cargo.toml             # depends on bluetooth-a2dp-sink
  src/
    main.rs
    pipewire/
    dsp/
    web/
    audio/
```

#### 3.3.2 Event-Driven Architecture

The original code had tight coupling between Bluetooth state changes and audio pipeline actions. Use a channel-based event system:

```rust
enum AudioEvent {
    DeviceConnected { address: String, name: String },
    DeviceDisconnected { address: String },
    StreamStarted { sample_rate: u32, channels: u16 },
    StreamStopped,
    EqChanged { bands: Vec<f32> },
    VolumeChanged { level: f32 },
}

// Bluetooth module sends events
bt_tx.send(AudioEvent::DeviceConnected { ... }).await;

// Audio pipeline receives and reacts
while let Some(event) = audio_rx.recv().await {
    match event {
        AudioEvent::DeviceConnected { .. } => pipeline.prepare(),
        AudioEvent::StreamStarted { .. } => pipeline.start(),
        // ...
    }
}
```

This decouples Bluetooth from PipeWire and makes each component independently testable.

#### 3.3.3 Configuration Management

Use a layered config approach:
```
/etc/soundsync/config.toml          # System defaults
~/.config/soundsync/config.toml     # User overrides
./config.toml                       # Development overrides
```

With validation at startup (not at runtime when things break):
```rust
#[derive(Deserialize, Validate)]
struct Config {
    #[validate(range(min = 1, max = 65535))]
    web_port: u16,
    #[validate(length(min = 1))]
    device_name: String,
    audio: AudioConfig,
    eq: EqConfig,
}
```

### 3.4 PipeWire Audio Routing: The Right Way

The original project's biggest struggle was PipeWire audio routing. Here's the correct approach:

1. **Create a virtual sink** for SoundSync to own:
   ```
   pw-cli create-node adapter {
     factory.name = support.null-audio-sink
     node.name = "soundsync-sink"
     media.class = "Audio/Sink"
     audio.position = "FL,FR"
     monitor.channel-volumes = true
   }
   ```

2. **Use WirePlumber policy** to route Bluetooth audio to this sink (not manual link manipulation).

3. **Read from the sink's monitor port** for DSP processing.

4. **Write processed audio** to the system's default output.

5. **For the spectrum analyzer**, tap the processed audio stream — don't create a separate capture.

Key principle: **Let WirePlumber manage the graph. SoundSync should declare what it needs, not micromanage node connections.**

### 3.5 Browser Audio Streaming: The Right Way

| Approach | Latency | Safari Support | Complexity |
|----------|---------|---------------|------------|
| WebSocket + Web Audio API (Opus) | 100-500ms | Yes | Medium |
| WebSocket + Web Audio API (PCM) | 50-200ms | Yes | Low |
| WebRTC | 200-500ms | Yes | High |
| MSE + WebSocket | 1-3s | Partial | Medium |

**Recommended**: WebSocket + Web Audio API with Opus encoding. Send Opus frames over WebSocket, decode with `decodeAudioData()` in the browser. This works in all modern browsers including Safari.

**Fallback for Safari edge cases**: Raw PCM over WebSocket with AudioWorklet processing. Higher bandwidth but guaranteed compatibility.

---

## 4. Feature Suggestions & Enhancements

### 4.1 High-Impact Features

#### Multi-Room Audio via Snapcast Integration
- Write decoded audio to a named pipe (`/tmp/snapfifo`) for Snapcast consumption
- PipeWire 1.2+ has native Snapcast streaming support, making this nearly free to implement
- Enables time-synchronized playback across multiple rooms/devices
- Reference: [Snapcast](https://github.com/badaix/snapcast)

#### Extended Codec Support
- **LDAC**: Sony's high-quality codec, common on Android (up to 990 kbps)
- **aptX / aptX HD**: Qualcomm's low-latency codecs
- **LC3**: Bluetooth LE Audio codec (the future standard)
- Study [BlueALSA's codec architecture](https://github.com/arkq/bluez-alsa) for implementation patterns
- Current SoundSync only negotiates SBC; adding LDAC alone would be a major differentiator

#### LUFS Metering and True Peak Display
- Professional loudness measurement (EBU R128 / ITU-R BS.1770)
- True Peak detection for clipping prevention
- Display as real-time meters in the web UI alongside the spectrum analyzer
- Reference: [soundscope crate](https://crates.io/crates/soundscope)

#### Crossfade and Gapless Playback
- Detect track boundaries from AVRCP metadata changes
- Apply configurable crossfade (0-12 seconds)
- Prevents audio pops/clicks during track transitions

### 4.2 Medium-Impact Features

#### Preset Management
- Save/load EQ presets with names
- Include sensible defaults: Flat, Bass Boost, Vocal, Classical, Rock, Electronic, Podcast
- Per-device presets (auto-apply when a known device connects)
- Import/export presets as JSON

#### Audio Recording
- Record Bluetooth audio to FLAC/WAV/MP3 files
- Timestamped filenames with device name
- Configurable output directory and format
- Useful for archiving radio shows, conference calls, etc.

#### Device Priority & Auto-Connect
- Maintain a priority list of known devices
- Auto-reconnect to highest-priority device when it comes in range
- Reject connections from unknown devices (optional security mode)
- Remember per-device volume and EQ settings

#### Headless/Kiosk Mode
- Auto-start on boot (systemd service — already partially implemented)
- Status LED control via GPIO (for Raspberry Pi deployments)
- mDNS/Avahi advertisement so the web UI is discoverable as `soundsync.local`

### 4.3 Nice-to-Have Features

#### Room Correction / Auto-EQ
- Microphone calibration mode: play test tones, measure room response
- Auto-generate corrective EQ curve
- Would require Web Audio API microphone access in the browser

#### Parametric EQ with Visual Frequency Response
- Interactive frequency response curve (drag control points)
- Real-time visualization of the EQ effect on the spectrum
- A/B comparison toggle (bypass EQ)

#### Chromecast / AirPlay Output
- Route processed audio to Chromecast or AirPlay speakers
- Would complement Snapcast integration for mixed ecosystems

#### Voice Enhancement Mode
- Optimize EQ for speech clarity (boost 2-4 kHz, cut low frequencies)
- Auto-detect speech vs. music content
- Useful for podcast/audiobook listening

---

## 5. Competitive Landscape

### 5.1 Direct Competitors

| Project | Language | Codecs | Web UI | EQ | Multi-Room | Maturity |
|---------|----------|--------|--------|-----|------------|----------|
| **SoundSync** | Rust | SBC | Yes | Yes | No | Early |
| **BlueALSA** | C | SBC/AAC/aptX/LDAC/LC3 | No | No | No | Mature |
| **BT-Speaker** | Python | SBC | No | No | No | Stable |
| **PipeWire (built-in)** | C | SBC/AAC/aptX/LDAC | No | Via filter-chain | Via Snapcast module | Mature |

### 5.2 Key Differentiators for SoundSync

SoundSync's unique value proposition is the **combination** of:
1. Web-based remote control and visualization
2. Built-in DSP equalizer
3. Real-time spectrum analyzer
4. Audio streaming to browser

No other project offers all four. To strengthen this position:
- **Add LDAC/aptX**: Removes the biggest functional gap vs. BlueALSA
- **Add Snapcast integration**: Removes the biggest functional gap vs. PipeWire's built-in BT support
- **Keep the web UI excellent**: This is the primary differentiator

### 5.3 Projects to Learn From

- **BlueALSA**: Codec architecture, multi-profile support, separation of daemon vs. player utility
- **Snapcast**: Time-synchronized audio distribution, client grouping, JSON-RPC API
- **audioMotion-analyzer**: Spectrum visualization (use it directly in the frontend)
- **HydraPlay**: How to wrap audio services with an integrated web UI
- **BlueR**: Clean Rust abstractions for BlueZ (adapter enumeration, device lifecycle)

---

## 6. Implementation Priorities

### Tier 1: Must-Have for Rebuild (Before First Release)

1. **Pre-commit hooks and CI** — Never merge broken code again
2. **Core Bluetooth A2DP sink** — Using BlueR where possible
3. **PipeWire audio output** — With proper WirePlumber integration
4. **Basic web UI** — Connection status, volume control
5. **Parametric EQ** — Using proven DSP crates
6. **Spectrum analyzer** — Using audioMotion-analyzer on frontend
7. **Browser audio streaming** — WebSocket + Opus + Web Audio API, tested in Safari from day one

### Tier 2: High Priority (v1.x releases)

8. **Snapcast integration** — Multi-room audio for nearly free
9. **LDAC codec support** — Table stakes for high-quality BT audio
10. **Preset management** — Save/load EQ presets, per-device settings
11. **AVRCP media controls** — Play/pause/skip from the web UI
12. **Device priority & auto-connect** — Essential for headless deployments

### Tier 3: Differentiators (v2.x releases)

13. **LUFS metering and True Peak** — Professional loudness tools
14. **Audio recording** — Record BT audio to files
15. **Room correction** — Auto-EQ via microphone calibration
16. **aptX/LC3 codec support** — Future-proofing
17. **Chromecast/AirPlay output** — Mixed ecosystem support

### Tier 4: Ecosystem (Long-term)

18. **Extract `bluetooth-a2dp-sink` as standalone crate** — Fill the Rust ecosystem gap
19. **WebRTC support** — Ultra-low-latency browser audio
20. **Monitor `pipewire-native` crate** — Pure Rust PipeWire (no C FFI)

---

## Appendix A: The Original PR Timeline

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

## Appendix B: Key Resources

- [BlueR — Official BlueZ Rust Bindings](https://github.com/bluez/bluer)
- [BlueALSA — Mature C A2DP Implementation](https://github.com/arkq/bluez-alsa)
- [BT-Speaker — Minimal Python A2DP Sink](https://github.com/lukasjapan/bt-speaker)
- [pipewire-rs — PipeWire Rust Bindings](https://crates.io/crates/pipewire)
- [wireplumber.rs — WirePlumber Rust Bindings](https://github.com/arcnmx/wireplumber.rs)
- [audioMotion-analyzer — Browser Spectrum Visualizer](https://github.com/hvianna/audioMotion-analyzer)
- [spectrum-analyzer — Rust FFT Wrapper](https://github.com/phip1611/spectrum-analyzer)
- [biquad — Rust Biquad Filter](https://github.com/korken89/biquad-rs)
- [FunDSP — Rust DSP Graph Framework](https://github.com/SamiPerttu/fundsp)
- [Snapcast — Multi-Room Audio](https://github.com/badaix/snapcast)
- [Headless A2DP Setup Guide](https://gist.github.com/mill1000/74c7473ee3b4a5b13f6325e9994ff84c)
- [zbus Documentation](https://docs.rs/zbus/latest/zbus/)
- [Streaming Audio APIs with Axum](https://xd009642.github.io/2025/01/20/streaming-audio-APIs-the-axum-server.html)
