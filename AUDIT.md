# SoundSync Codebase Audit

**Date:** 2026-03-13
**Scope:** Full codebase review — all Rust source, JavaScript frontend, configuration, and build system

---

## 1. SECURITY ISSUES

### 1a. `authorize_service` always accepts — no pairing gate
**File:** `src/bluetooth/agent.rs:93-103`

Every other Agent1 method checks `is_pairing_allowed()` before accepting. But `authorize_service` unconditionally returns `Ok(())`. This means any already-paired device can connect any Bluetooth profile (not just A2DP) at any time without the pairing window being open.

**Recommendation:** Add an `is_pairing_allowed()` check, or at minimum validate that the UUID is an A2DP/AVRCP profile before accepting.

### 1b. XSS risk in `updateDeviceCard` via `innerHTML`
**File:** `web/app.js:767-781`

Device addresses are injected into inline `onclick` attributes. While `escapeHtml()` is used, it doesn't escape backticks or backslashes. Use `addEventListener` instead of inline handlers.

### 1c. `std::env::set_var` is unsafe in multi-threaded contexts
**File:** `src/main.rs:62`

Since Rust 1.66, `set_var` is documented as unsound in multi-threaded programs. Pass the format as a parameter to `logging::init()` instead.

### 1d. No MAC address validation on API inputs
**File:** `src/api/routes.rs:241-243`

Connect/disconnect/remove endpoints only check `is_empty()`. Add regex validation for `XX:XX:XX:XX:XX:XX` format.

---

## 2. RELIABILITY / BUG-RISK ISSUES

### 2a. `monitor_events` loop exits after D-Bus stream ends — no reconnection
**File:** `src/bluetooth/manager.rs:420-427`

When both D-Bus signal streams end, the function breaks and returns. Nothing restarts it. If BlueZ restarts, the event monitor dies permanently.

**Recommendation:** Wrap the entire monitor_events body in a retry loop with backoff.

### 2b. `poll_device_properties` re-creates ObjectManagerProxy every 500ms
**File:** `src/bluetooth/manager.rs:441-447`

Creates a new proxy and calls `get_managed_objects()` every tick. Cache the proxy and reuse it, only recreating on error.

### 2c. Broadcast channel lag handling is incomplete
**File:** `src/api/websocket.rs:72-76`

When a WebSocket client lags, the code logs a warning but doesn't resync. Send a fresh `StateSnapshot` to the lagged client.

### 2d. PipeWire main loop never exits
**File:** `src/pipewire/manager.rs:106`

`main_loop.run()` blocks forever with no shutdown mechanism. The filter-chain cleanup code after it is dead code.

**Recommendation:** Use `main_loop.quit()` triggered by a shutdown channel.

### 2e. Spectrum capture thread panics on spawn failure
**File:** `src/pipewire/spectrum.rs:113`

`.expect()` will crash the entire application. Use `.ok()` and log a warning instead.

### 2f. Race condition in scan auto-stop
**File:** `src/bluetooth/manager.rs:587-610`

The 30-second auto-stop timer is not cancelled on manual StopScan or new StartScan. Store the JoinHandle and abort it on subsequent scan commands.

---

## 3. ARCHITECTURAL CONCERNS

### 3a. In-process Equaliser is unused — EQ runs via subprocess
**File:** `src/pipewire/manager.rs:304-324`, `src/dsp/eq.rs`

The `Equaliser` struct has full real-time processing capabilities but they're never used. EQ is applied via `pipewire-filter-chain` subprocess restart, causing audio dropouts.

### 3b. Duplicate `Device1` proxy definitions
**Files:** `src/bluetooth/manager.rs:97-122` and `src/bluetooth/device.rs:11-58`

Two separate `Device1` trait definitions with different method sets. Unify into one.

### 3c. No `max_devices` enforcement
**File:** `src/state/app.rs:209`

`Config::max_devices` exists but is never checked. Multiple devices can connect without limit.

### 3d. Config save is not atomic
**File:** `src/state/app.rs:275`

