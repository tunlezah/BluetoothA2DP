# PipeWire Audio Flow

This document explains the internal audio pipeline used by the
application.

------------------------------------------------------------------------

## Audio Path

    Bluetooth Device
          │
          ▼
    BlueZ A2DP Transport
          │
          ▼
    WirePlumber Session Manager
          │
          ▼
    PipeWire Graph Node
          │
          ▼
    Audio Router
          │
          ▼
    Encoder
          │
          ▼
    Web Streaming

------------------------------------------------------------------------

## PipeWire Node Types

Typical nodes:

    bluez_source.xx_xx_xx
    alsa_output.loopback
    ffmpeg_capture

------------------------------------------------------------------------

## Debug Commands

List nodes:

    pw-cli ls Node

Inspect audio graph:

    pw-top

List devices:

    pw-cli ls Device

------------------------------------------------------------------------

## Troubleshooting

### Node missing

Restart WirePlumber:

    systemctl --user restart wireplumber

### No audio frames

Check PipeWire link graph:

    pw-link -l

------------------------------------------------------------------------

## Latency Notes

Direct PipeWire capture provides lower latency than ALSA loopback.
