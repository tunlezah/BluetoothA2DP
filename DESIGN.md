# BluetoothA2DP — Application Design

**Version:** 1.0 (Research & Design Phase)
**Status:** Design only — no code written yet
**Target:** Intel-based Linux machine, Safari-first web interface

---

## 1. Executive Summary

This document describes the design for a Bluetooth A2DP sink application that presents the host machine as a named Bluetooth speaker, receives audio from any A2DP-capable source device (iPhone, record player, Bluetooth cassette player, etc.), and streams that audio live to a web browser — with all Bluetooth management (scan, pair, connect, disconnect, rename the speaker) handled through the web UI.

### What previous attempts got wrong

Analysis of prior implementations (`a2dplaya`, `Burger`, and others) reveals three recurring failure modes that this design explicitly solves:

| Failure Mode | Previous Approach | This Design |
|---|---|---|
| A2DP audio never captured | Attempted to implement `MediaTransport1` FD reads in Python — but never completed it | Delegate entirely to PipeWire/WirePlumber; route via ALSA loopback |
| Bluetooth control unreliable | Shell out to `bluetoothctl` (ANSI output, async events, no machine-readable output) | Direct D-Bus via `dbus-python` / `dasbus` — never touch `bluetoothctl` |
| Safari audio broken | Opus/WebM streaming (unsupported in Safari, broken in WebKit) | AAC in fragmented MP4 + HLS fallback; detect codec support client-side |
| System service vs. user session | Ran as system daemon while PipeWire runs as user session | Run as user systemd service with proper `DBUS_SESSION_BUS_ADDRESS` and `PULSE_SERVER` |
| Single-consumer audio streams | One FFmpeg process, one pipe reader — crashes on second connection | Per-client FFmpeg subprocess, lifecycle-managed by the web server |
| PipeWire source race | Started FFmpeg before the BT source appeared | Event-driven: only start stream after `bluez_source` confirmed in PipeWire graph |

---

## 2. High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          HOST MACHINE (Linux)                           │
│                                                                         │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │  LAYER 1 — BLUETOOTH STACK                                        │  │
│  │                                                                   │  │
│  │   bluetoothd (BlueZ 5.70+)                                       │  │
│  │       │  D-Bus (org.bluez)                                       │  │
│  │       ▼                                                           │  │
│  │   WirePlumber (A2DP sink endpoint registration)                  │  │
│  │       │  registers MediaEndpoint1, acquires transport FD         │  │
│  │       ▼                                                           │  │
│  │   PipeWire graph  ──── bluez_source.* (A2DP PCM node)           │  │
│  │       │                                                           │  │
│  │   bt_web_sink (pw-loopback virtual sink)                         │  │
│  │       │  routes BT audio into loopback                           │  │
│  │       ▼                                                           │  │
│  │   ALSA Loopback (snd-aloop)   hw:Loopback,1,0                   │  │
│  └──────────────────────────────┬────────────────────────────────────┘  │
│                                 │ raw PCM                                │
│  ┌──────────────────────────────▼────────────────────────────────────┐  │
│  │  LAYER 2 — APPLICATION SERVER (Python / FastAPI)                  │  │
│  │                                                                   │  │
│  │   bt_agent.py    ← D-Bus agent (pairing, trust, auto-connect)   │  │
│  │   bt_manager.py  ← D-Bus device/adapter control                  │  │
│  │   audio_router.py← pw-loopback lifecycle, source detection       │  │
│  │   stream_manager.py ← per-client FFmpeg subprocess management    │  │
│  │   web_server.py  ← FastAPI + WebSocket endpoints                 │  │
│  └──────────────────────────────┬────────────────────────────────────┘  │
│                                 │ HTTP/WS/WSS                            │
│  ┌──────────────────────────────▼────────────────────────────────────┐  │
│  │  LAYER 3 — WEB BROWSER (Safari, Chrome, Firefox)                  │  │
│  │                                                                   │  │
│  │   index.html     ← BT management UI (scan, connect, rename)      │  │
│  │   audio.js       ← MediaSource or HLS.js playback engine         │  │
│  │   bt.js          ← WebSocket control channel                     │  │
│  └───────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## 3. Component Breakdown

