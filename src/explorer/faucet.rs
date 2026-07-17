use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use axum::Json;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use bitcoin::{Address, Network};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::{get_config, get_network};

const FORWARDED_IP_HEADERS: [&str; 3] = ["cf-connecting-ip", "x-forwarded-for", "x-real-ip"];

#[derive(Debug, Deserialize)]
pub struct FaucetSendRequest {
    address: String,
    amount: Option<f64>,
}

pub(crate) fn faucet_enabled() -> bool {
    get_network() == Network::Regtest && get_config().b8_faucet_url.is_some()
}

fn faucet_url() -> Option<&'static str> {
    faucet_enabled().then(|| get_config().b8_faucet_url.as_deref()).flatten()
}

fn error_response(status: StatusCode, code: i64, message: &str) -> Response {
    (
        status,
        Json(json!({
            "id": 1,
            "jsonrpc": "2.0",
            "error": {
                "code": code,
                "message": message,
            },
        })),
    )
        .into_response()
}

fn valid_regtest_address(raw: &str) -> bool {
    Address::from_str(raw)
        .ok()
        .and_then(|address| address.require_network(Network::Regtest).ok())
        .is_some()
}

fn faucet_send_params(address: &str, amount: Option<f64>) -> Result<Value, &'static str> {
    match amount {
        Some(amount) if !amount.is_finite() || amount < 0.0 => Err("invalid_amount"),
        Some(amount) => Ok(json!([address, amount])),
        None => Ok(json!([address])),
    }
}

async fn call_faucet(
    method: &'static str,
    params: Option<Value>,
    headers: &HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
) -> Result<(StatusCode, Value), &'static str> {
    let Some(url) = faucet_url() else {
        return Err("not_configured");
    };
    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| "client_failed")?;
    let mut body = json!({
        "id": 1,
        "jsonrpc": "2.0",
        "method": method,
    });
    if let Some(params) = params {
        body["params"] = params;
    }
    let mut request = client.post(url).json(&body);

    let mut forwarded_ip = false;
    for name in FORWARDED_IP_HEADERS {
        if let Some(value) = headers.get(name) {
            request = request.header(name, value);
            forwarded_ip = true;
        }
    }
    if !forwarded_ip && let Some(ConnectInfo(peer)) = peer {
        let ip = peer.ip().to_string();
        request = request.header("x-forwarded-for", &ip).header("x-real-ip", ip);
    }

    let response = request.send().await.map_err(|_| "request_failed")?;
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let body = response.json::<Value>().await.map_err(|_| "response_decode_failed")?;
    Ok((status, body))
}

pub async fn faucet_status(headers: HeaderMap, peer: ConnectInfo<SocketAddr>) -> Response {
    match call_faucet("faucet_status", None, &headers, Some(peer)).await {
        Ok((status, body)) => (status, Json(body)).into_response(),
        Err("not_configured") => {
            error_response(StatusCode::NOT_FOUND, -32004, "Faucet is not available")
        }
        Err(_) => {
            error_response(StatusCode::BAD_GATEWAY, -32002, "Unable to reach the faucet service")
        }
    }
}

pub async fn faucet_send(
    headers: HeaderMap,
    peer: ConnectInfo<SocketAddr>,
    Json(payload): Json<FaucetSendRequest>,
) -> Response {
    let address = payload.address.trim();
    if !valid_regtest_address(address) {
        return error_response(StatusCode::BAD_REQUEST, -32602, "Invalid regtest address");
    }
    let params = match faucet_send_params(address, payload.amount) {
        Ok(params) => params,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, -32602, "Invalid faucet amount"),
    };

    match call_faucet("faucet_send", Some(params), &headers, Some(peer)).await {
        Ok((status, body)) => (status, Json(body)).into_response(),
        Err("not_configured") => {
            error_response(StatusCode::NOT_FOUND, -32004, "Faucet is not available")
        }
        Err(_) => {
            error_response(StatusCode::BAD_GATEWAY, -32002, "Unable to reach the faucet service")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{faucet_send_params, valid_regtest_address};
    use bitcoin::{Address, Network, ScriptBuf};

    #[test]
    fn accepts_regtest_address() {
        let script = ScriptBuf::new();
        let address = Address::p2wsh(&script, Network::Regtest).to_string();

        assert!(valid_regtest_address(&address));
    }

    #[test]
    fn rejects_non_regtest_and_malformed_addresses() {
        assert!(!valid_regtest_address("bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh"));
        assert!(!valid_regtest_address("not-an-address"));
    }

    #[test]
    fn faucet_send_amount_is_forwarded_when_provided() {
        assert_eq!(
            faucet_send_params("bcrt1qexample", Some(0.25)).unwrap(),
            serde_json::json!(["bcrt1qexample", 0.25])
        );
        assert_eq!(
            faucet_send_params("bcrt1qexample", None).unwrap(),
            serde_json::json!(["bcrt1qexample"])
        );
        assert!(faucet_send_params("bcrt1qexample", Some(-0.1)).is_err());
    }
}
