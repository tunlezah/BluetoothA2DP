# BluetoothA2DP --- Robust Application Architecture

**Version:** 2.0\
**Status:** Implementation Architecture\
**Primary Target:** Ubuntu 24.04 LTS (Intel x86_64)\
**Preferred Language:** Rust (Python fallback)

------------------------------------------------------------------------

## 1. Executive Summary

BluetoothA2DP is a headless Bluetooth audio receiver that exposes a
Linux machine as a Bluetooth speaker and streams received audio to
browsers on the local network.

Goals:

-   Robust Bluetooth handling
-   Automatic recovery from failures
-   Low latency streaming
-   Safari-compatible playback
-   Clean extensible architecture

Core components:

-   BlueZ --- Bluetooth stack
-   PipeWire --- audio routing
-   WirePlumber --- session manager
-   FFmpeg --- audio encoder
-   Rust backend --- control + streaming
-   Web UI --- device management

------------------------------------------------------------------------

## 2. System Architecture

    Bluetooth Device
          │
          ▼
    BlueZ Bluetooth Stack
          │
          ▼
    WirePlumber Policy
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
    Web Server
          │
          ▼
    Browser Clients

The Bluetooth stack handles A2DP transport.\
The application handles **control, routing, and streaming**.

------------------------------------------------------------------------

## 3. Core Design Principles

### Never Implement A2DP Yourself

A2DP transport is handled by:

-   BlueZ
-   WirePlumber
-   PipeWire

The application must never attempt to read MediaTransport sockets
directly.

------------------------------------------------------------------------

### Never Use `bluetoothctl`

All Bluetooth interactions occur through D‑Bus:

    org.bluez.Adapter1
    org.bluez.Device1
    org.bluez.Agent1

`bluetoothctl` is avoided due to unreliable parsing and agent conflicts.

------------------------------------------------------------------------

### Delegate Audio to PipeWire

PipeWire automatically exposes Bluetooth sources such as:

    bluez_source.xx_xx_xx

The application consumes these nodes.

------------------------------------------------------------------------

### Design for Failure

Bluetooth hardware frequently fails.\
The system must detect and recover from:

-   adapter crashes
-   PipeWire failures
-   dropped connections
-   codec negotiation failures

------------------------------------------------------------------------

## 4. Bluetooth State Machine

To prevent race conditions the system uses a strict device state
machine.

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

  State                   Trigger
  ----------------------- --------------------------
  CONNECTED               Device1.Connected = true
  PROFILE_NEGOTIATED      A2DP UUID detected
  PIPEWIRE_SOURCE_READY   PipeWire node exists
  AUDIO_ACTIVE            audio packets flowing

------------------------------------------------------------------------

## 5. Bluetooth Control API

Internal interface:

``` python
class BluetoothManager:
    get_adapter_info()
    set_adapter_name(name)

    start_scan()
    stop_scan()

    get_devices()
    connect_device(addr)
    disconnect_device(addr)
    remove_device(addr)

    get_connected_devices()
```

------------------------------------------------------------------------

### Changing Device Name

The Bluetooth device name is set using:

    org.bluez.Adapter1.Alias

Example:

    props.Set(
      "org.bluez.Adapter1",
      "Alias",
      "My Bluetooth Speaker"
    )

------------------------------------------------------------------------

## 6. Audio Pipeline

### Phase 1 --- ALSA Loopback

    PipeWire
       │
    pw-loopback
       │
    ALSA Loopback
       │
    FFmpeg

Advantages:

-   simple implementation
-   easy FFmpeg integration

Disadvantages:

-   extra latency
-   additional dependency

------------------------------------------------------------------------

### Phase 2 --- Native PipeWire Capture

    PipeWire Node
         │
    PipeWire API
         │
    Encoder

Advantages:

-   lower latency
-   fewer components
-   higher reliability

------------------------------------------------------------------------

## 7. Streaming Architecture

### Phase 1

Per-client encoders.

    Audio Source
        │
    FFmpeg
        │
    HTTP stream

### Phase 2

Shared encoder.

    PipeWire
        │
    Single Encoder
        │
    Broadcast Buffer
        │
    Multiple Clients

Benefits:

-   lower CPU usage
-   synchronized playback
-   improved scalability

------------------------------------------------------------------------

## 8. Browser Compatibility

  Browser        Codec
  -------------- -----------
  Chrome         Opus/WebM
  Firefox        Opus/WebM
  Safari macOS   AAC/fMP4
  Safari iOS     HLS

The web client performs automatic detection.

------------------------------------------------------------------------

## 9. Web API

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

Streaming:

    /stream/audio.webm
    /stream/audio.mp4
    /stream/audio.m3u8

------------------------------------------------------------------------

## 10. Security Model

Pairing is restricted.

Policy:

-   pairing allowed only during scan
-   trusted devices saved locally
-   unknown devices rejected

Trusted device list:

    ~/.config/btsink/devices.json

------------------------------------------------------------------------

## 11. Failure Recovery

The system continuously checks:

  Component      Detection
  -------------- ------------------
  Bluetooth      D‑Bus ping
  PipeWire       pw-cli info
  Audio source   PipeWire node
  Encoder        process watchdog

Automatic recovery actions:

-   restart bluetooth
-   restart PipeWire
-   reconnect device
-   restart encoder

------------------------------------------------------------------------

## 12. Diagnostics

Structured logging recommended.

Example events:

    BT_DEVICE_CONNECTED
    BT_DEVICE_DISCONNECTED
    PIPEWIRE_SOURCE_CREATED
    STREAM_STARTED
    STREAM_STOPPED

------------------------------------------------------------------------

### Debug Endpoint

    GET /api/debug

Returns:

-   adapter status
-   connected devices
-   PipeWire nodes
-   encoder processes
-   kernel version

------------------------------------------------------------------------

## 13. Health Endpoints

    /health/bluetooth
    /health/audio
    /health/stream

Example:

``` json
{
 "bluetooth": "ok",
 "audio": "ok",
 "stream": "active"
}
```

------------------------------------------------------------------------

## 14. Configuration

Runtime config:

    ~/.config/btsink/config.toml

Example:

``` toml
port = 8080
adapter = "hci0"

[audio]
bitrate = 192000
codec = "aac"

[bluetooth]
auto_pair = true
max_devices = 1
```

------------------------------------------------------------------------

## 15. Rust Architecture

Suggested layout:

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

Recommended crates:

  Purpose         Crate
  --------------- -------------------
  D‑Bus           zbus
  PipeWire        pipewire-rs
  HTTP            axum
  WebSocket       tokio-tungstenite
  Async runtime   tokio

------------------------------------------------------------------------

## 16. System Dependencies

Required packages:

    bluez
    pipewire
    pipewire-pulse
    wireplumber
    ffmpeg

Kernel module:

    snd-aloop

------------------------------------------------------------------------

## 17. Systemd Service

Runs as a **user service**.

    ~/.config/systemd/user/btsink.service

Required command:

    loginctl enable-linger USER

------------------------------------------------------------------------

## 18. Future Expansion

Planned features:

Phase 2

-   Chromecast support
-   native PipeWire capture
-   shared encoder

Phase 3

-   AirPlay output
-   Snapcast multiroom

Phase 4

-   DSP pipeline
-   EQ
-   compression

------------------------------------------------------------------------

## Conclusion

This architecture creates a robust Bluetooth audio receiver by
delegating low‑level audio transport to the Linux audio stack and
focusing the application on control, routing, and streaming.