### 3.1 `bt_agent` — Bluetooth Pairing Agent

**Responsibility:** Register a BlueZ pairing agent that auto-accepts incoming connections, trusts devices, and keeps the adapter in a permanently discoverable/pairable state.

**Implementation:** D-Bus service object implementing `org.bluez.Agent1` interface.

**Key operations:**
- `RegisterAgent(path, "NoInputNoOutput")` — no PIN required (headless speaker mode)
- `RequestDefaultAgent(path)` — this agent handles all pairing requests
- `RequestConfirmation(device, passkey)` — auto-accept; set `Trusted=True` on device
- `AuthorizeService(device, uuid)` — auto-authorize `0000110b` (A2DP Sink) and `0000110a` (A2DP Source)
- Set `Adapter1.Discoverable = True`, `Pairable = True`, `DiscoverableTimeout = 0`

**Why no `bluetoothctl`:**
`bluetoothctl` outputs ANSI escape codes, has no JSON mode, registers its own agent (causing `AlreadyExists` errors), and all events are asynchronous and interleaved with prompt strings. It is not reliably parseable. Direct D-Bus avoids all of these issues.

**Pitfall — BlueZ 5.72+ signal reliability:** `PropertiesChanged` signals for `Connected` and `Paired` can be dropped. Implement active polling of `Device1.Connected` as a fallback alongside the signal handler.

**Library choice:** `dbus-python` (widely available, `python3-dbus` on Debian/Ubuntu) with `gi.repository.GLib.MainLoop`. For async integration with FastAPI: run the D-Bus GLib loop in a dedicated thread; communicate via `asyncio.Queue`.

---

### 3.2 `bt_manager` — Device & Adapter Manager

**Responsibility:** Expose a clean Python API for all Bluetooth operations needed by the web UI.

**Interface (internal Python API):**
```python
class BluetoothManager:
    def get_adapter_info() -> dict          # name, address, powered, discoverable
    def set_adapter_name(name: str)         # set Alias on org.bluez.Adapter1
    def get_devices() -> list[dict]         # all known devices with Connected/Paired/RSSI
    def remove_device(address: str)         # forget/unpair a device
    def start_discovery()                   # begin scanning
    def stop_discovery()                    # stop scanning
    def connect_device(address: str)        # connect specific device
    def disconnect_device(address: str)     # disconnect specific device
    def get_connected_devices() -> list     # actively connected devices
```

**D-Bus interfaces used:**
- `org.bluez.Adapter1` — power, discovery, alias (device name)
- `org.bluez.Device1` — connect, disconnect, trusted, paired
- `org.freedesktop.DBus.ObjectManager` — enumerate all BlueZ objects

**Adapter naming (speaker rename feature):**
```
org.bluez.Adapter1.Alias = "My Vinyl Player"
```
This is the name that remote devices see in their Bluetooth menu. It is persistent across reboots (stored by BlueZ in `/var/lib/bluetooth/`). The system hostname affects the default name; setting `Alias` overrides it independently.

**Pitfall — Adapter vs. Adapter.Name vs. Adapter.Alias:**
`Adapter1.Name` is read-only (reflects system hostname). `Adapter1.Alias` is the user-visible Bluetooth name and is writable. Always use `Alias`.

---

### 3.3 `audio_router` — PipeWire/ALSA Audio Routing

**Responsibility:** Detect when a Bluetooth A2DP source appears in the PipeWire graph and route it to the ALSA loopback device for web streaming.

**Audio path:**
```
PipeWire bluez_source.* node
         │
         │  pw-loopback (one instance per connected BT device)
         ▼
ALSA Loopback playback side  (hw:Loopback,0,0)
         │
         │  kernel loopback
         ▼
ALSA Loopback capture side  (hw:Loopback,1,0)
         │
         └──→ FFmpeg (in stream_manager)
```

