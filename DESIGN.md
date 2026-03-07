BluetoothA2DP — Robust Application Architecture
Version: 2.0 (Implementation Architecture)
Status: Design approved for implementation
Primary Target: Ubuntu 24.04 LTS (Intel x86_64)
Preferred Language: Rust
Fallback Language: Python
Interface: Local web UI (Safari-first compatibility)
1. Executive Summary
BluetoothA2DP is a headless Bluetooth audio receiver and web streaming system that presents a Linux machine as a Bluetooth speaker and streams received audio to browsers over the local network.
The application is designed for:
robust Bluetooth handling
automatic recovery from failures
low-latency audio streaming
cross-browser compatibility (including Safari)
future extensibility (Chromecast, AirPlay, Snapcast)
The system leverages the Linux Bluetooth and audio stack rather than attempting to re-implement it.
Core components include:
BlueZ — Linux Bluetooth stack
PipeWire — audio routing and processing
WirePlumber — PipeWire session manager
FFmpeg — audio encoding
Rust backend — Bluetooth control and streaming server
Web UI — device management and playback
The system is designed to avoid known failure modes common in existing Bluetooth audio projects.
2. System Architecture
Bluetooth Device
      │
      │ A2DP
      ▼
BlueZ Bluetooth Stack
      │
      ▼
WirePlumber Policy Manager
      │
      ▼
PipeWire Audio Graph
      │
      ▼
Audio Router
      │
      ▼
Encoder (FFmpeg)
      │
      ▼
Web Server (Rust)
      │
      ▼
Browser Clients
The Bluetooth transport layer is handled entirely by BlueZ + PipeWire.
The application focuses only on:
device management
audio routing
encoding
web streaming
3. Core Design Principles
3.1 Never Implement A2DP Yourself
A2DP transport is handled by:
BlueZ
WirePlumber
PipeWire
Attempting to manually read MediaTransport1 sockets is unnecessary and fragile.
3.2 Never Use bluetoothctl
Bluetooth control must use the D-Bus API directly.
Problems with bluetoothctl:
ANSI output
asynchronous prompts
unreliable parsing
agent conflicts
The application communicates directly with:
org.bluez.Adapter1
org.bluez.Device1
org.bluez.Agent1
3.3 Delegate Audio to PipeWire
The application does not manipulate raw Bluetooth audio streams.
PipeWire automatically creates nodes such as:
bluez_source.XX_XX_XX
These are routed to the encoder.
3.4 Build for Recovery
Bluetooth hardware fails frequently.
The system must automatically recover from:
adapter crashes
PipeWire failures
codec negotiation issues
dropped connections
4. Bluetooth State Machine
To prevent race conditions, Bluetooth device connections follow a strict state machine.
DISCONNECTED
    │
    ▼
CONNECTED
    │
    ▼
PROFILE_NEGOTIATED
    │
    ▼
PIPEWIRE_SOURCE_READY
    │
    ▼
AUDIO_ACTIVE
Transitions occur when:
State	Trigger
CONNECTED	Device1.Connected=True
PROFILE_NEGOTIATED	A2DP UUID present
PIPEWIRE_SOURCE_READY	PipeWire node detected
AUDIO_ACTIVE	audio packets flowing
5. Bluetooth Control Layer
The Bluetooth layer exposes a clean internal API.
Example interface:
BluetoothManager
Functions:
get_adapter_info()
set_adapter_name(name)

start_scan()
stop_scan()

get_devices()
connect_device(address)
disconnect_device(address)
remove_device(address)

get_connected_devices()
Adapter Name Change
Bluetooth name changes are performed via:
org.bluez.Adapter1.Alias
Example:
props.Set(
  "org.bluez.Adapter1",
  "Alias",
  "My Bluetooth Speaker"
)
This updates the visible device name for scanning devices.
6. Audio Pipeline
Two audio routing strategies are supported.
6.1 Phase 1 (Simplest)
PipeWire
   │
pw-loopback
   │
ALSA Loopback
   │
FFmpeg
Advantages:
easy integration
simple FFmpeg capture
Disadvantages:
extra latency
ALSA dependency
6.2 Phase 2 (Preferred)
Direct PipeWire capture.
PipeWire Node
     │
PipeWire API
     │
