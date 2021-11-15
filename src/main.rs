use axum::{
    body::Bytes,
    http::{HeaderMap, Request, Response},
    routing::get,
    AddExtensionLayer, Router,
};
use std::{collections::HashMap, env, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::{broadcast, Mutex};
use tower_http::{classify::ServerErrorsFailureClass, trace::TraceLayer};
use tracing::Span;
mod handler;

pub struct AppState {
    group_list: Mutex<HashMap<String, broadcast::Sender<String>>>,
    nc: nats::asynk::Connection,
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let nats_host = env::var("NATS_HOST").expect("NATS_HOST is not defined");
    let nc = match nats::asynk::connect(&nats_host).await {
        Ok(nc) => nc,
        Err(e) => panic!("{:?}", e),
    };

    let group_list = Mutex::new(HashMap::new());

    let app_state = Arc::new(AppState { group_list, nc });

    let addr = SocketAddr::from(([0, 0, 0, 0], 8088));

    let mut s_task = gen_server_task(app_state.clone(), addr);
    let mut nc_task = gen_nc_task(app_state.clone());
    tracing::info!("listening on {}", addr);
    tokio::select! {
        _ = (&mut nc_task) => s_task.abort(),
        _ = (&mut s_task) => nc_task.abort(),
    }
}

fn app(app_state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/websocket/:group_id/:user_id",
            get(handler::websocket::handler),
        )
        .layer(AddExtensionLayer::new(app_state))
        .layer(
            TraceLayer::new_for_http()
                .on_request(|request: &Request<_>, _span: &Span| {
                    tracing::debug!("started {} {}", request.method(), request.uri().path())
                })
                .on_response(|_response: &Response<_>, latency: Duration, _span: &Span| {
                    tracing::debug!("response generated in {:?}", latency)
                })
                .on_body_chunk(|chunk: &Bytes, _latency: Duration, _span: &Span| {
                    tracing::debug!("sending {} bytes", chunk.len())
                })
                .on_eos(
                    |_trailers: Option<&HeaderMap>, stream_duration: Duration, _span: &Span| {
                        tracing::debug!("stream closed after {:?}", stream_duration)
                    },
                )
                .on_failure(
                    |error: ServerErrorsFailureClass, _latency: Duration, _span: &Span| {
                        tracing::debug!("something went wrong: {:?}", error)
                    },
                ),
        )
}

fn gen_nc_task(app_state: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let sub = match app_state.nc.subscribe("*").await {
            Ok(sub) => sub,
            Err(e) => panic!("{:?}", e),
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
    })
}

fn gen_server_task(app_state: Arc<AppState>, addr: SocketAddr) -> tokio::task::JoinHandle<()> {
    let app = app(app_state);
    tokio::spawn(async move {
        if let Err(e) = axum::Server::bind(&addr)
            .serve(app.into_make_service())
            .await
        {
            panic!("{:?}", e)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::SinkExt;
    use futures::StreamExt;
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    #[tokio::test]
    async fn connect_websocket() {
        let nats_host = env::var("NATS_HOST").expect("NATS_HOST is not defined");
        let nc = match nats::asynk::connect(&nats_host).await {
            Ok(nc) => nc,
            Err(e) => panic!("{:?}", e),
        };
        let group_list = Mutex::new(HashMap::new());
        let app_state = Arc::new(AppState { group_list, nc });
        let addr = SocketAddr::from(([0, 0, 0, 0], 8088));

        gen_server_task(app_state.clone(), addr);
        gen_nc_task(app_state.clone());

        let url =
            url::Url::parse("ws://localhost:8088/websocket/group1/user1").expect("Can't parse url");
        let (ws_stream, _) = connect_async(url).await.expect("Failed to connect");
        let (mut write, mut read) = ws_stream.split();

        write
            .send(Message::Text(format!("test")))
            .await
            .expect("Failed to send message");
        if let Some(Ok(message)) = read.next().await {
            assert_eq!(message, Message::Text(format!("user1: test")));
        }
    }
}