**Why ALSA loopback instead of reading PipeWire directly:**
PipeWire's ALSA compatibility layer (`pipewire-alsa`) makes the loopback device accessible via standard ALSA tools (FFmpeg `-f alsa`). This means `stream_manager` has no PipeWire dependency — it only needs FFmpeg and ALSA. This isolation is critical for modularity and testability.

**PipeWire source detection strategy:**
```bash
# Poll until bluez source appears (called after device connects)
pw-cli ls Node | grep bluez_source
# Or via PipeWire D-Bus: org.freedesktop.DBus.ObjectManager on the pipewire socket
```

Alternatively: parse `pactl list sources short` and look for `bluez_source.*` or `bluez_input.*`. PipeWire's `pipewire-pulse` compatibility layer makes this available.

**pw-loopback command:**
```bash
pw-loopback \
  --capture-props='node.name=bt-source target.object=<bluez_node_id>' \
  --playback-props='node.name=bt-web-sink audio.device=hw:Loopback,0,0'
```

**Lifecycle management:**
- On `Device1.Connected = True` + A2DP profile active: start `pw-loopback` subprocess
- On `Device1.Connected = False`: terminate `pw-loopback` subprocess
- Track PID per device address for clean teardown
- On application shutdown: terminate all `pw-loopback` subprocesses and kill FFmpeg

**Pitfall — PipeWire source appearance latency:**
The `bluez_source.*` node does not appear instantly when the device connects. WirePlumber needs to negotiate the codec and create the node. Allow up to 3 seconds after `Connected=True` before concluding the source is unavailable. Use a retry loop with 500ms intervals, not a fixed sleep.

**Pitfall — Multiple simultaneous sources:**
If two BT devices connect simultaneously, each needs its own `pw-loopback`. The ALSA loopback only mixes them if multiple sources write to it simultaneously — this is the default ALSA mixer behaviour. Consider whether mono-source or multi-source mixing is required; default to mono-source (first connected device wins) for now, but make the policy configurable.

**Pitfall — snd-aloop module loading:**
`snd-aloop` must be loaded before the application starts. Add to `/etc/modules` or load via `modprobe snd-aloop` in the install script. Verify at startup with `aplay -l | grep Loopback`.

---

### 3.4 `stream_manager` — Per-Client Audio Stream

**Responsibility:** Manage one FFmpeg subprocess per connected web client, encoding the ALSA loopback capture to a format appropriate for that client's browser.

**Why per-client, not shared:**
A shared FFmpeg subprocess with a single `stdout` pipe can only have one reader. A second HTTP connection will either get no data or start a second FFmpeg, causing resource conflicts. Per-client subprocesses are wasteful in CPU but correct in behaviour and simple to reason about. At the expected scale (1–5 simultaneous browser tabs), this is acceptable. Future optimization: a single FFmpeg process writing to a named pipe or tee-split to multiple readers.

**Format selection:**
```
Client browser capability → Format
Chrome / Firefox          → Opus in WebM (frag_keyframe)
Safari macOS/iOS          → AAC in fragmented MP4
Fallback / HLS            → AAC in HLS segments (.m3u8)
```

**Detection method (JavaScript, client-side):**
```javascript
function getPreferredAudioFormat() {
    const a = document.createElement('audio');
    if (a.canPlayType('audio/webm; codecs="opus"') === 'probably') return 'webm-opus';
    if (a.canPlayType('audio/mp4; codecs="mp4a.40.2"') !== '')      return 'aac-mp4';
    return 'hls';
}
```

**FFmpeg command templates:**

*Opus/WebM (Chrome, Firefox):*
```bash
ffmpeg -f alsa -i hw:Loopback,1,0 \
  -c:a libopus -b:a 128k -vbr off \
  -application audio \
  -movflags frag_keyframe+empty_moov+default_base_moof \
  -f webm pipe:1
```
Note: `-vbr off` (CBR Opus) required for compatibility with any Safari version that claims WebM support.

*AAC/fMP4 (Safari macOS, Safari iOS):*
```bash
ffmpeg -f alsa -i hw:Loopback,1,0 \
  -c:a aac -b:a 192k -profile:a aac_low \
  -movflags frag_keyframe+empty_moov+default_base_moof \
  -f mp4 pipe:1
```