Encoder
Advantages:
lower latency
fewer dependencies
higher reliability
7. Streaming Architecture
Each browser receives an encoded audio stream.
Phase 1 uses per-client encoders.
ALSA capture
     │
FFmpeg
     │
HTTP stream
Phase 2 will introduce a shared encoder.
PipeWire
    │
Single Encoder
    │
Broadcast Buffer
    │
Multiple Clients
Benefits:
lower CPU usage
consistent latency
improved scalability
8. Browser Compatibility
Different browsers require different codecs.
Browser	Format
Chrome	Opus/WebM
Firefox	Opus/WebM
Safari (macOS)	AAC/fMP4
Safari (iOS)	HLS
The frontend performs automatic capability detection.
9. Web Server
The application exposes a REST API.
Endpoints:
GET  /
GET  /api/status
GET  /api/devices
POST /api/scan/start
POST /api/scan/stop
POST /api/devices/{addr}/connect
POST /api/devices/{addr}/disconnect
POST /api/devices/{addr}/remove
POST /api/adapter/name
Streaming endpoints:
/stream/audio.webm
/stream/audio.mp4
/stream/audio.m3u8
10. Security Model
Bluetooth pairing is restricted.
Policy:
pairing allowed only during active scan
devices added to trusted list
unknown devices rejected after scan ends
Trusted devices stored in:
~/.config/btsink/devices.json
11. Failure Recovery
The system continuously monitors:
Component	Detection
Bluetooth adapter	D-Bus health check
PipeWire	pw-cli info
Audio source	PipeWire node presence
Encoder	process monitoring
Automatic recovery includes:
restarting Bluetooth
restarting PipeWire
reconnecting devices
restarting encoder
12. Diagnostics and Observability
Structured logging is required.
Recommended Rust crate:
tracing
Events include:
BT_DEVICE_CONNECTED
BT_DEVICE_DISCONNECTED
PIPEWIRE_SOURCE_CREATED
STREAM_STARTED
STREAM_STOPPED
Debug Endpoint
GET /api/debug
Returns:
Bluetooth adapter status
Connected devices
PipeWire nodes
Encoder processes
Kernel version
13. Health Checks
Health endpoints allow monitoring.
/health/bluetooth
/health/audio
/health/stream
Example response:
{
 "bluetooth": "ok",
 "audio": "ok",
 "stream": "active"
}
14. Configuration
Runtime configuration stored in:
~/.config/btsink/config.toml
Example:
port = 8080
adapter = "hci0"

[audio]
bitrate = 192000
codec = "aac"

[bluetooth]
auto_pair = true
max_devices = 1
15. Rust Architecture
Preferred implementation modules:
src/

bluetooth/
  adapter.rs
  device.rs
  agent.rs
  events.rs

audio/
  pipewire.rs
  router.rs
  encoder.rs

streaming/
  broadcast.rs
  session.rs

web/
  api.rs
  websocket.rs
  static.rs

config.rs
main.rs
Recommended Rust crates:
Purpose	Crate
D-Bus	zbus
PipeWire	pipewire-rs
HTTP	axum
WebSocket	tokio-tungstenite
Async runtime	tokio
16. System Dependencies
Required packages:
bluez
pipewire
pipewire-pulse
wireplumber
ffmpeg
python3 (fallback)
Kernel requirement:
snd-aloop
17. Systemd Service
Runs as a user service.
~/.config/systemd/user/btsink.service
Key requirement:
loginctl enable-linger USER
This ensures the PipeWire session remains active.
18. Known Linux Pitfalls
Issue	Mitigation
A2DP drop during phone calls	disable HFP profiles
PipeWire source race	retry detection
Bluetooth adapter freeze	watchdog restart
Safari buffer pause	resume playback
iOS no MSE	force HLS
19. Future Expansion
Planned features:
Phase 2
Chromecast output
PipeWire native capture
shared encoder
Phase 3
AirPlay output
Snapcast multiroom
Phase 4
DSP pipeline
equalizer
compression
20. Implementation Priority
Phase 1 tasks:
Bluetooth manager
pairing agent
PipeWire source detection
ALSA capture
FFmpeg streaming
web UI
Conclusion
This architecture provides a robust, extensible Bluetooth audio platform built on the modern Linux audio stack.
By relying on BlueZ + PipeWire for low-level functionality and focusing the application on control, routing, and streaming, the design avoids many pitfalls that plague typical Bluetooth audio implementations.
