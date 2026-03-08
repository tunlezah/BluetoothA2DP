//! WebSocket handler for real-time UI updates.
//!
//! Clients connect to `/ws/status` and receive JSON-encoded `SystemEvent`
//! messages whenever state changes occur.
//!
//! On connect, a full `StateSnapshot` is sent so the client can
//! initialise its UI without a separate REST call.

use axum::extract::ws::{Message, WebSocket};
use axum::{
    extract::{State, WebSocketUpgrade},
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::state::{AppStateHandle, SystemEvent};

/// WebSocket upgrade handler.
///
/// Upgrades the HTTP connection to a WebSocket and spawns a task to
/// handle bidirectional messaging.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppStateHandle>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Handle an individual WebSocket connection.
async fn handle_socket(socket: WebSocket, state: AppStateHandle) {
    let (mut sender, mut receiver) = socket.split();

    // Subscribe to broadcast events before sending the snapshot
    // to avoid missing events that occur between snapshot and subscribe
    let mut event_rx = state.subscribe();

    // Send initial state snapshot
    {
        let app_state = state.state.read().await;
        let snapshot = app_state.snapshot_event();
        drop(app_state);

        match serde_json::to_string(&snapshot) {
            Ok(json) => {
                if sender.send(Message::Text(json)).await.is_err() {
                    return; // Client disconnected
                }
            }
            Err(e) => {
                tracing::warn!("Failed to serialise state snapshot: {}", e);
            }
        }
    }

    tracing::debug!("WebSocket client connected, snapshot sent");

    // Forward broadcast events to this client
    let mut send_task = tokio::spawn(async move {
        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    match serde_json::to_string(&event) {
                        Ok(json) => {
                            if sender.send(Message::Text(json)).await.is_err() {
                                break; // Client disconnected
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to serialise event: {}", e);
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("WebSocket client lagged by {} events", n);
                    // Send a snapshot to resync
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // Receive messages from the client (ping/pong, close frames)
    let mut recv_task = tokio::spawn(async move {
        while let Some(msg) = receiver.next().await {
            match msg {
                Ok(Message::Close(_)) => break,
                Ok(Message::Ping(data)) => {
                    // Pong is handled automatically by axum
                }
                Ok(_) => {} // Ignore other messages from client
                Err(e) => {
                    tracing::debug!("WebSocket receive error: {}", e);
                    break;
                }
            }
        }
    });

    // Wait for either task to finish, then abort the other
    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }

    tracing::debug!("WebSocket client disconnected");
}
