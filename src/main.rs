use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::Path,
    prelude::*,
    response::IntoResponse,
    AddExtensionLayer,
};
use futures::{sink::SinkExt, stream::StreamExt};
use serde::Deserialize;
use std::{collections::HashMap, env, net::SocketAddr, sync::Arc};
use tokio::sync::{broadcast, Mutex};

struct AppState {
    group_list: Mutex<HashMap<String, broadcast::Sender<String>>>,
    nc: nats::asynk::Connection,
}

#[derive(Deserialize, Clone)]
struct Params {
    user_id: String,
    group_id: String,
}

#[tokio::main]
async fn main() {
    let nats_host = env::var("NATS_HOST").expect("NATS_HOST is not defined");
    let nc = match nats::asynk::connect(&nats_host).await {
        Ok(nc) => nc,
        Err(_) => return,
    };

    let group_list = Mutex::new(HashMap::new());

    let app_state = Arc::new(AppState { group_list, nc });

    let app = route("/websocket/:group_id/:user_id", get(websocket_handler))
        .layer(AddExtensionLayer::new(app_state.clone()));

    let addr = SocketAddr::from(([0, 0, 0, 0], 8088));

    let mut nc_task = tokio::spawn(async move {
        let sub = match app_state.nc.subscribe("*").await {
            Ok(sub) => sub,
            Err(_) => return,
        };
        while let Some(msg) = sub.next().await {
            let mut group_list = app_state.group_list.lock().await;

            if let Some(group) = group_list.get_mut(&msg.subject) {
                let converted: String = match String::from_utf8(msg.data) {
                    Ok(v) => v,
                    Err(e) => e.to_string(),
                };
                let _drop = group.send(converted.clone());
            }
        }
    });

    let mut s_task = tokio::spawn(async move {
        axum::Server::bind(&addr)
            .serve(app.into_make_service())
            .await
            .unwrap();
    });

    tokio::select! {
        _ = (&mut nc_task) => s_task.abort(),
        _ = (&mut s_task) => nc_task.abort(),
    }
}

async fn websocket_handler(
    Path(params): Path<Params>,
    ws: WebSocketUpgrade,
    extract::Extension(state): extract::Extension<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| websocket(socket, state, params))
}

async fn websocket(stream: WebSocket, state: Arc<AppState>, params: Params) {
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

async fn setup_sender(state: &AppState, group_id: &str) -> broadcast::Sender<String> {
    let (tx, _rx) = broadcast::channel(100);
    let mut group_list = state.group_list.lock().await;
    let group_id = group_id.to_string();

    match group_list.get_mut(&group_id) {
        None => {
            group_list.insert(group_id, tx.clone());
            tx
        }
        Some(group) => group.clone(),
    }
}
