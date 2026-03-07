# Production Deployment Guide

This document describes how to deploy the BluetoothA2DP system in a
stable production configuration.

------------------------------------------------------------------------

## Operating System

Recommended:

Ubuntu 24.04 LTS

------------------------------------------------------------------------

## Required Packages

    bluez
    pipewire
    pipewire-pulse
    wireplumber
    ffmpeg

------------------------------------------------------------------------

## Kernel Modules

Enable ALSA loopback:

    modprobe snd-aloop

Add permanently:

    echo snd-aloop >> /etc/modules

------------------------------------------------------------------------

## User Service

Install service file:

    ~/.config/systemd/user/btsink.service

Enable:

    systemctl --user enable btsink
    systemctl --user start btsink

------------------------------------------------------------------------

## Persistent User Session

Enable lingering:

    loginctl enable-linger USER

This keeps PipeWire running without login.

------------------------------------------------------------------------

## Firewall

Allow local web access only.

Example:

    ufw allow from 192.168.0.0/16 to any port 8080

------------------------------------------------------------------------

## Monitoring

Recommended monitoring checks:

-   Bluetooth adapter availability
-   PipeWire node presence
-   encoder process health
