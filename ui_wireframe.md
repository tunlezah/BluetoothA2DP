# UI Wireframe --- Single Page Layout

This document defines the **pixel‑level layout and grid structure** for
the BluetoothA2DP Web UI.

Goals:

-   Single page
-   No scrolling
-   Apple‑style glass interface
-   Responsive for desktop and tablet
-   Equaliser always visible
-   Settings hidden unless expanded

------------------------------------------------------------------------

# Layout Grid

The UI uses a **12‑column CSS grid**.

    |----------------------------------------------------------|
    | Header (12 columns)                                      |
    |----------------------------------------------------------|
    | Devices (4) | Equaliser (5) | Playback (3)               |
    |----------------------------------------------------------|
    | Expandable Settings Drawer (12)                          |
    |----------------------------------------------------------|

------------------------------------------------------------------------

# Header

Height: **72px**

Contents:

Left:

-   Speaker Name (editable)
-   Bluetooth icon
-   connection status

Right:

-   settings button
-   scan button

Example:

     -----------------------------------------------------------
     |  My Bluetooth Speaker    🔵 Connected        ⚙ Scan     |
     -----------------------------------------------------------

------------------------------------------------------------------------

# Devices Panel

Columns: **1--4**

Purpose:

Display nearby and trusted devices.

Layout:

     ---------------------------------
     | Devices                        |
     |--------------------------------|
     | iPhone 15      -55dB   Connect |
     | Vinyl Player   -60dB   Connect |
     | Laptop         -70dB   Connect |
     ---------------------------------

Device card:

-   device name
-   signal strength
-   connect/disconnect button

Cards are **glass panels**.

------------------------------------------------------------------------

# Equaliser Panel

Columns: **5--9**

Central feature of UI.

10‑band graphical equaliser.

    60Hz   ███
    120Hz  █████
    250Hz  ██
    500Hz  ████
    1kHz   ███
    2kHz   █████
    4kHz   ███
    8kHz   ████
    12kHz  ██
    16kHz  ███

Each band:

-   vertical slider
-   drag control
-   double‑click reset

------------------------------------------------------------------------

# Playback Panel

Columns: **10--12**

Controls:

     --------------------------
     |        PLAYBACK         |
     |-------------------------|
     |   ▶️   ⏸️                |
     | Volume:  ███████        |
     | Stream: Active          |
     --------------------------

------------------------------------------------------------------------

# Settings Drawer

Collapsed by default.

Expands from bottom.

Height:

-   collapsed: **48px**
-   expanded: **300px**

Contents:

    Port
    Codec
    Trusted Devices
    Diagnostics
    ``

    ---

    # Apple Glass Styling

    Visual characteristics:

    - translucent backgrounds
    - soft shadows
    - large rounded corners
    - subtle gradients

    Example CSS:

    ```css
    .glass {
      backdrop-filter: blur(30px);
      background: rgba(255,255,255,0.15);
      border-radius: 24px;
      border: 1px solid rgba(255,255,255,0.25);
    }

------------------------------------------------------------------------

# Color Palette

Primary background:

    #0b0b0c

Glass panels:

    rgba(255,255,255,0.15)

Accent:

    #0a84ff

Equaliser active band:

    #30d158

------------------------------------------------------------------------

# Interaction Design

Animations:

-   device connect highlight
-   equaliser slider bounce
-   panel hover glow

Recommended duration:

    120–200ms

------------------------------------------------------------------------

# Accessibility

Requirements:

-   keyboard navigation
-   ARIA labels
-   high contrast mode

------------------------------------------------------------------------

# Implementation Suggestions

Frontend framework options:

-   vanilla JS + Web Components
-   Svelte
-   React

Avoid heavy UI frameworks to maintain performance.
