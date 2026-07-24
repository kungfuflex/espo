//! Blocking JSON-RPC client used by the explorer when `explorer_espo_rpc_host`
//! is configured: every getter the explorer needs is fulfilled by a getter RPC
//! on a remote espo instance instead of a local database read. One getter call
//! is one RPC round-trip carrying the getter's full typed result.
//!
//! `internal.*` methods additionally carry the shared `explorer_espo_rpc_key`
//! as `"auth"`; the remote (running with `enable_internal_rpc: true` and
//! `internal_rpc_key`) rejects them otherwise.
//!
//! The client is deliberately synchronous (ureq) so it can be called from the
//! same call sites that previously performed synchronous RocksDB reads.

use anyhow::{Result, anyhow};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Bounded, short-TTL memo for getter responses: SSR pages re-issue the same
/// hot getters (icons, inspections, counters, index heights) on every page
/// view, so a couple of seconds of reuse collapses most repeat round-trips
/// without meaningfully staleing an explorer. Cleared wholesale at the cap.
const GETTER_CACHE_MAX_ENTRIES: usize = 50_000;

pub struct RemoteEspoClient {
    rpc_url: String,
    agent: ureq::Agent,
    auth_key: Option<String>,
    cache_ttl: Duration,
    getter_cache: Mutex<HashMap<(String, String), (Instant, Value)>>,
    calls_total: AtomicU64,
}

impl RemoteEspoClient {
    pub fn new(host: &str, auth_key: Option<String>, cache_ttl: Duration) -> Self {
        let trimmed = host.trim_end_matches('/');
        let rpc_url =
            if trimmed.ends_with("/rpc") { trimmed.to_string() } else { format!("{trimmed}/rpc") };
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .build();
        Self {
            rpc_url,
            agent,
            auth_key,
            cache_ttl,
            getter_cache: Mutex::new(HashMap::new()),
            calls_total: AtomicU64::new(0),
        }
    }

    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    /// Total JSON-RPC round-trips issued by this client (cache hits excluded).
    pub fn total_calls(&self) -> u64 {
        self.calls_total.load(Ordering::Relaxed)
    }