*HLS (universal fallback, 2–4 second latency):*
```bash
ffmpeg -f alsa -i hw:Loopback,1,0 \
  -c:a aac -b:a 192k \
  -f hls -hls_time 2 -hls_list_size 5 \
  -hls_flags delete_segments+omit_endlist \
  -hls_segment_filename /tmp/bt_hls/seg%03d.aac \
  /tmp/bt_hls/audio.m3u8
```

**Silence when no device connected:**
When the ALSA loopback has no writer, `arecord`/FFmpeg reads silence. This is acceptable behaviour. The UI should show a "No device connected — waiting for audio" state based on Bluetooth connection events, not audio presence.

**Pitfall — FFmpeg ALSA "no space left on device":**
If a previous FFmpeg process left the ALSA loopback in a bad state, a new open may fail. Implement a startup check: attempt to open `hw:Loopback,1,0` with `arecord -d 0` (zero-duration test capture) before starting the stream server. If it fails, reload `snd-aloop` (`rmmod snd-aloop && modprobe snd-aloop`).

**Pitfall — FFmpeg startup latency:**
FFmpeg buffers the first ~0.5 seconds before sending data. The browser's `<audio>` element may stall waiting for data. Use `-probesize 32 -analyzeduration 0` flags in the FFmpeg command to minimise startup buffering.

---

### 3.5 `web_server` — FastAPI Application

**Responsibility:** Serve the web UI, expose REST API for Bluetooth management, WebSocket for real-time events, and streaming endpoints for audio.

**Endpoints:**

```
GET  /                          → serve index.html
GET  /static/*                  → static assets (JS, CSS)

REST (JSON):
GET  /api/status                → adapter info, connected devices, audio state
GET  /api/devices               → list of known/scanned devices
POST /api/scan/start            → begin BT discovery
POST /api/scan/stop             → end BT discovery
POST /api/devices/{addr}/connect
POST /api/devices/{addr}/disconnect
POST /api/devices/{addr}/remove  → unpair/forget
POST /api/adapter/name          → {"name": "My Speaker"} — rename BT device
GET  /api/adapter/info          → current name, address, status

WebSocket:
WS   /ws/events                 → real-time BT events (device found, connected, disconnected)

Audio streaming:
GET  /stream/audio.webm         → Opus/WebM stream (chunked, per-client FFmpeg)
GET  /stream/audio.mp4          → AAC/fMP4 stream (chunked, per-client FFmpeg)
GET  /stream/audio.m3u8         → HLS playlist
GET  /stream/seg*.aac           → HLS segments (served from /tmp/bt_hls/)
```

**WebSocket event protocol (JSON):**
```json
{"event": "device_found",       "address": "XX:XX:XX:XX:XX:XX", "name": "iPhone", "rssi": -65}
{"event": "device_connected",   "address": "XX:XX:XX:XX:XX:XX", "name": "iPhone"}
{"event": "device_disconnected","address": "XX:XX:XX:XX:XX:XX"}
{"event": "audio_active",       "source": "iPhone"}
{"event": "audio_inactive"}
{"event": "adapter_updated",    "name": "My Speaker", "address": "YY:YY:YY:YY:YY:YY"}
{"event": "scan_started"}
{"event": "scan_stopped"}
{"event": "error",              "message": "..."}
```

**CORS configuration:**
For development: `allow_origins=["*"]`. For production: `allow_origins=["http://localhost:8080", "https://your-pi.local"]`. HTTPS is required for any deployment beyond localhost (Web Audio API secure context requirement).

**Process model:**
FastAPI/uvicorn runs in the foreground. D-Bus GLib loop runs in a background thread (`threading.Thread(daemon=True)`). Communication between D-Bus thread and FastAPI async event loop uses `asyncio.Queue` + `loop.call_soon_threadsafe()`.

---

### 3.6 Web UI — Browser Frontend

**Responsibility:** Provide a clean, single-page interface for Bluetooth management and audio playback.

