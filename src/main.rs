use axum::{prelude::*, AddExtensionLayer};
use std::{collections::HashMap, env, net::SocketAddr, sync::Arc};
use tokio::sync::{broadcast, Mutex};
mod handler;

pub struct AppState {
    group_list: Mutex<HashMap<String, broadcast::Sender<String>>>,
    nc: nats::asynk::Connection,
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

    let app = route(
        "/websocket/:group_id/:user_id",
        get(handler::websocket::handler),
    )
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
