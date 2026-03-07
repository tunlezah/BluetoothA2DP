# PipeWire DSP Equaliser Design

This document defines how the graphical equaliser will be implemented in
the PipeWire audio pipeline.

------------------------------------------------------------------------

# Goal

Allow the Web UI equaliser to control **real‑time DSP filters** applied
to Bluetooth audio.

------------------------------------------------------------------------

# Audio Pipeline

    Bluetooth Device
          │
          ▼
    PipeWire Source Node
          │
          ▼
    DSP Filter Chain
          │
          ▼
    Encoder
          │
          ▼
    Web Stream

------------------------------------------------------------------------

# PipeWire Filter Node

PipeWire supports filter nodes via:

    pw-filter

or

    SPA DSP plugins

------------------------------------------------------------------------

# Equaliser Bands

Standard **10‑band EQ**.

  Band   Frequency
  ------ -----------
  1      60Hz
  2      120Hz
  3      250Hz
  4      500Hz
  5      1kHz
  6      2kHz
  7      4kHz
  8      8kHz
  9      12kHz
  10     16kHz

------------------------------------------------------------------------

# DSP Filter Type

Recommended filter:

    biquad peaking filter

Each band uses:

-   gain
-   Q factor
-   frequency

------------------------------------------------------------------------

# Filter Chain Example

    bluez_source
       │
       ▼
    EQ band 1
       │
    EQ band 2
       │
    EQ band 3
       │
    ...
       │
    EQ band 10
       │
       ▼
    encoder input

------------------------------------------------------------------------

# Control Interface

Web UI sends filter updates:

    POST /api/eq

Example payload:

``` json
{
 "bands": [
  {"freq":60,"gain":3},
  {"freq":120,"gain":2},
  {"freq":250,"gain":0},
  {"freq":500,"gain":-1}
 ]
}
```

------------------------------------------------------------------------

# Backend Processing

Steps:

1.  receive EQ update
2.  update filter coefficients
3.  apply via PipeWire API

------------------------------------------------------------------------

# PipeWire API Options

Two main approaches:

### Method 1 --- pw-filter

Simple external filter node.

Pros:

-   easy implementation

Cons:

-   less dynamic control

------------------------------------------------------------------------

### Method 2 --- Native PipeWire DSP

Implement custom filter node.

Pros:

-   best performance
-   dynamic control

Cons:

-   more complex

------------------------------------------------------------------------

# Latency Considerations

DSP chain must maintain latency below:

    20ms

Recommended:

    float32 processing

------------------------------------------------------------------------

# Presets

Provide preset profiles.

Examples:

-   Flat
-   Bass Boost
-   Vinyl Warm
-   Speech

Stored in:

    ~/.config/btsink/eq-presets.json

------------------------------------------------------------------------

# Safety Limits

Limit gain range:

    -12dB to +12dB

Prevent clipping.

------------------------------------------------------------------------

# Future Enhancements

Possible future DSP:

-   compressor
-   limiter
-   stereo widening
-   room correction

------------------------------------------------------------------------

# Summary

The PipeWire DSP equaliser allows the web interface to control real‑time
audio processing with minimal latency while remaining fully integrated
with the PipeWire graph.
