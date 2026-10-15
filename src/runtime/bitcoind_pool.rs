//! A keep-alive bitcoind JSON-RPC client for the indexer's per-transaction
//! lookups (runes etching-commitment checks, delegate inscriptions).
//!
//! The shared `bitcoincore_rpc` client opens a new TCP connection, and resolves
//! the host name again, for every call. The runes module makes these calls one
//! at a time, several per etching, so through the 84x,xxx launch weeks a block
//! cost hundreds of connect + DNS round trips (measured ~60 calls/s, ~17 ms
//! each). This client reuses pooled connections and caches block-header
//! heights by block hash; nothing else is cached.
//!
//! Semantics are the callers' old ones, with one deliberate difference: a
//! TRANSPORT failure (bitcoind unreachable, connection reset, timeout, HTTP 5xx)
//! is retried until it succeeds instead of being read as "no answer". A JSON-RPC
//! error from bitcoind itself (e.g. -5 "No such mempool or blockchain
//! transaction") is still an answer, and callers treat it exactly as before.
//! The old behaviour turned a network blip into "commitment not mature", which
//! would silently drop a valid etching from the index.

use anyhow::{Context, Result, bail};
use bitcoin::consensus::deserialize;
use bitcoin::{BlockHash, ScriptBuf, Transaction, Txid};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Header heights kept by hash. A hash names one header forever, so this can
/// never go stale; the bound only limits memory (~100 B per entry).
const HEADER_CACHE_MAX: usize = 200_000;

pub struct PooledBitcoind {
    url: String,
    auth: Option<String>,
    client: reqwest::blocking::Client,
    header_heights: Mutex<HashMap<BlockHash, u64>>,
}

/// What `commitment_input_is_mature` needs from a verbose transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxOutputsAndBlock {
    /// scriptPubKey of each output, in order (None where it does not decode).
    pub scripts: Vec<Option<ScriptBuf>>,
    /// Confirming block, if the transaction is confirmed.
    pub blockhash: Option<BlockHash>,
}

static POOL: OnceLock<PooledBitcoind> = OnceLock::new();

pub fn get() -> &'static PooledBitcoind {
    POOL.get_or_init(|| {
        let cfg = crate::config::get_config();
        PooledBitcoind::new(&cfg.bitcoind_rpc_url, &cfg.bitcoind_rpc_user, &cfg.bitcoind_rpc_pass)
            .expect("build pooled bitcoind client")
    })
}

enum Answer {
    Result(Value),
    RpcError(Value),
}

