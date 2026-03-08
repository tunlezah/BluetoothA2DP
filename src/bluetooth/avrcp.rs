//! AVRCP MediaPlayer1 polling — track metadata and playback status.
//!
//! When a Bluetooth device is connected and supports AVRCP, BlueZ exposes an
//! `org.bluez.MediaPlayer1` object at the device's D-Bus path + `/player0`.
//!
//! This task polls that interface every second and broadcasts `TrackChanged`
//! and `PlaybackStatusChanged` events whenever metadata or status changes.
//!
//! If the device does not support AVRCP, the proxy build will fail silently
//! and the UI will fall back to showing only the device name.

use std::collections::HashMap;
use std::time::Duration;

use zbus::proxy;
use zbus::zvariant::OwnedValue;
use zbus::Connection;

use crate::state::{AppStateHandle, DeviceState, PlaybackStatus, SystemEvent, TrackInfo};

/// D-Bus proxy for `org.bluez.MediaPlayer1`.
///
/// The `Track` property returns `a{sv}` — a string-keyed variant dict.
/// Keys include: Title, Artist, Album, Duration (ms), NumberOfTracks, TrackNumber.
#[proxy(interface = "org.bluez.MediaPlayer1", default_service = "org.bluez")]
trait MediaPlayer1 {
    /// Playback status: "playing" | "stopped" | "paused" | "forward-seek" | "reverse-seek"
    #[zbus(property)]
    fn status(&self) -> zbus::Result<String>;

    /// Track metadata dict (a{sv}).
    #[zbus(property)]
    fn track(&self) -> zbus::Result<HashMap<String, OwnedValue>>;
}

/// Background task: poll AVRCP for track info and playback status.
///
/// Spawned once in main. Loops forever, polling the active device's
/// MediaPlayer1 object and broadcasting state changes.
pub async fn run_avrcp_monitor(state: AppStateHandle) {
    let connection = match Connection::system().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("AVRCP monitor: D-Bus connect failed: {}", e);
            return;
        }
    };

    tracing::info!("AVRCP monitor started");

    let mut last_track: Option<TrackInfo> = None;
    let mut last_status = PlaybackStatus::Unknown;
    let mut interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        interval.tick().await;

        // Find the highest-priority connected device and adapter path
        let (device_addr, adapter_name) = {
            let s = state.state.read().await;
            let addr = s
                .devices
                .values()
                .find(|d| {
                    matches!(
                        d.state,
                        DeviceState::AudioActive
                            | DeviceState::PipewireSourceReady
                            | DeviceState::ProfileNegotiated
                            | DeviceState::Connected
                    )
                })
                .map(|d| d.address.clone());
            let adapter = s.config.adapter.clone();
            (addr, adapter)
        };

        let Some(addr) = device_addr else {
            // No connected device — clear track info if we had any
            if last_track.is_some() || last_status != PlaybackStatus::Unknown {
                last_track = None;
                last_status = PlaybackStatus::Unknown;
                {
                    let mut s = state.state.write().await;
                    s.track_info = None;
                    s.playback_status = PlaybackStatus::Unknown;
                }
                state.broadcast(SystemEvent::TrackChanged { track: None });
                state.broadcast(SystemEvent::PlaybackStatusChanged {
                    status: PlaybackStatus::Unknown,
                });
            }
            continue;
        };

        // Build the MediaPlayer1 D-Bus path:
        //   /org/bluez/<adapter>/dev_AA_BB_CC_DD_EE_FF/player0
        let dev_seg = addr.replace(':', "_");
        let player_path = format!("/org/bluez/{}/dev_{}/player0", adapter_name, dev_seg);

        let builder = match MediaPlayer1Proxy::builder(&connection).path(player_path.as_str()) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let proxy = match builder.build().await {
            Ok(p) => p,
            Err(_) => continue, // Device does not support AVRCP
        };

        // Read playback status
        let new_status = match proxy.status().await {
            Ok(s) => parse_playback_status(&s),
            Err(_) => PlaybackStatus::Unknown,
        };

        // Read track metadata
        let new_track = match proxy.track().await {
            Ok(t) => extract_track_info(&t),
            Err(_) => None,
        };

        let status_changed = new_status != last_status;
        let track_changed = new_track != last_track;

        if status_changed || track_changed {
            {
                let mut s = state.state.write().await;
                if status_changed {
                    s.playback_status = new_status.clone();
                }
                if track_changed {
                    s.track_info = new_track.clone();
                }
            }

            if status_changed {
                tracing::debug!(status = ?new_status, "AVRCP playback status changed");
                state.broadcast(SystemEvent::PlaybackStatusChanged {
                    status: new_status.clone(),
                });
            }
            if track_changed {
                tracing::info!(
                    title = ?new_track.as_ref().and_then(|t| t.title.as_ref()),
                    artist = ?new_track.as_ref().and_then(|t| t.artist.as_ref()),
                    "AVRCP track changed"
                );
                state.broadcast(SystemEvent::TrackChanged {
                    track: new_track.clone(),
                });
            }

            last_status = new_status;
            last_track = new_track;
        }
    }
}

fn parse_playback_status(s: &str) -> PlaybackStatus {
    match s.to_lowercase().as_str() {
        "playing" => PlaybackStatus::Playing,
        "paused" => PlaybackStatus::Paused,
        "stopped" => PlaybackStatus::Stopped,
        _ => PlaybackStatus::Unknown,
    }
}

fn extract_track_info(props: &HashMap<String, OwnedValue>) -> Option<TrackInfo> {
    use zbus::zvariant::Value;

    // Helper: extract a String from a possibly-variant OwnedValue
    let get_str = |key: &str| -> Option<String> {
        props.get(key).and_then(|owned| {
            let s = match &**owned {
                Value::Str(s) => Some(s.to_string()),
                // a{sv} variant wrapping — unwrap one level
                Value::Value(inner) => match inner.as_ref() {
                    Value::Str(s) => Some(s.to_string()),
                    _ => None,
                },
                _ => None,
            };
            s.filter(|s| !s.is_empty())
        })
    };

    let get_u32 = |key: &str| -> Option<u32> {
        props.get(key).and_then(|owned| match &**owned {
            Value::U32(n) => Some(*n),
            Value::Value(inner) => match inner.as_ref() {
                Value::U32(n) => Some(*n),
                _ => None,
            },
            _ => None,
        })
    };

    let title = get_str("Title");
    let artist = get_str("Artist");
    let album = get_str("Album");
    let duration_ms = get_u32("Duration");

    // Only emit a TrackInfo if we have at least one meaningful field
    if title.is_none() && artist.is_none() && album.is_none() {
        None
    } else {
        Some(TrackInfo {
            title,
            artist,
            album,
            duration_ms,
        })
    }
}
