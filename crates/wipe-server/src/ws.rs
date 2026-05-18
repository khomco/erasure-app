//! WebSocket endpoint that streams live Job broadcasts + fleet events.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use serde::Serialize;
use tracing::debug;

use crate::app::AppState;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsEnvelope {
    /// One of: `JobStateChanged`, `ActivityAdded`, `ErasureUpdate` —
    /// see `wipe_engine::JobBroadcast`. Wire shape: the inner tagged
    /// enum is embedded under `payload`.
    JobBroadcast {
        payload: wipe_engine::JobBroadcast,
    },
    FleetEvent(wipe_fleet::FleetEvent),
    Hello { tool_version: String },
    Heartbeat,
}

pub async fn ws_events(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut job_rx = state.runner.subscribe();
    let mut fleet_rx_opt = state.fleet.as_ref().map(|f| f.subscribe());

    // Greet the client so they can immediately render version info.
    let hello = WsEnvelope::Hello {
        tool_version: state.tool_version.clone(),
    };
    if send_json(&mut sender, &hello).await.is_err() {
        return;
    }

    // Drain any client-originated messages in the background (ping/pong, close).
    let drain = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if matches!(msg, Message::Close(_)) {
                break;
            }
        }
    });

    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(15));

    loop {
        tokio::select! {
            r = job_rx.recv() => match r {
                Ok(payload) => {
                    if send_json(&mut sender, &WsEnvelope::JobBroadcast { payload }).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    debug!(?e, "job event channel closed");
                    break;
                }
            },
            r = async {
                match fleet_rx_opt.as_mut() {
                    Some(rx) => rx.recv().await.ok(),
                    None => futures::future::pending().await,
                }
            } => {
                if let Some(ev) = r {
                    if send_json(&mut sender, &WsEnvelope::FleetEvent(ev)).await.is_err() {
                        break;
                    }
                }
            }
            _ = heartbeat.tick() => {
                if send_json(&mut sender, &WsEnvelope::Heartbeat).await.is_err() {
                    break;
                }
            }
        }
    }
    drain.abort();
}

async fn send_json<S: SinkExt<Message> + Unpin>(
    sender: &mut S,
    env: &WsEnvelope,
) -> Result<(), ()>
where
    <S as futures::Sink<Message>>::Error: std::fmt::Display,
{
    let json = serde_json::to_string(env).map_err(|_| ())?;
    sender.send(Message::Text(json)).await.map_err(|_| ())
}