impl PooledBitcoind {
    pub fn new(url: &str, user: &str, pass: &str) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .pool_max_idle_per_host(8)
            .pool_idle_timeout(Duration::from_secs(30))
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .context("build pooled bitcoind http client")?;
        let auth = (!user.is_empty() || !pass.is_empty()).then(|| {
            use base64::Engine;
            format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}")))
        });
        Ok(Self { url: url.to_string(), auth, client, header_heights: Mutex::new(HashMap::new()) })
    }

    fn call_once(&self, method: &str, params: &Value) -> Result<Answer> {
        let body = json!({"jsonrpc": "1.0", "id": "espo", "method": method, "params": params});
        let mut req = self.client.post(&self.url).json(&body);
        if let Some(a) = &self.auth {
            req = req.header("authorization", a);
        }
        let resp = req.send().with_context(|| format!("bitcoind {method}: transport"))?;
        let status = resp.status();
        // bitcoind answers JSON-RPC errors with HTTP 404/500 and a JSON body.
        let bytes = resp.bytes().with_context(|| format!("bitcoind {method}: read body"))?;
        let v: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => bail!("bitcoind {method}: HTTP {status}, non-JSON body"),
        };
        match v.get("error").filter(|e| !e.is_null()) {
            Some(err) => {
                // -28 (warming up) is not an answer about the transaction
                if err.get("code").and_then(Value::as_i64) == Some(-28) {
                    bail!("bitcoind {method}: warming up ({err})");
                }
                Ok(Answer::RpcError(err.clone()))
            }
            None => Ok(Answer::Result(v.get("result").cloned().unwrap_or(Value::Null))),
        }
    }

    /// Retries transport failures (never RPC errors) with backoff, forever: a
    /// bitcoind outage stalls the indexer rather than changing what it writes.
    fn call(&self, method: &str, params: Value) -> Answer {
        let mut attempt: u32 = 0;
        loop {
            match self.call_once(method, &params) {
                Ok(a) => return a,
                Err(e) => {
                    attempt += 1;
                    let wait = Duration::from_millis(200u64.saturating_mul(1 << attempt.min(8)));
                    eprintln!("[bitcoind-pool] {e:#}; retry {attempt} in {wait:?}");
                    std::thread::sleep(wait);
                }
            }
        }
    }

    /// `getrawtransaction <txid> true` reduced to output scripts + blockhash.
    /// `None` when bitcoind answers with an error (unknown tx).
    pub fn tx_outputs_and_block(&self, txid: &Txid) -> Option<TxOutputsAndBlock> {
        let Answer::Result(v) = self.call("getrawtransaction", json!([txid.to_string(), true])) else {
            return None;
        };
        parse_tx_outputs_and_block(&v)
    }

    /// `getrawtransaction <txid> false` decoded. `None` on an RPC error or an
    /// undecodable body.
    pub fn raw_transaction(&self, txid: &Txid) -> Option<Transaction> {
        let Answer::Result(v) = self.call("getrawtransaction", json!([txid.to_string(), false])) else {
            return None;
        };
        let raw = hex::decode(v.as_str()?).ok()?;
        deserialize::<Transaction>(&raw).ok()
    }

    /// Height of the block `hash` (cached by hash). `None` on an RPC error.
    pub fn header_height(&self, hash: &BlockHash) -> Option<u64> {
        if let Some(h) = self.header_heights.lock().ok().and_then(|m| m.get(hash).copied()) {
            return Some(h);
        }
        let Answer::Result(v) = self.call("getblockheader", json!([hash.to_string(), true])) else {
            return None;
        };
        let h = v.get("height").and_then(Value::as_u64)?;
        if let Ok(mut m) = self.header_heights.lock() {
            if m.len() >= HEADER_CACHE_MAX {
                m.clear();
            }
            m.insert(*hash, h);
        }
        Some(h)
    }
}

/// The fields of a verbose `getrawtransaction` that the maturity check reads,
/// decoded the same way bitcoincore_rpc's `GetRawTransactionResult` does
/// (`vout[i].scriptPubKey.hex`, top-level `blockhash`).
pub fn parse_tx_outputs_and_block(v: &Value) -> Option<TxOutputsAndBlock> {
    let vout = v.get("vout")?.as_array()?;
    let scripts = vout
        .iter()
        .map(|o| {
            o.get("scriptPubKey")
                .and_then(|s| s.get("hex"))
                .and_then(Value::as_str)
                .and_then(|h| hex::decode(h).ok())
                .map(ScriptBuf::from_bytes)
        })
        .collect();
    let blockhash = match v.get("blockhash") {
        None | Some(Value::Null) => None,
        Some(b) => Some(b.as_str()?.parse::<BlockHash>().ok()?),
    };
    Some(TxOutputsAndBlock { scripts, blockhash })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbose_tx_parsing_matches_the_fields_the_check_reads() {
        let v = json!({
            "txid": "00",
            "vout": [
                {"value": 0.0001, "n": 0, "scriptPubKey": {"hex": "5120" , "type": "witness_v1_taproot"}},
                {"value": 0.0, "n": 1, "scriptPubKey": {"hex": "zz"}}
            ],
            "blockhash": "00000000000000000001b1a3d1b6c2bd7e5a0a0b4dbd6d4e6c2c1c5d4b3a2f1e"
        });
        let p = parse_tx_outputs_and_block(&v).unwrap();
        assert_eq!(p.scripts.len(), 2);
        assert_eq!(p.scripts[0].as_ref().unwrap().as_bytes(), &[0x51, 0x20]);
        assert!(p.scripts[1].is_none(), "an undecodable script is None, like a failed .script()");
        assert!(p.blockhash.is_some());
        let unconfirmed = json!({"vout": [], "blockhash": null});
        assert_eq!(parse_tx_outputs_and_block(&unconfirmed).unwrap().blockhash, None);
    }
}