**Features:**
- **Speaker Name:** Editable field at top — change the Bluetooth device name as it appears to source devices. Updates via `POST /api/adapter/name`.
- **Scan:** Button to start/stop Bluetooth discovery. Shows discovered devices with RSSI signal strength.
- **Device List:** Shows paired/known devices with Connect/Disconnect/Remove buttons. Real-time status via WebSocket.
- **Connected Device:** Prominent indicator when a source device is connected.
- **Audio Player:** `<audio>` element that auto-selects the appropriate stream format. Play/pause, volume.
- **Status Bar:** Shows Bluetooth status, adapter address, and audio state.

**Audio playback logic:**
```javascript
async function startAudio() {
    const fmt = getPreferredAudioFormat();
    const audio = document.getElementById('player');

    if (fmt === 'hls') {
        if (Hls.isSupported()) {
            const hls = new Hls({ lowLatencyMode: true, maxBufferLength: 4 });
            hls.loadSource('/stream/audio.m3u8');
            hls.attachMedia(audio);
        } else {
            // Native HLS in Safari
            audio.src = '/stream/audio.m3u8';
        }
    } else if (fmt === 'aac-mp4') {
        // Safari: AAC in fMP4 via MediaSource API
        streamViaMediaSource(audio, '/stream/audio.mp4', 'audio/mp4; codecs="mp4a.40.2"');
    } else {
        // Chrome/Firefox: Opus in WebM via MediaSource
        streamViaMediaSource(audio, '/stream/audio.webm', 'audio/webm; codecs="opus"');
    }
}
```

**Safari MSE (MediaSource API) pitfall — buffer underflow:**
Safari's MSE implementation pauses when the buffer is exhausted and does not auto-resume. The app must listen for the `waiting` event on the `<audio>` element and call `.play()` again.

```javascript
audio.addEventListener('waiting', () => {
    setTimeout(() => audio.play().catch(() => {}), 200);
});
```

**iOS Safari limitation:**
iOS Safari does not support MSE at all. HLS is the only option. Therefore: always use HLS on iOS, even if AAC/fMP4 might work on macOS Safari.

```javascript
const isIOS = /iPad|iPhone|iPod/.test(navigator.userAgent);
if (isIOS) return 'hls';
```

**No autoplay restriction workaround:**
Browsers block autoplay for audio until the user has interacted with the page. The "Connect & Play" button in the UI serves as the required user gesture — call `audio.play()` inside that click handler.

---

## 4. System Dependencies

### Required System Packages

```
# Bluetooth stack
bluez                    >= 5.70
bluez-tools              (for bt-adapter, bt-device utilities — optional but useful)

# Audio stack
pipewire                 >= 0.3.60
pipewire-pulse           (PulseAudio compatibility layer)
wireplumber              >= 0.4.14 (prefer 0.5+)
pipewire-audio           (meta package on Debian)

# ALSA loopback
linux kernel with snd-aloop support (standard in mainline)

# Audio encoding
ffmpeg                   >= 4.4 (with libopus and libfdk-aac or built-in aac)

# Python runtime
python3                  >= 3.10
python3-dbus             (dbus-python)
python3-gi               (PyGObject — for GLib.MainLoop)
```

### Python Package Dependencies

```
fastapi>=0.100
uvicorn[standard]>=0.23
websockets>=11.0
dbus-python>=1.3
PyGObject>=3.44
python-multipart>=0.0.6  (for form data, rename endpoint)
```

### Optional (future Chromecast support)
```
pychromecast>=14.0
zeroconf>=0.115
```

---

## 5. WirePlumber Configuration

The following configuration must be in place before the application runs:

**File:** `~/.config/wireplumber/wireplumber.conf.d/50-bluetooth-sink.conf`
```ini
monitor.bluez.properties = {
    # Only expose A2DP sink role — no HFP/HSP profile switching
    bluez5.roles = [ a2dp_sink ]
    bluez5.codecs = [ sbc sbc_xq aac ldac aptx aptx_hd ]
    bluez5.enable-sbc-xq = true
    bluez5.headset-roles = []
}

monitor.bluez.rules = [
    {
        matches = [ { device.name = "~bluez_card.*" } ]
        actions = {
            update-props = {
                bluez5.auto-connect = [ a2dp_sink ]
                bluez5.profile = "a2dp-sink"
            }
        }
    }
]
```