    /// Raw JSON-RPC call. Attaches the auth key to `internal.*` methods.
    /// Returns the `result` value; JSON-RPC errors and `{ok:false}` results
    /// surface as errors.
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let mut params = params;
        if let Some(key) = &self.auth_key {
            if method.starts_with("internal.") {
                params["auth"] = json!(key);
            }
        }
        let total = self.calls_total.fetch_add(1, Ordering::Relaxed) + 1;
        if std::env::var_os("ESPO_REMOTE_RPC_LOG_CALLS").is_some() {
            eprintln!("[remote_espo] call={method} total={total}");
        }
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let response = self
            .agent
            .post(&self.rpc_url)
            .set("Content-Type", "application/json")
            .send_json(body)
            .map_err(|e| anyhow!("remote espo rpc {method} failed: {e}"))?;
        let parsed: Value = response
            .into_json()
            .map_err(|e| anyhow!("remote espo rpc {method}: invalid json: {e}"))?;
        if let Some(err) = parsed.get("error") {
            if !err.is_null() {
                return Err(anyhow!("remote espo rpc {method}: {err}"));
            }
        }
        let result = parsed.get("result").cloned().unwrap_or(Value::Null);
        if result.get("ok").and_then(Value::as_bool) == Some(false) {
            let detail = result.get("error").and_then(Value::as_str).unwrap_or("unknown_error");
            let extra = result.get("detail").and_then(Value::as_str).unwrap_or("");
            return Err(anyhow!("remote espo rpc {method}: {detail} {extra}"));
        }
        Ok(result)
    }

    /// Invoke a remote getter RPC: the getter's native params struct is sent
    /// as `"p"` and its native result struct comes back as `"r"`, both via
    /// serde, so the caller receives exactly what the local getter would have
    /// returned.
    pub fn getter<P: Serialize, R: DeserializeOwned>(&self, method: &str, params: &P) -> Result<R> {
        let p = serde_json::to_value(params)
            .map_err(|e| anyhow!("serialize params for {method}: {e}"))?;

        let cache_key = (!self.cache_ttl.is_zero()).then(|| (method.to_string(), p.to_string()));
        if let Some(ck) = &cache_key {
            if let Some((stored_at, r)) = self.getter_cache.lock().unwrap().get(ck) {
                if stored_at.elapsed() <= self.cache_ttl {
                    return serde_json::from_value(r.clone())
                        .map_err(|e| anyhow!("deserialize cached result of {method}: {e}"));
                }
            }
        }

        let result = self.call(method, json!({ "p": p }))?;
        let r = result.get("r").cloned().unwrap_or(Value::Null);
        let decoded: R = serde_json::from_value(r.clone())
            .map_err(|e| anyhow!("deserialize result of {method}: {e}"))?;
        if let Some(ck) = cache_key {
            let mut cache = self.getter_cache.lock().unwrap();
            if cache.len() >= GETTER_CACHE_MAX_ENTRIES {
                cache.clear();
            }
            cache.insert(ck, (Instant::now(), r));
        }
        Ok(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct DemoParams {
        blockhash: crate::runtime::state_at::StateAt,
        alkane: crate::schemas::SchemaAlkaneId,
    }

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct DemoResult {
        count: u64,
    }

    /// One-shot mock JSON-RPC server: asserts method + auth + params shape,
    /// replies with a canned getter result, then exits. A second network hit
    /// would block on accept() and fail the test via the canned single reply.
    fn spawn_mock(expected_method: &'static str, reply_r: serde_json::Value) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            let body = loop {
                let n = stream.read(&mut tmp).expect("read");
                buf.extend_from_slice(&tmp[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&buf[..pos]).to_string();
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    let start = pos + 4;
                    while buf.len() < start + content_length {
                        let n = stream.read(&mut tmp).expect("read body");
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    break buf[start..start + content_length].to_vec();
                }
            };
            let request: Value = serde_json::from_slice(&body).expect("request json");
            assert_eq!(request["method"].as_str(), Some(expected_method));
            assert_eq!(request["params"]["auth"].as_str(), Some("sekrit"));
            assert_eq!(request["params"]["p"]["alkane"]["block"].as_u64(), Some(2));
            let reply = serde_json::to_string(
                &json!({ "jsonrpc": "2.0", "id": 1, "result": { "ok": true, "r": reply_r } }),
            )
            .unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.len(),
                reply
            );
            stream.write_all(response.as_bytes()).expect("write");
        });
        format!("http://{addr}")
    }

    #[test]
    fn getter_round_trip_carries_auth_and_caches_repeats() {
        let host = spawn_mock("internal.essentials_get_holders_count", json!({ "count": 42 }));
        let client =
            RemoteEspoClient::new(&host, Some("sekrit".to_string()), Duration::from_secs(60));
        let params = DemoParams {
            blockhash: crate::runtime::state_at::StateAt::Latest,
            alkane: crate::schemas::SchemaAlkaneId { block: 2, tx: 0 },
        };

        let first: DemoResult = client
            .getter("internal.essentials_get_holders_count", &params)
            .expect("first call");
        assert_eq!(first, DemoResult { count: 42 });
        assert_eq!(client.total_calls(), 1);

        // Served from the TTL cache — the one-shot mock would hang otherwise.
        let second: DemoResult = client
            .getter("internal.essentials_get_holders_count", &params)
            .expect("cached call");
        assert_eq!(second, DemoResult { count: 42 });
        assert_eq!(client.total_calls(), 1);
    }

    #[test]
    fn getter_surfaces_remote_errors() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut tmp = [0u8; 4096];
            let _ = stream.read(&mut tmp);
            let reply = serde_json::to_string(&json!({
                "jsonrpc": "2.0", "id": 1,
                "result": { "ok": false, "error": "unauthorized", "hint": "missing or invalid auth key" }
            }))
            .unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.len(),
                reply
            );
            let _ = stream.write_all(response.as_bytes());
        });
        let client = RemoteEspoClient::new(&format!("http://{addr}"), None, Duration::ZERO);
        let err = client
            .getter::<_, DemoResult>(
                "internal.essentials_get_holders_count",
                &DemoParams {
                    blockhash: crate::runtime::state_at::StateAt::Latest,
                    alkane: crate::schemas::SchemaAlkaneId { block: 2, tx: 0 },
                },
            )
            .expect_err("must surface remote rejection");
        assert!(err.to_string().contains("unauthorized"), "got: {err}");
    }

    #[test]
    fn borsh_hex_round_trips_storage_types() {
        use crate::runtime::internal_rpc::{borsh_hex, borsh_unhex};
        let pairs: Vec<(crate::schemas::SchemaAlkaneId, u128)> = vec![
            (crate::schemas::SchemaAlkaneId { block: 2, tx: 77627 }, 293_280_009_999u128),
            (crate::schemas::SchemaAlkaneId { block: 4, tx: 797 }, u128::MAX),
        ];
        let encoded = borsh_hex(&pairs).expect("encode");
        let decoded: Vec<(crate::schemas::SchemaAlkaneId, u128)> =
            borsh_unhex(&encoded).expect("decode");
        assert_eq!(decoded, pairs);
    }
}
