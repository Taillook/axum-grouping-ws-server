use axum::{
    extract,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::Path,
    response::IntoResponse,
};
use futures::{sink::SinkExt, stream::StreamExt};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};

#[derive(Deserialize, Clone)]
pub struct Params {
    user_id: String,
    group_id: String,
}

pub async fn handler(
    Path(params): Path<Params>,
    ws: WebSocketUpgrade,
    extract::Extension(state): extract::Extension<Arc<super::super::AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| websocket(socket, state, params))
}

async fn websocket(stream: WebSocket, state: Arc<super::super::AppState>, params: Params) {
    let (sender, mut receiver) = stream.split();
    let sender = Arc::new(Mutex::new(sender));
    let broadcast_sender = sender.clone();

    let tx = setup_sender(&state, &params.group_id).await;

    let user_id = params.clone().user_id;
    let group_id = params.clone().group_id;

    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(message)) = receiver.next().await {
            match message.clone() {
                Message::Text(message) => {
                    let _drop = state
                        .nc
                        .publish(&group_id, format!("{}: {}", user_id, message))
                        .await;
                }
                Message::Binary(_) => {}
                Message::Ping(ping) => {
                    if sender.lock().await.send(Message::Pong(ping)).await.is_err() {
                        break;
                    }
                }
                Message::Pong(_) => {}
                Message::Close(_) => return,
            }
        }
    });

    let mut send_task = tokio::spawn(async move {
        let mut rx = tx.clone().subscribe();
        while let Ok(message) = rx.recv().await {
            if broadcast_sender
                .lock()
                .await
                .send(Message::Text(message))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    }
}

async fn setup_sender(state: &super::super::AppState, group_id: &str) -> broadcast::Sender<String> {
    let mut group_list = state.group_list.lock().await;
    let group_id = group_id.to_string();

    group_list
        .entry(group_id)
        .or_insert_with(|| broadcast::channel(100).0)
        .clone()
}