**Rationale for `a2dp_sink` only:**
Including HFP/HSP in `bluez5.roles` causes WirePlumber to auto-switch profiles when a phone call is initiated on the connected device (e.g., iPhone). This drops A2DP audio mid-stream and is the most common source of "it stopped working" reports. For a dedicated audio receiver, restrict to `a2dp_sink` only.

**Disable auto profile switching:**
```bash
wpctl settings --save bluetooth.autoswitch-to-headset-profile false
```

---

## 6. Systemd Service Configuration

**Key requirement:** Run as a **user service**, not a system service. PipeWire and WirePlumber run in the user session. A system service cannot access `$DBUS_SESSION_BUS_ADDRESS` or the PipeWire socket without explicit setup, which was the primary failure mode of the `Burger` repo.

**File:** `~/.config/systemd/user/bt-sink.service`
```ini
[Unit]
Description=Bluetooth A2DP Sink Web Server
After=pipewire.service wireplumber.service bluetooth.target
Wants=pipewire.service wireplumber.service

[Service]
Type=simple
ExecStartPre=/sbin/modprobe snd-aloop
ExecStart=/path/to/venv/bin/python -m btsink.web_server
Restart=on-failure
RestartSec=5
Environment=PYTHONUNBUFFERED=1

[Install]
WantedBy=default.target
```

**Enable with:**
```bash
systemctl --user daemon-reload
systemctl --user enable --now bt-sink.service
loginctl enable-linger $USER   # keeps user session alive without active login
```

**The `enable-linger` requirement:** Without `loginctl enable-linger`, the user session (and thus PipeWire, WirePlumber, and the D-Bus session bus) is torn down when the user logs out. `enable-linger` keeps the session alive indefinitely, essential for headless deployment.

---

## 7. Directory / Module Structure

```
btsink/
├── __init__.py
├── __main__.py              # Entry point: python -m btsink
│
├── bluetooth/
│   ├── __init__.py
│   ├── agent.py             # D-Bus pairing agent (NoInputNoOutput)
│   ├── manager.py           # Adapter + device control via D-Bus
│   └── events.py            # D-Bus signal → asyncio event bridge
│
├── audio/
│   ├── __init__.py
│   ├── router.py            # pw-loopback lifecycle management
│   ├── loopback.py          # snd-aloop detection and health check
│   └── stream_manager.py   # Per-client FFmpeg subprocess management
│
├── web/
│   ├── __init__.py
│   ├── server.py            # FastAPI app, route definitions
│   ├── static/
│   │   ├── index.html       # Single-page UI
│   │   ├── app.js           # BT management + WebSocket client
│   │   ├── audio.js         # Audio format detection + playback
│   │   └── style.css
│   └── templates/           # (empty for now; Jinja2 if needed later)
│
├── config.py                # Configuration dataclass (port, adapter name, etc.)
├── install.sh               # System dependency installer + service setup
└── requirements.txt
```

---

## 8. Known Pitfalls and Mitigations

