# Bluetooth Testing Matrix

This document defines the test scenarios for validating Bluetooth A2DP sink
behaviour in SoundSync.

## Test Environments

| Device Type | Test Priority |
|-------------|---------------|
| iPhone (iOS 16+) | High |
| Android smartphone | High |
| MacBook (macOS 13+) | Medium |
| Vinyl player with BT adapter | High |
| Bluetooth cassette adapter | Medium |
| Windows laptop | Medium |

---

## TC-001: Initial Pairing

| Step | Expected Result |
|------|-----------------|
| Enable scanning (Scan button) | Status shows "Scanning", devices appear in list |
| Source device enables BT, selects SoundSync | Device appears in device list within 5s |
| Click Connect on device card | State transitions: Discovered → Pairing → Paired |
| Pairing completes | State transitions to: Connected → Profile Negotiated |
| Source begins playing audio | State transitions to: Audio Active, waveform animates |

Pass criteria: Latency < 150ms from audio send to system speaker output.

---

## TC-002: Auto-Reconnect

| Step | Expected Result |
|------|-----------------|
| Previously paired device is powered off | State → Disconnected |
| Device powered back on within range | Automatic reconnect without user action |
| Reconnect occurs | State → Connected → Profile Negotiated → Audio Active |
| Audio resumes | Waveform animation active, stream status = Active |

Pass criteria: Auto-reconnect within 10 seconds of device power-on.

---

## TC-003: Multiple Connect/Disconnect Cycles

| Step | Expected Result |
|------|-----------------|
| Connect device, stream audio | Audio Active |
| Disconnect via web UI | State → Disconnected, stream stops |
| Reconnect via web UI | Returns to Audio Active |
| Repeat 10 times | No failures, no memory leaks |

Pass criteria: System stable after 10 cycles, no service restart required.

---

## TC-004: EQ Parameter Changes During Streaming

| Step | Expected Result |
|------|-----------------|
| Device connected and streaming | Audio Active |
| Drag EQ slider (60Hz, +6dB) | Gain label updates immediately |
| Wait 300ms for API debounce | API call sent, EQ coefficients updated |
| Change multiple bands rapidly | No audio dropout, no clicks/pops |
| Apply preset (bass_boost) | All bands update, audio character changes |

Pass criteria: EQ changes apply within 500ms, no audio glitches.

---

## TC-005: Adapter Recovery

| Step | Expected Result |
|------|-----------------|
| Disable Bluetooth adapter (rfkill) | Status → Unavailable |
| Re-enable adapter | Service auto-detects and re-initialises |
| Re-enable within 30s | Status → Ready |
| Reconnect previously paired device | Auto-reconnect succeeds |

Pass criteria: Service recovers without restart within 60 seconds.

---

## TC-006: Stream Stability Under Load

| Step | Expected Result |
|------|-----------------|
| Connect device and stream | Audio Active |
| Open multiple browser tabs with Web UI | No service instability |
| Stream audio for 60 minutes continuously | No dropout, consistent latency |
| Check logs for errors | No errors in journalctl |

Pass criteria: Zero dropouts over 60 minutes, CPU < 10% average.

---

## TC-007: Web UI Under Poor Network

| Step | Expected Result |
|------|-----------------|
| Connect to Web UI | Loads within 2 seconds on LAN |
| Disconnect Wi-Fi briefly | WebSocket disconnects gracefully |
| Reconnect Wi-Fi | WebSocket auto-reconnects, state resyncs |
| UI shows current state | Device list and EQ state correct |

Pass criteria: UI recovers within 5 seconds of network restoration.

---

## TC-008: A2DP Profile Validation

| Device | A2DP UUID Expected |
|--------|-------------------|
| iPhone | 0000110a-0000-1000-8000-00805f9b34fb (source) |
| Android | 0000110a-0000-1000-8000-00805f9b34fb (source) |
| BT Cassette adapter | 0000110a-0000-1000-8000-00805f9b34fb (source) |

Pass criteria: Only devices with A2DP UUID advance to Profile Negotiated.

---

## TC-009: EQ Preset Save/Load

| Step | Expected Result |
|------|-----------------|
| Set custom EQ bands | Bands reflect in sliders |
| Save preset with name "My Custom" | Preset appears in dropdown |
| Restart SoundSync service | Preset still available |
| Apply "My Custom" preset | Bands restore to saved values |
| Delete "My Custom" | Preset removed from dropdown |

Pass criteria: Custom presets persist across service restarts.

---

## TC-010: Concurrent Device Handling

| Step | Expected Result |
|------|-----------------|
| Pair two devices | Both appear in device list |
| Connect device A | A → Audio Active |
| Connect device B | B → Connected (A remains active) |
| Disconnect device A | B → Audio Active (if configured) |

Pass criteria: State machine handles device transitions without corruption.

---

## Automated Test Coverage

Run unit tests:
```bash
cargo test --lib
```

Run integration tests (requires no system D-Bus):
```bash
cargo test
```

Tests cover:
- Biquad filter math (biquad_tests)
- EQ band management (eq_tests)
- Preset serialization (preset_tests)
- Device state machine transitions (state_tests)
- Bluetooth path helpers (bluetooth_tests)
