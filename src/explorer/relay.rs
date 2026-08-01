//! Client-mode live-data relay: when this instance renders from a remote espo
//! (`explorer_espo_rpc_host`) it has no local mempool service or indexer, so
//! the live surfaces are served from the DATA instance instead:
//!
//! - the events websocket becomes a per-client bidirectional pipe to the data
//!   instance's events websocket, so subscriptions, mempool-block detail
//!   queries, tx-status pushes and heartbeats behave exactly as they would
//!   against the data instance itself;
//! - the mempool-blocks JSON endpoint becomes an HTTP proxy of the data
//!   instance's endpoint.
//!
//! Both use `explorer_espo_events_host` — the data espo's EXPLORER base URL
//! (the websocket and mempool APIs live on the explorer port, not /rpc).

use axum::extract::ws::{Message as AxumMessage, WebSocket};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

/// The data instance's events websocket URL derived from the events host.
pub fn upstream_events_ws_url(events_host: &str) -> String {
    let base = events_host.trim_end_matches('/');
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        format!("ws://{base}")
    };
    format!("{ws_base}/api/events/ws")
}

fn upstream_mempool_url(events_host: &str, path: &str) -> String {
    format!("{}{}", events_host.trim_end_matches('/'), path)
}

/// Bidirectionally pipe one local websocket client to the data instance's
/// events websocket. Every frame passes through untouched in both directions,
/// so the upstream protocol (hello, subscriptions, projected-block detail
/// requests, pings) works with full fidelity.
pub async fn piped_events_socket(local: WebSocket, events_host: String) {
    let url = upstream_events_ws_url(&events_host);
    let upstream = match connect_async(&url).await {
        Ok((stream, _)) => stream,
        Err(e) => {
            eprintln!("[relay] upstream events connect failed ({url}): {e}");
            return;
        }
    };

    let (mut upstream_tx, mut upstream_rx) = upstream.split();
    let (mut local_tx, mut local_rx) = local.split();

    let to_client = async {
        while let Some(frame) = upstream_rx.next().await {
            let forwarded = match frame {
                Ok(TungsteniteMessage::Text(text)) => {
                    local_tx.send(AxumMessage::Text(text.as_str().into())).await
                }
                Ok(TungsteniteMessage::Binary(bytes)) => {
                    local_tx.send(AxumMessage::Binary(bytes.into())).await
                }
                Ok(TungsteniteMessage::Ping(payload)) => {
                    local_tx.send(AxumMessage::Ping(payload.into())).await
                }
                Ok(TungsteniteMessage::Pong(payload)) => {
                    local_tx.send(AxumMessage::Pong(payload.into())).await
                }
                Ok(TungsteniteMessage::Close(_)) | Err(_) => break,
                Ok(TungsteniteMessage::Frame(_)) => Ok(()),
            };
            if forwarded.is_err() {
                break;
            }
        }
    };

    let to_upstream = async {
        while let Some(frame) = local_rx.next().await {
            let forwarded = match frame {
                Ok(AxumMessage::Text(text)) => {
                    upstream_tx.send(TungsteniteMessage::Text(text.as_str().into())).await
                }
                Ok(AxumMessage::Binary(bytes)) => {
                    upstream_tx.send(TungsteniteMessage::Binary(bytes.into())).await
                }
                Ok(AxumMessage::Ping(payload)) => {
                    upstream_tx.send(TungsteniteMessage::Ping(payload.into())).await
                }
                Ok(AxumMessage::Pong(payload)) => {
                    upstream_tx.send(TungsteniteMessage::Pong(payload.into())).await
                }
                Ok(AxumMessage::Close(_)) | Err(_) => break,
            };
            if forwarded.is_err() {
                break;
            }
        }
    };

    // Either side closing tears down the pipe.
    tokio::select! {
        _ = to_client => {}
        _ = to_upstream => {}
    }
}

/// Proxy a rendered explorer page from the data instance.
///
/// Used for the projected-mempool-block page: it renders from live mempool
/// service state (raw transactions, protostones, traces) that a client-mode
/// instance deliberately does not run, so the page is served by the instance
/// that owns that state — the same relay approach as the events websocket and
/// the mempool-blocks snapshot.
pub async fn proxy_explorer_page(events_host: &str, path_and_query: &str) -> Response {
    let url = format!("{}{}", events_host.trim_end_matches('/'), path_and_query);
    let response = match reqwest::get(&url).await {
        Ok(response) => response,
        Err(e) => {
            eprintln!("[relay] upstream page fetch failed ({url}): {e}");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("text/html; charset=utf-8")
        .to_string();
    match response.bytes().await {
        Ok(body) => (status, [(axum::http::header::CONTENT_TYPE, content_type)], body.to_vec())
            .into_response(),
        Err(e) => {
            eprintln!("[relay] upstream page body failed ({url}): {e}");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

/// Proxy the data instance's mempool-blocks snapshot.
pub async fn proxy_mempool_blocks(events_host: &str) -> Response {
    proxy_mempool_json(events_host, "/api/mempool/blocks").await
}

/// Proxy one of the data instance's mempool JSON endpoints verbatim.
pub async fn proxy_mempool_json(events_host: &str, path: &str) -> Response {
    let url = upstream_mempool_url(events_host, path);
    let response = match reqwest::get(&url).await {
        Ok(response) => response,
        Err(e) => {
            eprintln!("[relay] upstream mempool blocks fetch failed ({url}): {e}");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    match response.bytes().await {
        Ok(body) => {
            (status, [(axum::http::header::CONTENT_TYPE, "application/json")], body.to_vec())
                .into_response()
        }
        Err(e) => {
            eprintln!("[relay] upstream mempool blocks body failed ({url}): {e}");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}