| Pitfall | Root Cause | Mitigation |
|---|---|---|
| `snd-aloop` not loaded | Module not in `/etc/modules` | Check at startup; load if missing; fail loudly if kernel lacks support |
| WirePlumber not running | User session not started | Verify `pw-cli info` succeeds at startup; emit clear error |
| `bluez_source` never appears | WirePlumber config excludes sink role | Health-check endpoint; log WirePlumber config during startup |
| A2DP disconnects mid-stream | HFP auto-switch triggered by phone call | Restrict `bluez5.roles = [ a2dp_sink ]` only |
| Kernel 6.8 A2DP stutter | Known regression | Detect kernel version; apply adapter power-cycle workaround if on affected version |
| Safari MSE buffer pause | Safari does not auto-resume on underflow | Listen for `waiting` event; call `.play()` in setTimeout |
| iOS no MSE | iOS Safari policy | Detect iOS; force HLS path unconditionally |
| No autoplay | Browser security policy | Require explicit user click to start playback |
| D-Bus `AlreadyExists` on agent | Another process (bluetoothctl) registered an agent | Never run `bluetoothctl` concurrently; check for agent at startup |
| PropertiesChanged signals dropped | BlueZ 5.72+ bug | Poll `Device1.Connected` every 5 seconds as fallback |
| FFmpeg ALSA open fails | Prior FFmpeg left loopback locked | Test-capture on startup; reload `snd-aloop` if needed |
| FFmpeg startup buffering | FFmpeg buffers before first byte | Use `-probesize 32 -analyzeduration 0` flags |
| PipeWire source race | WirePlumber takes ~1-2s to create node | Poll for source with 500ms interval, max 10 retries |
| Loopback source has no writer (silence) | No BT device connected | Expected; UI reflects "waiting" state |
| Multiple devices connecting | Two `pw-loopback` procs compete for loopback write | Policy: first device wins; second device gets a "mixer" path (future feature) |
| FFmpeg `libfdk_aac` not available | GPL restrictions exclude `libfdk_aac` from some FFmpeg builds | Use built-in `aac` encoder (slightly lower quality but universally available); check at startup with `ffmpeg -codecs | grep aac` |
| HTTPS required for production | Web Audio API requires secure context | Document requirement; provide self-signed cert generation in install.sh |

---

## 9. Bluetooth Device Name Change — Detailed Flow

The speaker rename feature is a key UI requirement. Here is the exact mechanism:

1. User edits the name field in the web UI and clicks "Save"
2. Browser sends `POST /api/adapter/name` with `{"name": "My Vinyl Player"}`
3. `bt_manager.set_adapter_name("My Vinyl Player")` is called
4. Via D-Bus:
   ```python
   props.Set('org.bluez.Adapter1', 'Alias', 'My Vinyl Player')
   ```
5. BlueZ stores the alias in `/var/lib/bluetooth/<mac>/settings`
6. The name is now visible to any device scanning for Bluetooth speakers
7. Server sends `{"event": "adapter_updated", "name": "My Vinyl Player"}` to all WebSocket clients
8. UI updates to reflect the new name

**Important:** Devices that have already paired will continue to see the old name in their Bluetooth menu until they re-scan. This is a Bluetooth specification behaviour, not a bug.

---

## 10. Extensibility Design

The application is designed for future expansion without requiring architectural changes.

### Adding Chromecast Output (Phase 2)

The `stream_manager` currently creates per-client HTTP streams. To add Chromecast:
1. Add `cast/manager.py` module using `pychromecast`
2. Add `/api/cast/devices` endpoint (returns mDNS-discovered Chromecasts)
3. Add `/api/cast/play` endpoint: select a Chromecast + start a shared HTTP stream
4. The shared stream is served from a single long-running FFmpeg process writing to a named pipe with `tee` — one pipe end goes to the Chromecast, other ends go to browser WebSocket clients
5. No changes required to `bt_agent`, `bt_manager`, `audio_router`, or the ALSA loopback path

### Adding AirPlay Output (Phase 3)

Add `airplay/manager.py` using a proper RAOP implementation (`pyairplay` or shelling to `shairport-sync` in sender mode).

### Adding Multiple Bluetooth Adapters

`bt_manager` already iterates `ObjectManager` — support for a second adapter (e.g., USB dongle) requires only a configuration option to select which `hci*` adapter to use.

### Adding Snapcast Multi-Room

Replace the direct ALSA loopback write with a PipeWire FIFO module writing to Snapcast's `/tmp/snapfifo`. All browser clients then connect to Snapcast's WebSocket API. No changes to the Bluetooth layer.

### Adding EQ / Effects

Insert a GStreamer pipeline between the ALSA loopback read and FFmpeg encode:
```
hw:Loopback,1,0 → GStreamer (equalizer, compressor) → named pipe → FFmpeg
```