`std::fs::write` can produce a corrupted config on crash. Use write-to-temp + rename.

---

## 4. PERFORMANCE ISSUES

### 4a. Spectrum data broadcast rate (~21 fps)
**File:** `src/pipewire/spectrum.rs:217-219`

64-float spectrum broadcast on every FFT frame. Consider throttling to 15 fps or using binary WebSocket format.

### 4b. Full `get_managed_objects` call every 500ms
**File:** `src/bluetooth/manager.rs:449`

Fetches entire BlueZ object tree every 500ms. Subscribe to per-device `PropertiesChanged` signals instead.

### 4c. Per-read buffer allocation in audio stream
**File:** `src/api/routes.rs:841, 886`

Each read allocates a fresh 8KB Vec. Use a reusable buffer.

---

## 5. CODE QUALITY ISSUES

### 5a. Blanket `#[allow(dead_code)]` on multiple modules
**Files:** `src/state/app.rs:2`, `src/pipewire/manager.rs:2`, `src/dsp/eq.rs:2`, `src/dsp/biquad.rs:2`

Suppresses useful warnings. Remove and address the actual dead code.

### 5b. `Instant` is not serializable
**File:** `src/state/app.rs:302`

`started_at: Instant` prevents serializing `AppState`. Use `chrono::DateTime<Utc>`.

### 5c. Empty `tests/` directory
No integration tests despite CI claiming to run them.

### 5d. Hardcoded default comparison for CLI args
**File:** `src/main.rs:77-85`

Comparing against defaults means explicit `--port 8080` won't override config. Use clap's `value_source()`.

### 5e. `EqBand::coefficients()` ignores per-band Q
**File:** `src/dsp/eq.rs:75-77`

Always uses fixed `EQ_Q_FACTOR` while `make_band_coeffs()` uses correct per-band Q. Calling `coefficients()` directly gives wrong results for bands 0, 7, 8, 9.

---

## 6. FUTURE-PROOFING CONCERNS

### 6a. No CORS configuration
`tower-http` cors feature is listed but no CORS layer is applied. External clients will be blocked.

### 6b. No rate limiting on API endpoints
Scan/connect/disconnect endpoints are unprotected against abuse.

### 6c. No health check endpoint
No `/health` endpoint for systemd/Docker monitoring.

### 6d. Static files compiled into binary
`include_str!()` requires recompilation for any UI change. Add filesystem fallback for development.

### 6e. No server-side WebSocket keepalive
Silent client disconnects will leak broadcast subscribers indefinitely.

---

## Priority Summary

| Priority | Issue | Location |
|----------|-------|----------|
| **High** | `authorize_service` bypasses pairing gate | `agent.rs:93` |
| **High** | `monitor_events` dies on D-Bus disconnect | `manager.rs:420` |
| **High** | Scan auto-stop timer race condition | `manager.rs:587` |
| **Medium** | WebSocket lag handler doesn't resync | `websocket.rs:72` |
| **Medium** | `max_devices` limit never enforced | `app.rs` / `manager.rs` |
| **Medium** | Duplicate `Device1` proxy definitions | `manager.rs` / `device.rs` |
| **Medium** | No MAC address input validation | `routes.rs` |
| **Medium** | PipeWire thread has no shutdown path | `manager.rs:106` |
| **Low** | ObjectManagerProxy recreated every 500ms | `manager.rs:441` |
| **Low** | Spectrum broadcast rate may be excessive | `spectrum.rs:217` |
| **Low** | Non-atomic config file writes | `app.rs:275` |
| **Low** | Empty test directory | `tests/` |
| **Low** | Hardcoded default comparison for CLI args | `main.rs:77` |

---

## Overall Assessment

The codebase is well-structured and well-documented. The DSP math is correct, the Bluetooth state machine is well-designed, error handling is mostly thorough, and the code is readable. The issues identified are primarily the kind that surface under adverse production conditions (D-Bus disconnections, misbehaving clients, resource exhaustion) rather than fundamental design flaws.
