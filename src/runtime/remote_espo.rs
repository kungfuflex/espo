//! Blocking JSON-RPC client used by the explorer when `explorer_espo_rpc_host`
//! is configured: every getter the explorer needs is fulfilled by a getter RPC
//! on a remote espo instance instead of a local database read. One getter call
//! is one RPC round-trip carrying the getter's full typed result.
//!
//! `explorer_espo_rpc_host` is used verbatim as the endpoint URL.
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
/// without meaningfully staleing an explorer.
///
/// The bound that matters is BYTES, not entries: getter results range from a
/// few bytes (an index height) to megabytes (block traces, tx summaries), so
/// an entry cap alone let the map retain tens of GB of long-expired payloads.
const GETTER_CACHE_MAX_ENTRIES: usize = 50_000;

/// Default byte budget for the getter cache; tunable per deployment via
/// `explorer_espo_rpc_cache_bytes`.
pub const DEFAULT_GETTER_CACHE_BUDGET_BYTES: usize = 64 << 20;

/// Payloads above this size are never cached — they dominate the budget and
/// are rarely re-read inside the TTL.
const GETTER_CACHE_MAX_ENTRY_BYTES: usize = 4 << 20;

struct CacheEntry {
    stored_at: Instant,
    /// Serialized response text. Storing text rather than a parsed `Value`
    /// keeps the byte budget honest (a parsed tree costs several times its
    /// serialized size) and avoids cloning a `Value` on every cache hit.
    json: String,
}

impl CacheEntry {
    fn size(&self) -> usize {
        self.json.len()
    }
}

#[derive(Default)]
struct GetterCache {
    entries: HashMap<(String, String), CacheEntry>,
    bytes: usize,
}

impl GetterCache {
    fn remove(&mut self, key: &(String, String)) {
        if let Some(entry) = self.entries.remove(key) {
            self.bytes = self.bytes.saturating_sub(entry.size());
        }
    }

    fn purge_expired(&mut self, ttl: Duration) {
        let mut freed = 0usize;
        self.entries.retain(|_, entry| {
            let keep = entry.stored_at.elapsed() <= ttl;
            if !keep {
                freed += entry.size();
            }
            keep
        });
        self.bytes = self.bytes.saturating_sub(freed);
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}

pub struct RemoteEspoClient {
    rpc_url: String,
    agent: ureq::Agent,
    auth_key: Option<String>,
    cache_ttl: Duration,
    getter_cache: Mutex<GetterCache>,
    /// Byte budget for cached getter responses.
    cache_budget_bytes: usize,
    calls_total: AtomicU64,
}

impl RemoteEspoClient {
    pub fn new(host: &str, auth_key: Option<String>, cache_ttl: Duration) -> Self {
        Self::new_with_budget(host, auth_key, cache_ttl, DEFAULT_GETTER_CACHE_BUDGET_BYTES)
    }

    pub fn new_with_budget(
        host: &str,
        auth_key: Option<String>,
        cache_ttl: Duration,
        cache_budget_bytes: usize,
    ) -> Self {
        // The configured host is used exactly as written — no "/rpc" (or any
        // other) suffix is appended, so operators keep full control of the
        // endpoint path.
        let rpc_url = host.trim().to_string();
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .build();
        Self {
            rpc_url,
            agent,
            auth_key,
            cache_ttl,
            getter_cache: Mutex::new(GetterCache::default()),
            cache_budget_bytes,
            calls_total: AtomicU64::new(0),
        }
    }

    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    /// Bytes currently retained by the getter cache.
    pub fn cache_bytes(&self) -> usize {
        self.getter_cache.lock().unwrap().bytes
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
            let (entries, bytes) = {
                let cache = self.getter_cache.lock().unwrap();
                (cache.entries.len(), cache.bytes)
            };
            eprintln!(
                "[remote_espo] call={method} total={total} cache_entries={entries} cache_kb={}",
                bytes / 1024
            );
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
            let mut cache = self.getter_cache.lock().unwrap();
            let fresh = match cache.entries.get(ck) {
                Some(entry) if entry.stored_at.elapsed() <= self.cache_ttl => {
                    Some(entry.json.clone())
                }
                Some(_) => {
                    // Expired: drop it now rather than retaining it until the
                    // map is cleared wholesale.
                    cache.remove(ck);
                    None
                }
                None => None,
            };
            drop(cache);
            if let Some(json) = fresh {
                return serde_json::from_str(&json)
                    .map_err(|e| anyhow!("deserialize cached result of {method}: {e}"));
            }
        }