Or use PipeWire's `pw-filter` API to insert DSP plugins directly in the graph.

---

## 11. Installation Script Outline (`install.sh`)

```bash
# 1. System packages
apt-get install -y bluez bluez-tools pipewire pipewire-pulse wireplumber \
    python3 python3-pip python3-venv python3-dbus python3-gi ffmpeg

# 2. Load snd-aloop permanently
echo "snd-aloop" | tee -a /etc/modules
modprobe snd-aloop

# 3. Python virtualenv
python3 -m venv /opt/btsink/venv
/opt/btsink/venv/bin/pip install -r requirements.txt

# 4. WirePlumber config
mkdir -p ~/.config/wireplumber/wireplumber.conf.d/
cp config/50-bluetooth-sink.conf ~/.config/wireplumber/wireplumber.conf.d/

# 5. Restart WirePlumber
systemctl --user restart wireplumber pipewire pipewire-pulse

# 6. User service
cp config/bt-sink.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now bt-sink

# 7. Enable linger
loginctl enable-linger $USER

# 8. Verify installation
echo "Checking: BlueZ..."      && bluetoothctl show | grep Powered
echo "Checking: PipeWire..."   && pw-cli info default
echo "Checking: ALSA loopback..." && aplay -l | grep Loopback
echo "Checking: FFmpeg AAC..."  && ffmpeg -codecs 2>/dev/null | grep -E "aac|opus"
echo "All checks passed."
```

---

## 12. What Was Not Implemented in Previous Versions (and Why This Design Solves It)

### `a2dplaya` — A2DP audio capture gap

The `BluetoothManager._transport_fd` field and `_audio_thread` skeleton existed in the code but were never populated. The `MediaTransport1.Acquire()` call was never made, so no audio ever flowed from Bluetooth into the pipeline. This design sidesteps this entirely by letting WirePlumber own the transport FD and routing audio through the ALSA loopback.

### `Burger` — User session boundary

`Burger` ran its FastAPI server as a system service (`/etc/systemd/system/`) but PipeWire runs as a user session service (`~/.config/systemd/user/`). The `DBUS_SESSION_BUS_ADDRESS` and `PULSE_SERVER` environment variables that `pactl` and `pw-cli` need are only set in the user session. This caused `pactl list sources short` to fail with "Connection refused" whenever the service started before the user session was fully initialised. This design runs as a **user service** with `enable-linger`, which guarantees the PipeWire session is available.

### All implementations — Safari streaming

All previous implementations either used Opus/WebM (not supported in Safari on iOS, broken on macOS) or plain MP3 HTTP streams. This design explicitly provides AAC/fMP4 for Safari and HLS as a universal fallback, with client-side format detection determining which path to use.

### All implementations — `bluetoothctl` fragility

All previous implementations either shelled to `bluetoothctl` directly or used `pexpect` on `bluetoothctl` interactive mode. This design uses D-Bus exclusively for all Bluetooth operations.

---

## 13. Lastly

1. **Target OS/distribution:**  Ubuntu 24 LTS on intel. Rust preferred (even though references to Python in thi sdocument) fallback to Python if needed).

2. **Web port and HTTPS:** HTTP only for a local lan, with a user selectable port on install. If the port is in use, the application should increment to another port and informa the user upon successful setup.

3. **Single room audio:** ALSA loopback single-source model acceptable for phase 1. If two devices connect, first-connected wins.

4. **FFmpeg `libfdk_aac` vs built-in `aac`:** Built-in FFmpeg AAC encoder is lower quality than `libfdk_aac` but universally available. Verify target machine's FFmpeg build upon installing, install if not installed and configure correctly..

5. **PipeWire version:** WirePlumber 0.5+ uses SPA-JSON config syntax (`~/.config/wireplumber/wireplumber.conf.d/*.conf`); WirePlumber 0.4 uses Lua scripts. Determine target version and write config for the correct syntax. This design shows 0.5+ syntax, this should be based on the installer figuring out what is available.

---

*End of design document. Ready for implementation review.*
