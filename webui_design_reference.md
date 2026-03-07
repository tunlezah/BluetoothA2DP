# Web UI Design Reference

This document defines the visual design and layout of the BluetoothA2DP
web interface.

------------------------------------------------------------------------

## Design Philosophy

The interface should resemble modern Apple web applications.

Design inspiration:

-   Apple Music Web UI
-   Apple System Settings
-   Apple Glass aesthetic

Key principles:

-   glassmorphism
-   blur backgrounds
-   minimal controls
-   large touch targets

------------------------------------------------------------------------

## Layout

Single-page application with no scrolling.

    -------------------------------------------------
    | Header: Speaker Name | Bluetooth Status       |
    -------------------------------------------------
    | Device Panel | Equalizer | Playback Controls  |
    -------------------------------------------------
    | Hidden Settings Drawer                         |
    -------------------------------------------------

------------------------------------------------------------------------

## Apple Glass Style

Visual elements:

-   translucent panels
-   backdrop blur
-   soft shadows
-   rounded corners

Example CSS concept:

    .glass-panel {
     backdrop-filter: blur(20px);
     background: rgba(255,255,255,0.15);
     border-radius: 20px;
    }

------------------------------------------------------------------------

## Equalizer Panel

Graphical equalizer with 10 bands.

    60Hz
    120Hz
    250Hz
    500Hz
    1kHz
    2kHz
    4kHz
    8kHz
    12kHz
    16kHz

Each band adjustable via vertical sliders.

------------------------------------------------------------------------

## Device Panel

Shows:

-   discovered devices
-   signal strength
-   connect button

Compact card layout.

------------------------------------------------------------------------

## Playback Panel

Controls:

-   play
-   pause
-   volume
-   stream status

------------------------------------------------------------------------

## Hidden Settings

Settings drawer expandable from bottom.

Contains:

-   port configuration
-   codec preference
-   trusted devices

Collapsed by default.

------------------------------------------------------------------------

## Responsive Behaviour

UI must adapt for:

-   desktop browsers
-   tablets
-   phones

Panels collapse into horizontal stack.

------------------------------------------------------------------------

## Accessibility

Support:

-   keyboard navigation
-   screen readers
-   high contrast mode
