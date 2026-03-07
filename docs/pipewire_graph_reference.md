# PipeWire Graph Reference

This document describes the PipeWire audio graph as created by SoundSync.

## Graph Topology

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                          PipeWire Graph                                     │
│                                                                             │
│  ┌──────────────────────────┐    ┌───────────────────────────────────────┐  │
│  │  bluez_input.*           │    │  soundsync-eq                         │  │
│  │  (WirePlumber/BlueZ)     │───▶│  (SoundSync EQ Filter)                │  │
│  │                          │    │                                       │  │
│  │  media.class:            │    │  node.name: soundsync-eq              │  │
│  │    Audio/Source          │    │  media.type: Audio                    │  │
│  │  node.name:              │    │  media.category: Filter               │  │
│  │    bluez_input.XX_XX_XX  │    │  media.role: DSP                      │  │
│  └──────────────────────────┘    └──────────────────┬────────────────────┘  │
│                                                     │                       │
│                                                     ▼                       │
│                                  ┌───────────────────────────────────────┐  │
│                                  │  alsa_output.* / default.sink         │  │
│                                  │  (System Audio Output)                │  │
│                                  └───────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Node Properties

### Bluetooth Source Node (created by WirePlumber)

| Property | Value |
|----------|-------|
| `node.name` | `bluez_input.XX_XX_XX_XX_XX_XX` |
| `media.class` | `Audio/Source` |
| `object.id` | (dynamic, assigned by PipeWire) |
| `device.api` | `bluez5` |
| `api.bluez5.profile` | `a2dp-source` |

### SoundSync EQ Filter Node

| Property | Value |
|----------|-------|
| `node.name` | `soundsync-eq` |
| `node.description` | `SoundSync 10-Band EQ` |
| `media.type` | `Audio` |
| `media.category` | `Filter` |
| `media.role` | `DSP` |
| `node.autoconnect` | `true` |
| `audio.position` | `[ FL, FR ]` |

### Ports

| Port | Direction | Channel | Format |
|------|-----------|---------|--------|
| `input_FL` | Input | Front Left | 32-bit float mono |
| `input_FR` | Input | Front Right | 32-bit float mono |
| `output_FL` | Output | Front Left | 32-bit float mono |
| `output_FR` | Output | Front Right | 32-bit float mono |

## Monitoring the Graph

```bash
# View all nodes
pw-cli list-objects Node

# View the EQ filter node
pw-cli info $(pw-cli list-objects Node | grep soundsync-eq | awk '{print $1}')

# Dump the entire graph
pw-dump | jq '.[] | select(.type == "PipeWire:Interface:Node") | {name: .info.props["node.name"], class: .info.props["media.class"]}'

# Monitor graph changes in real-time
pw-mon

# View links
pw-link -l
```

## Sample Rate

SoundSync operates at **48000 Hz** (standard Bluetooth A2DP sample rate).

The quantum (buffer size) defaults to **512 samples** = 10.67ms per cycle.

## WirePlumber Integration

WirePlumber automatically:
1. Detects the Bluetooth device connection
2. Creates the `bluez_input.*` source node
3. Links it to available sinks (or the SoundSync filter)

SoundSync monitors the registry for `bluez_input.*` and `bluez_source.*` nodes
and handles linking via `node.autoconnect = true`.

## Troubleshooting

```bash
# Restart PipeWire and WirePlumber
systemctl --user restart pipewire wireplumber

# Reset the PipeWire state
rm -rf ~/.local/state/pipewire
systemctl --user restart pipewire wireplumber

# Check PipeWire is running
systemctl --user status pipewire

# Check WirePlumber
systemctl --user status wireplumber

# Verbose PipeWire logging
PIPEWIRE_DEBUG=3 soundsync
```
