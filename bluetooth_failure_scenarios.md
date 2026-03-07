# Bluetooth Failure Scenarios

This document describes real‑world Bluetooth failures that must be
handled automatically by the system.

------------------------------------------------------------------------

## 1. Adapter Freeze

Symptoms:

-   device disconnects
-   new connections fail
-   D‑Bus calls time out

Detection:

    Adapter1.Powered fails

Recovery:

    rfkill unblock bluetooth
    systemctl restart bluetooth

------------------------------------------------------------------------

## 2. PipeWire Crash

Symptoms:

-   audio stops
-   PipeWire nodes disappear

Detection:

    pw-cli info fails

Recovery:

    systemctl --user restart pipewire
    systemctl --user restart wireplumber

------------------------------------------------------------------------

## 3. PipeWire Source Race

Symptoms:

-   device connects
-   no audio node created

Cause:

WirePlumber delay during codec negotiation.

Mitigation:

Retry detection every 500 ms for up to 5 seconds.

------------------------------------------------------------------------

## 4. Device Connects Without A2DP

Symptoms:

-   device shows connected
-   no audio

Cause:

AVRCP only connection.

Detection:

Check device UUID list for A2DP.

------------------------------------------------------------------------

## 5. Phone Call Interruptions

Phones may switch profiles during calls.

Mitigation:

Disable HFP/HSP profiles.

------------------------------------------------------------------------

## 6. Bluetooth Adapter Removal

USB adapters can be unplugged.

Detection:

    InterfacesRemoved signal

Recovery:

Pause streaming and wait for adapter return.

------------------------------------------------------------------------

## 7. Encoder Crash

Symptoms:

-   clients disconnect
-   no stream

Detection:

Process watchdog.

Recovery:

Restart encoder.

------------------------------------------------------------------------

## 8. Kernel Bluetooth Bugs

Some kernels introduce regressions.

Detection:

    uname -r

Mitigation:

Apply version specific workarounds.

------------------------------------------------------------------------

## 9. Safari Playback Stall

Safari pauses when buffer underflows.

Mitigation:

Listen for `waiting` event and resume playback.

------------------------------------------------------------------------

## 10. Multiple Device Conflict

Two devices connect simultaneously.

Policy:

First connected device wins.

Additional devices are rejected.

------------------------------------------------------------------------

## Conclusion

Handling these scenarios automatically dramatically increases system
reliability.
