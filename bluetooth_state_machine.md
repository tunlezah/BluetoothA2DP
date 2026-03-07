# Bluetooth Device State Machine

This document defines the lifecycle of Bluetooth devices interacting
with the BluetoothA2DP system.

------------------------------------------------------------------------

## Overview

A strict state machine prevents race conditions between:

-   Bluetooth connection events
-   PipeWire node creation
-   audio routing
-   stream activation

------------------------------------------------------------------------

## State Diagram

    DISCONNECTED
        │
        ▼
    DISCOVERED
        │
        ▼
    PAIRING
        │
        ▼
    PAIRED
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

------------------------------------------------------------------------

## State Descriptions

### DISCONNECTED

Device is not known or not present.

### DISCOVERED

Device detected during scan.

### PAIRING

Pairing handshake occurring.

### PAIRED

Device is trusted and remembered.

### CONNECTED

Bluetooth transport connected.

### PROFILE_NEGOTIATED

A2DP profile confirmed in device UUID list.

### PIPEWIRE_SOURCE_READY

PipeWire created the Bluetooth audio node.

### AUDIO_ACTIVE

Audio packets flowing through the pipeline.

------------------------------------------------------------------------

## Transition Triggers

  Transition                                   Trigger
  -------------------------------------------- -----------------------
  DISCOVERED → PAIRING                         user connect request
  PAIRING → PAIRED                             BlueZ pairing success
  PAIRED → CONNECTED                           device reconnect
  CONNECTED → PROFILE_NEGOTIATED               A2DP UUID detected
  PROFILE_NEGOTIATED → PIPEWIRE_SOURCE_READY   PipeWire node created
  PIPEWIRE_SOURCE_READY → AUDIO_ACTIVE         audio frames detected
