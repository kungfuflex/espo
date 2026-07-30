//! `internal.*` getter RPC methods: for every getter the explorer calls, this
//! namespace serves that getter over JSON-RPC so a remote espo explorer
//! (configured with `explorer_espo_rpc_host`) can render entirely from RPC
//! calls. Each method receives the getter's native params struct as `"p"` and
//! returns its native result struct as `"r"` (serde on both sides), so one
//! getter invocation is exactly one round-trip regardless of result size.
//!
//! Registered only when `enable_internal_rpc` is true, which requires
//! `internal_rpc_key`: every request must carry the key as `"auth"` or it is
//! rejected outright.

use crate::modules::defs::RpcNsRegistrar;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// Encode any Borsh-serializable value as hex for transport inside getter
/// RPC results. Heavy nested result types (records, summaries, blobs) travel
/// this way so their existing storage encoding is the wire contract.
pub fn borsh_hex<T: borsh::BorshSerialize>(value: &T) -> anyhow::Result<String> {
    Ok(hex::encode(borsh::to_vec(value)?))
}

pub fn borsh_unhex<T: borsh::BorshDeserialize>(raw: &str) -> anyhow::Result<T> {
    let bytes = hex::decode(raw)?;
    Ok(T::try_from_slice(&bytes)?)
}

pub fn err(error: &str, hint: &str) -> Value {
    json!({ "ok": false, "error": error, "hint": hint })
}

/// Constant-time-ish comparison of the request key against the configured
/// `internal_rpc_key`. Requests without the correct key are rejected outright.
pub fn check_auth(payload: &Value) -> Result<(), Value> {
    let Some(expected) = crate::config::internal_rpc_key() else {
        // enable_internal_rpc without a key is refused at startup; if we ever
        // get here anyway, fail closed.
        return Err(err("unauthorized", "internal rpc key not configured"));
    };
    let provided = payload.get("auth").and_then(Value::as_str).unwrap_or("");
    let expected_bytes = expected.as_bytes();
    let provided_bytes = provided.as_bytes();
    let mut diff = expected_bytes.len() ^ provided_bytes.len();
    for i in 0..expected_bytes.len() {
        let p = provided_bytes.get(i).copied().unwrap_or(0);
        diff |= (expected_bytes[i] ^ p) as usize;
    }
    if diff != 0 {
        return Err(err("unauthorized", "missing or invalid auth key"));
    }
    Ok(())
}

/// Register one getter as an `internal.*` RPC method. `f` runs the local
/// getter on a blocking thread; params/result travel as serde JSON under
/// `"p"` / `"r"`.
pub fn register_getter<P, R, F>(reg: &RpcNsRegistrar, name: &'static str, f: F)
where
    P: DeserializeOwned + Send + 'static,
    R: Serialize + Send + 'static,
    F: Fn(P) -> anyhow::Result<R> + Send + Sync + Clone + 'static,
{
    let reg = reg.clone();
    tokio::spawn(async move {
        reg.register(name, move |_cx, payload| {
            let f = f.clone();
            async move {
                if let Err(e) = check_auth(&payload) {
                    return e;
                }
                let p_value = payload.get("p").cloned().unwrap_or(Value::Null);
                let params: P = match serde_json::from_value(p_value) {
                    Ok(p) => p,
                    Err(e) => {
                        return err("invalid_params", &format!("failed to decode params: {e}"));
                    }
                };
                let handle = tokio::task::spawn_blocking(move || f(params));
                match handle.await {
                    // The result travels as opaque JSON TEXT ("r_json"), not as
                    // a JSON value. Getter results carry u128s (prices,
                    // balances, supplies) that exceed f64's exact range, and any
                    // intermediary that re-serializes JSON — a proxy, or any
                    // client whose JSON numbers are doubles — silently rewrites
                    // them (640460000000000000000 becomes 6.4046e20), after
                    // which the typed decode fails. Text is immune.
                    Ok(Ok(result)) => match serde_json::to_string(&result) {
                        Ok(text) => json!({ "ok": true, "r_json": text }),
                        Err(e) => err("internal_error", &format!("serialize result: {e}")),
                    },
                    Ok(Err(e)) => err("getter_failed", &format!("{e:#}")),
                    Err(e) => err("internal_error", &format!("join: {e}")),
                }
            }
        })
        .await;
    });
}

/// Height/blockhash getters served from the versioned tree; the remote
/// explorer uses these for height-pinned views and indexed-bounds displays.
mod tree_getters {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    pub struct BlockhashForHeightParams {
        pub height: u32,
    }

    #[derive(Serialize, Deserialize)]
    pub struct BlockhashForHeightResult {
        /// 32 bytes, internal byte order, hex.
        pub blockhash: Option<String>,
    }

    #[derive(Serialize, Deserialize)]
    pub struct IndexedHeightBoundsParams {}

    #[derive(Serialize, Deserialize)]
    pub struct IndexedHeightBoundsResult {
        pub bounds: Option<(u32, u32)>,
    }
}

pub use tree_getters::*;

pub fn register_internal_rpc(reg: RpcNsRegistrar) {
    eprintln!("[RPC::INTERNAL] registering explorer getter RPC handlers…");

    register_getter(&reg, "tree_blockhash_for_height", |p: BlockhashForHeightParams| {
        let Some(tree) = crate::runtime::tree_db::get_global_tree_db() else {
            anyhow::bail!("versioned tree unavailable");
        };
        let blockhash = tree
            .blockhash_for_height(p.height)
            .map_err(|e| anyhow::anyhow!("tree lookup failed: {e}"))?;
        Ok(BlockhashForHeightResult {
            blockhash: blockhash.map(|bh| {
                use bitcoin::hashes::Hash as _;
                hex::encode(bh.to_byte_array())
            }),
        })
    });

    register_getter(&reg, "tree_indexed_height_bounds", |_p: IndexedHeightBoundsParams| {
        let bounds = match crate::runtime::tree_db::get_global_tree_db() {
            Some(tree) => {
                tree.indexed_height_bounds().map_err(|e| anyhow::anyhow!("tree bounds: {e}"))?
            }
            None => None,
        };
        Ok(IndexedHeightBoundsResult { bounds })
    });

    crate::modules::essentials::internal_rpc::register_internal_getters(&reg);
    crate::modules::runes::internal_rpc::register_internal_getters(&reg);
    crate::modules::ammdata::internal_rpc::register_internal_getters(&reg);
    crate::modules::tokendata::internal_rpc::register_internal_getters(&reg);
    crate::modules::pizzafun::internal_rpc::register_internal_getters(&reg);
    crate::modules::subfrost::internal_rpc::register_internal_getters(&reg);
}