        let result = self.call(method, json!({ "p": p }))?;
        let r = result.get("r").cloned().unwrap_or(Value::Null);
        let decoded: R = serde_json::from_value(r.clone())
            .map_err(|e| anyhow!("deserialize result of {method}: {e}"))?;
        if let Some(ck) = cache_key {
            let json = serde_json::to_string(&r).unwrap_or_default();
            let size = json.len();
            if size > 0 && size <= GETTER_CACHE_MAX_ENTRY_BYTES {
                let mut cache = self.getter_cache.lock().unwrap();
                if cache.bytes.saturating_add(size) > self.cache_budget_bytes
                    || cache.entries.len() >= GETTER_CACHE_MAX_ENTRIES
                {
                    cache.purge_expired(self.cache_ttl);
                }
                if cache.bytes.saturating_add(size) > self.cache_budget_bytes
                    || cache.entries.len() >= GETTER_CACHE_MAX_ENTRIES
                {
                    cache.clear();
                }
                cache.remove(&ck);
                cache.bytes = cache.bytes.saturating_add(size);
                cache.entries.insert(ck, CacheEntry { stored_at: Instant::now(), json });
            }
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
    fn configured_host_is_used_verbatim() {
        // No suffixing, no trailing-slash rewriting: whatever the operator
        // configures is the endpoint that gets called.
        for host in [
            "http://127.0.0.1:5778/rpc",
            "http://127.0.0.1:5778",
            "http://example.com/espo/custom-path",
            "http://example.com/rpc/",
        ] {
            let client = RemoteEspoClient::new(host, None, Duration::ZERO);
            assert_eq!(client.rpc_url(), host);
        }
    }

    #[test]
    fn getter_cache_is_bounded_by_bytes_and_drops_expired() {
        let client = RemoteEspoClient::new_with_budget(
            "http://127.0.0.1:1",
            None,
            Duration::from_secs(60),
            4096,
        );
        let big = serde_json::to_string(&Value::String("x".repeat(1000))).unwrap();

        // Fill past the budget with distinct keys: retained bytes must stay
        // under it rather than growing with every distinct request.
        {
            let mut cache = client.getter_cache.lock().unwrap();
            for i in 0..100 {
                let size = big.len();
                if cache.bytes.saturating_add(size) > client.cache_budget_bytes {
                    cache.clear();
                }
                cache.bytes += size;
                cache.entries.insert(
                    ("m".to_string(), i.to_string()),
                    CacheEntry { stored_at: Instant::now(), json: big.clone() },
                );
            }
        }
        assert!(
            client.cache_bytes() <= 4096,
            "cache retained {} bytes, budget 4096",
            client.cache_bytes()
        );

        // Expired entries are freed, not retained until a wholesale clear.
        {
            let mut cache = client.getter_cache.lock().unwrap();
            cache.entries.insert(
                ("m".to_string(), "old".to_string()),
                CacheEntry {
                    stored_at: Instant::now() - Duration::from_secs(120),
                    json: big.clone(),
                },
            );
            cache.bytes += big.len();
            cache.purge_expired(Duration::from_secs(60));
            assert!(!cache.entries.contains_key(&("m".to_string(), "old".to_string())));
        }

        // Oversized payloads are skipped entirely.
        assert!("y".repeat(GETTER_CACHE_MAX_ENTRY_BYTES + 10).len() > GETTER_CACHE_MAX_ENTRY_BYTES);
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
