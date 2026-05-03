use super::storage::{
    ActionTxPointerBlob, GetRuneActivityPageParams, GetRuneAddressActivityPageParams, RuneActivity,
    RuneActivityKind, RuneActivityScope, RuneActivitySortField, RuneBalance, RuneMintActivity,
    RuneTxPointerBlob, RunesProvider, SortDir, TxRuneIo, rune_entry_to_json,
};
use crate::config::get_network;
use crate::modules::ammdata::consts::PRICE_SCALE;
use crate::modules::defs::RpcNsRegistrar;
use alloy_primitives::U256;
use bitcoin::hashes::Hash;
use bitcoin::{Address, Txid};
use serde_json::{Map, Value, json};
use std::str::FromStr;
use std::sync::Arc;

pub fn register_rpc(reg: &RpcNsRegistrar, provider: Arc<RunesProvider>) {
    let p = Arc::clone(&provider);
    let reg = reg.clone();
    tokio::spawn(async move {
        let p_rune = Arc::clone(&p);
        reg.register("get_rune", move |_cx, payload: Value| {
            let p = Arc::clone(&p_rune);
            async move {
                let query = payload
                    .get("rune")
                    .or_else(|| payload.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                match p.get_rune_by_query(query) {
                    Ok(Some(entry)) => {
                        let holders = p.get_holders_count(entry.id).unwrap_or(0);
                        json!({ "rune": rune_entry_to_json(&entry, holders) })
                    }
                    Ok(None) => json!({ "rune": null }),
                    Err(e) => json!({ "error": e.to_string() }),
                }
            }
        })
        .await;

        let p_top = Arc::clone(&p);
        reg.register("get_top_runes", move |_cx, payload: Value| {
            let p = Arc::clone(&p_top);
            async move {
                let page = payload.get("page").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
                let limit = payload.get("limit").and_then(|v| v.as_u64()).unwrap_or(50).clamp(1, 100) as usize;
                match p.get_top_runes(page, limit) {
                    Ok(rows) => json!({
                        "runes": rows.into_iter().map(|(entry, holders)| rune_entry_to_json(&entry, holders)).collect::<Vec<_>>()
                    }),
                    Err(e) => json!({ "error": e.to_string() }),
                }
            }
        }).await;

        let p_holders = Arc::clone(&p);
        reg.register("get_holders", move |_cx, payload: Value| {
            let p = Arc::clone(&p_holders);
            async move {
                let id = payload
                    .get("id")
                    .or_else(|| payload.get("rune"))
                    .or_else(|| payload.get("token"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let Some(entry) = p.get_rune_by_query(id).ok().flatten() else {
                    return json!({ "holders": [] });
                };
                let page =
                    payload.get("page").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
                let limit =
                    payload.get("limit").and_then(|v| v.as_u64()).unwrap_or(50).clamp(1, 100)
                        as usize;
                match p.get_holders(entry.id, page, limit) {
                    Ok(rows) => json!({
                        "holders": rows.into_iter().map(|(address, amount)| json!({
                            "address": address,
                            "amount": amount.to_string(),
                        })).collect::<Vec<_>>()
                    }),
                    Err(e) => json!({ "error": e.to_string() }),
                }
            }
        })
        .await;

        let p_address = Arc::clone(&p);
        reg.register("get_address_balances", move |_cx, payload: Value| {
            let p = Arc::clone(&p_address);
            async move {
                let Some(address_raw) =
                    payload.get("address").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty())
                else {
                    return json!({ "ok": false, "error": "missing_or_invalid_address" });
                };
                let Some(address) = normalize_address(address_raw) else {
                    return json!({ "ok": false, "error": "invalid_address_format" });
                };
                let include_outpoints = payload
                    .get("include_outpoints")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                match p.get_address_balances(&address) {
                    Ok(rows) => {
                        let mut balances = Map::new();
                        let items = rows
                            .iter()
                            .map(|(id, amount)| {
                                balances.insert(id.to_string(), Value::String(amount.to_string()));
                                json!({ "id": id.to_string(), "rune": id.to_string(), "amount": amount.to_string() })
                            })
                            .collect::<Vec<_>>();
                        let mut body = json!({
                            "ok": true,
                            "address": address,
                            "balances": Value::Object(balances),
                            "items": items,
                        });
                        if include_outpoints {
                            match p.get_address_outpoints(body["address"].as_str().unwrap_or_default()) {
                                Ok(rows) => {
                                    body.as_object_mut().unwrap().insert(
                                        "outpoints".to_string(),
                                        Value::Array(address_outpoints_to_json(rows)),
                                    );
                                }
                                Err(_) => return json!({ "ok": false, "error": "internal_error" }),
                            }
                        }
                        body
                    }
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                }
            }
        })
        .await;

        let p_address_outpoints = Arc::clone(&p);
        reg.register("get_address_outpoints", move |_cx, payload: Value| {
            let p = Arc::clone(&p_address_outpoints);
            async move {
                let Some(address_raw) = payload
                    .get("address")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    return json!({ "ok": false, "error": "missing_or_invalid_address" });
                };
                let Some(address) = normalize_address(address_raw) else {
                    return json!({ "ok": false, "error": "invalid_address_format" });
                };
                match p.get_address_outpoints(&address) {
                    Ok(rows) => json!({
                        "ok": true,
                        "address": address,
                        "outpoints": address_outpoints_to_json(rows),
                    }),
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                }
            }
        })
        .await;

        let p_outpoint = Arc::clone(&p);
        reg.register("get_outpoint_balances", move |_cx, payload: Value| {
            let p = Arc::clone(&p_outpoint);
            async move {
                let Some(outpoint) = payload
                    .get("outpoint")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    return json!({
                        "ok": false,
                        "error": "missing_or_invalid_outpoint",
                        "hint": "expected \"<txid>:<vout>\""
                    });
                };
                let (txid, vout) = match parse_outpoint_str(outpoint) {
                    Ok(parsed) => parsed,
                    Err(value) => return value,
                };
                match p.get_outpoint_balances(&txid, vout) {
                    Ok(row) => {
                        let entries = row
                            .as_ref()
                            .map(|row| rune_balances_to_json(&row.balances))
                            .unwrap_or_default();
                        let mut item = json!({
                            "outpoint": format!("{txid}:{vout}"),
                            "entries": entries,
                        });
                        if let Some(address) = row.and_then(|row| row.address) {
                            item.as_object_mut()
                                .unwrap()
                                .insert("address".to_string(), Value::String(address));
                        }
                        json!({
                            "ok": true,
                            "outpoint": format!("{txid}:{vout}"),
                            "items": [item],
                        })
                    }
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                }
            }
        })
        .await;

        let p_tx_io = Arc::clone(&p);
        reg.register("get_tx_io", move |_cx, payload: Value| {
            let p = Arc::clone(&p_tx_io);
            async move {
                let Some(txid_raw) = payload
                    .get("txid")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    return json!({ "ok": false, "error": "missing_or_invalid_txid" });
                };
                let txid = match Txid::from_str(txid_raw) {
                    Ok(txid) => txid,
                    Err(_) => return json!({ "ok": false, "error": "invalid_txid" }),
                };
                match p.get_tx_io(&txid) {
                    Ok(Some(io)) => {
                        json!({ "ok": true, "txid": txid.to_string(), "io": tx_io_to_json(&io) })
                    }
                    Ok(None) => json!({ "ok": true, "txid": txid.to_string(), "io": null }),
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                }
            }
        })
        .await;

        let p_activity = Arc::clone(&p);
        reg.register("get_mint_activity", move |_cx, payload: Value| {
            let p = Arc::clone(&p_activity);
            async move {
                let id = payload.get("id").and_then(|v| v.as_str()).unwrap_or_default();
                let Some(entry) = p.get_rune_by_query(id).ok().flatten() else {
                    return json!({ "activity": [] });
                };
                let page =
                    payload.get("page").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
                let limit =
                    payload.get("limit").and_then(|v| v.as_u64()).unwrap_or(50).clamp(1, 100)
                        as usize;
                match p.get_mint_activity(entry.id, page, limit) {
                    Ok(rows) => json!({
                        "activity": rows.iter().map(rune_mint_activity_to_json).collect::<Vec<_>>()
                    }),
                    Err(e) => json!({ "error": e.to_string() }),
                }
            }
        })
        .await;

        let p_rune_activity = Arc::clone(&p);
        reg.register("get_activity", move |_cx, payload: Value| {
            let p = Arc::clone(&p_rune_activity);
            async move {
                let id = payload
                    .get("id")
                    .or_else(|| payload.get("rune"))
                    .or_else(|| payload.get("token"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let Some(entry) = p.get_rune_by_query(id).ok().flatten() else {
                    return json!({ "ok": true, "activity": [], "entries": [], "total": 0 });
                };
                let page =
                    payload.get("page").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
                let limit =
                    payload.get("limit").and_then(|v| v.as_u64()).unwrap_or(50).clamp(1, 100)
                        as usize;
                let offset = limit.saturating_mul(page.saturating_sub(1));
                let kind = parse_activity_kind(payload.get("kind").and_then(|v| v.as_str()));
                let scope = parse_activity_scope(
                    payload
                        .get("activity_type")
                        .or_else(|| payload.get("type"))
                        .or_else(|| payload.get("filter"))
                        .or_else(|| payload.get("scope"))
                        .and_then(|v| v.as_str()),
                );
                let params = GetRuneActivityPageParams {
                    id: entry.id,
                    address: payload
                        .get("address")
                        .and_then(|v| v.as_str())
                        .and_then(normalize_address),
                    offset,
                    limit,
                    kind,
                    scope: scope_for_kind(kind, scope),
                    sort_by: parse_activity_sort(
                        payload
                            .get("sort")
                            .or_else(|| payload.get("sort_by"))
                            .and_then(|v| v.as_str()),
                    ),
                    dir: parse_sort_dir(payload.get("dir").and_then(|v| v.as_str())),
                    start_time: parse_time(
                        payload
                            .get("start_time")
                            .or_else(|| payload.get("start_ts"))
                            .or_else(|| payload.get("target_start"))
                            .or_else(|| payload.get("from")),
                    ),
                    end_time: parse_time(
                        payload
                            .get("end_time")
                            .or_else(|| payload.get("end_ts"))
                            .or_else(|| payload.get("target_end"))
                            .or_else(|| payload.get("to")),
                    ),
                };
                match p.get_rune_activity_page(params) {
                    Ok(page_data) => {
                        let entries =
                            page_data.entries.iter().map(rune_activity_to_json).collect::<Vec<_>>();
                        json!({
                            "ok": true,
                            "total": page_data.total,
                            "activity": entries.clone(),
                            "entries": entries,
                        })
                    }
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                }
            }
        })
        .await;

        let p_rune_activity_alias = Arc::clone(&p);
        reg.register("get_rune_activity", move |_cx, payload: Value| {
            let p = Arc::clone(&p_rune_activity_alias);
            async move { handle_rune_activity_rpc(p, payload).await }
        })
        .await;

        let p_address_activity = Arc::clone(&p);
        reg.register("get_address_activity", move |_cx, payload: Value| {
            let p = Arc::clone(&p_address_activity);
            async move {
                let Some(address_raw) = payload
                    .get("address")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    return json!({ "ok": false, "error": "missing_or_invalid_address" });
                };
                let Some(address) = normalize_address(address_raw) else {
                    return json!({ "ok": false, "error": "missing_or_invalid_address" });
                };
                let id = payload
                    .get("id")
                    .or_else(|| payload.get("rune"))
                    .or_else(|| payload.get("token"))
                    .and_then(|v| v.as_str());
                let rune_id = match id {
                    Some("all") | Some("") | None => None,
                    Some(value) => match p.get_rune_by_query(value).ok().flatten() {
                        Some(entry) => Some(entry.id),
                        None => return json!({ "ok": false, "error": "missing_or_invalid_token" }),
                    },
                };
                let page =
                    payload.get("page").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
                let limit =
                    payload.get("limit").and_then(|v| v.as_u64()).unwrap_or(50).clamp(1, 100)
                        as usize;
                let kind = parse_activity_kind(payload.get("kind").and_then(|v| v.as_str()));
                let scope = parse_activity_scope(
                    payload
                        .get("activity_type")
                        .or_else(|| payload.get("type"))
                        .or_else(|| payload.get("filter"))
                        .or_else(|| payload.get("scope"))
                        .and_then(|v| v.as_str()),
                );
                match p.get_rune_address_activity_page(GetRuneAddressActivityPageParams {
                    address,
                    id: rune_id,
                    offset: limit.saturating_mul(page.saturating_sub(1)),
                    limit,
                    kind,
                    scope: scope_for_kind(kind, scope),
                    sort_by: parse_activity_sort(
                        payload
                            .get("sort")
                            .or_else(|| payload.get("sort_by"))
                            .and_then(|v| v.as_str()),
                    ),
                    dir: parse_sort_dir(payload.get("dir").and_then(|v| v.as_str())),
                    start_time: parse_time(
                        payload
                            .get("start_time")
                            .or_else(|| payload.get("start_ts"))
                            .or_else(|| payload.get("target_start"))
                            .or_else(|| payload.get("from")),
                    ),
                    end_time: parse_time(
                        payload
                            .get("end_time")
                            .or_else(|| payload.get("end_ts"))
                            .or_else(|| payload.get("target_end"))
                            .or_else(|| payload.get("to")),
                    ),
                }) {
                    Ok(page_data) => {
                        let entries =
                            page_data.entries.iter().map(rune_activity_to_json).collect::<Vec<_>>();
                        json!({
                            "ok": true,
                            "total": page_data.total,
                            "activity": entries.clone(),
                            "entries": entries,
                        })
                    }
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                }
            }
        })
        .await;

        register_tx_index_rpc(
            &reg,
            "get_block_tx_count",
            Arc::clone(&p),
            TxIndexRpcKind::BlockCount,
        )
        .await;
        register_tx_index_rpc(&reg, "get_block_txs", Arc::clone(&p), TxIndexRpcKind::BlockRange)
            .await;
        register_tx_index_rpc(
            &reg,
            "get_address_tx_count",
            Arc::clone(&p),
            TxIndexRpcKind::AddressCount,
        )
        .await;
        register_tx_index_rpc(
            &reg,
            "get_address_txs",
            Arc::clone(&p),
            TxIndexRpcKind::AddressRange,
        )
        .await;
        register_tx_index_rpc(
            &reg,
            "get_action_block_tx_count",
            Arc::clone(&p),
            TxIndexRpcKind::ActionBlockCount,
        )
        .await;
        register_tx_index_rpc(
            &reg,
            "get_action_block_txs",
            Arc::clone(&p),
            TxIndexRpcKind::ActionBlockRange,
        )
        .await;
        register_tx_index_rpc(
            &reg,
            "get_action_address_tx_count",
            Arc::clone(&p),
            TxIndexRpcKind::ActionAddressCount,
        )
        .await;
        register_tx_index_rpc(
            &reg,
            "get_action_address_txs",
            Arc::clone(&p),
            TxIndexRpcKind::ActionAddressRange,
        )
        .await;
    });
}

#[derive(Clone, Copy)]
enum TxIndexRpcKind {
    BlockCount,
    BlockRange,
    AddressCount,
    AddressRange,
    ActionBlockCount,
    ActionBlockRange,
    ActionAddressCount,
    ActionAddressRange,
}

async fn handle_rune_activity_rpc(provider: Arc<RunesProvider>, payload: Value) -> Value {
    let id = payload
        .get("id")
        .or_else(|| payload.get("rune"))
        .or_else(|| payload.get("token"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let Some(entry) = provider.get_rune_by_query(id).ok().flatten() else {
        return json!({ "ok": true, "activity": [], "entries": [], "total": 0 });
    };
    let page = payload.get("page").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
    let limit = payload.get("limit").and_then(|v| v.as_u64()).unwrap_or(50).clamp(1, 100) as usize;
    let kind = parse_activity_kind(payload.get("kind").and_then(|v| v.as_str()));
    let scope = parse_activity_scope(
        payload
            .get("activity_type")
            .or_else(|| payload.get("type"))
            .or_else(|| payload.get("filter"))
            .or_else(|| payload.get("scope"))
            .and_then(|v| v.as_str()),
    );
    match provider.get_rune_activity_page(GetRuneActivityPageParams {
        id: entry.id,
        address: payload.get("address").and_then(|v| v.as_str()).and_then(normalize_address),
        offset: limit.saturating_mul(page.saturating_sub(1)),
        limit,
        kind,
        scope: scope_for_kind(kind, scope),
        sort_by: parse_activity_sort(
            payload.get("sort").or_else(|| payload.get("sort_by")).and_then(|v| v.as_str()),
        ),
        dir: parse_sort_dir(payload.get("dir").and_then(|v| v.as_str())),
        start_time: parse_time(
            payload
                .get("start_time")
                .or_else(|| payload.get("start_ts"))
                .or_else(|| payload.get("target_start"))
                .or_else(|| payload.get("from")),
        ),
        end_time: parse_time(
            payload
                .get("end_time")
                .or_else(|| payload.get("end_ts"))
                .or_else(|| payload.get("target_end"))
                .or_else(|| payload.get("to")),
        ),
    }) {
        Ok(page_data) => {
            let entries = page_data.entries.iter().map(rune_activity_to_json).collect::<Vec<_>>();
            json!({
                "ok": true,
                "total": page_data.total,
                "activity": entries.clone(),
                "entries": entries,
            })
        }
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

async fn register_tx_index_rpc(
    reg: &RpcNsRegistrar,
    name: &'static str,
    provider: Arc<RunesProvider>,
    kind: TxIndexRpcKind,
) {
    let p = Arc::clone(&provider);
    reg.register(name, move |_cx, payload: Value| {
        let p = Arc::clone(&p);
        async move { handle_tx_index_rpc(p, payload, kind) }
    })
    .await;
}

fn handle_tx_index_rpc(
    provider: Arc<RunesProvider>,
    payload: Value,
    kind: TxIndexRpcKind,
) -> Value {
    match kind {
        TxIndexRpcKind::BlockCount => {
            let Some(height) = payload.get("height").and_then(|v| v.as_u64()) else {
                return json!({ "ok": false, "error": "missing_or_invalid_height" });
            };
            match provider.get_block_tx_count(height) {
                Ok(count) => json!({ "ok": true, "height": height, "count": count }),
                Err(e) => json!({ "ok": false, "error": e.to_string() }),
            }
        }
        TxIndexRpcKind::ActionBlockCount => {
            let Some(height) = payload.get("height").and_then(|v| v.as_u64()) else {
                return json!({ "ok": false, "error": "missing_or_invalid_height" });
            };
            match provider.get_action_block_tx_count(height) {
                Ok(count) => json!({ "ok": true, "height": height, "count": count }),
                Err(e) => json!({ "ok": false, "error": e.to_string() }),
            }
        }
        TxIndexRpcKind::BlockRange => {
            let Some(height) = payload.get("height").and_then(|v| v.as_u64()) else {
                return json!({ "ok": false, "error": "missing_or_invalid_height" });
            };
            let (start, end) = parse_range(&payload);
            match provider.get_block_tx_range(height, start, end) {
                Ok(rows) => {
                    json!({ "ok": true, "height": height, "txs": rows.iter().map(rune_tx_pointer_to_json).collect::<Vec<_>>() })
                }
                Err(e) => json!({ "ok": false, "error": e.to_string() }),
            }
        }
        TxIndexRpcKind::ActionBlockRange => {
            let Some(height) = payload.get("height").and_then(|v| v.as_u64()) else {
                return json!({ "ok": false, "error": "missing_or_invalid_height" });
            };
            let (start, end) = parse_range(&payload);
            match provider.get_action_block_tx_range(height, start, end) {
                Ok(rows) => {
                    json!({ "ok": true, "height": height, "txs": rows.iter().map(action_tx_pointer_to_json).collect::<Vec<_>>() })
                }
                Err(e) => json!({ "ok": false, "error": e.to_string() }),
            }
        }
        TxIndexRpcKind::AddressCount => {
            with_address(&payload, |address| match provider.get_address_tx_count(&address) {
                Ok(count) => json!({ "ok": true, "address": address, "count": count }),
                Err(e) => json!({ "ok": false, "error": e.to_string() }),
            })
        }
        TxIndexRpcKind::ActionAddressCount => {
            with_address(&payload, |address| match provider.get_action_address_tx_count(&address) {
                Ok(count) => json!({ "ok": true, "address": address, "count": count }),
                Err(e) => json!({ "ok": false, "error": e.to_string() }),
            })
        }
        TxIndexRpcKind::AddressRange => with_address(&payload, |address| {
            let (start, end) = parse_range(&payload);
            match provider.get_address_tx_range(&address, start, end) {
                Ok(rows) => {
                    json!({ "ok": true, "address": address, "txs": rows.iter().map(rune_tx_pointer_to_json).collect::<Vec<_>>() })
                }
                Err(e) => json!({ "ok": false, "error": e.to_string() }),
            }
        }),
        TxIndexRpcKind::ActionAddressRange => with_address(&payload, |address| {
            let (start, end) = parse_range(&payload);
            match provider.get_action_address_tx_range(&address, start, end) {
                Ok(rows) => {
                    json!({ "ok": true, "address": address, "txs": rows.iter().map(action_tx_pointer_to_json).collect::<Vec<_>>() })
                }
                Err(e) => json!({ "ok": false, "error": e.to_string() }),
            }
        }),
    }
}

fn normalize_address(address: &str) -> Option<String> {
    Address::from_str(address)
        .ok()
        .and_then(|a| a.require_network(get_network()).ok())
        .map(|a| a.to_string())
}

fn with_address(payload: &Value, f: impl FnOnce(String) -> Value) -> Value {
    let Some(address_raw) = payload
        .get("address")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return json!({ "ok": false, "error": "missing_or_invalid_address" });
    };
    let Some(address) = normalize_address(address_raw) else {
        return json!({ "ok": false, "error": "invalid_address_format" });
    };
    f(address)
}

fn parse_outpoint_str(outpoint: &str) -> Result<(Txid, u32), Value> {
    let Some((txid_raw, vout_raw)) = outpoint.rsplit_once(':') else {
        return Err(json!({
            "ok": false,
            "error": "missing_or_invalid_outpoint",
            "hint": "expected \"<txid>:<vout>\""
        }));
    };
    let txid =
        Txid::from_str(txid_raw).map_err(|_| json!({ "ok": false, "error": "invalid_txid" }))?;
    let vout = vout_raw
        .parse::<u32>()
        .map_err(|_| json!({ "ok": false, "error": "invalid_vout" }))?;
    Ok((txid, vout))
}

fn parse_range(payload: &Value) -> (u64, u64) {
    let limit = payload.get("limit").and_then(|v| v.as_u64()).unwrap_or(50).clamp(1, 100);
    let start = payload.get("start").and_then(|v| v.as_u64()).unwrap_or_else(|| {
        let page = payload.get("page").and_then(|v| v.as_u64()).unwrap_or(1).max(1);
        limit.saturating_mul(page.saturating_sub(1))
    });
    let end = payload
        .get("end")
        .and_then(|v| v.as_u64())
        .unwrap_or(start.saturating_add(limit));
    (start, end.max(start))
}

fn rune_balances_to_json(balances: &[RuneBalance]) -> Vec<Value> {
    balances
        .iter()
        .map(|balance| {
            json!({
                "id": balance.id.to_string(),
                "rune": balance.id.to_string(),
                "amount": balance.amount.to_string(),
            })
        })
        .collect()
}

fn address_outpoints_to_json(
    rows: Vec<(Txid, u32, super::storage::OutpointRuneBalances)>,
) -> Vec<Value> {
    rows.into_iter()
        .map(|(txid, vout, row)| {
            let mut item = json!({
                "outpoint": format!("{txid}:{vout}"),
                "entries": rune_balances_to_json(&row.balances),
            });
            if let Some(address) = row.address {
                item.as_object_mut()
                    .unwrap()
                    .insert("address".to_string(), Value::String(address));
            }
            item
        })
        .collect()
}

fn tx_io_to_json(io: &TxRuneIo) -> Value {
    let inputs = io
        .inputs
        .iter()
        .map(|(vout, balances)| (vout.to_string(), Value::Array(rune_balances_to_json(balances))))
        .collect::<Map<_, _>>();
    let outputs = io
        .outputs
        .iter()
        .map(|(vout, balances)| (vout.to_string(), Value::Array(rune_balances_to_json(balances))))
        .collect::<Map<_, _>>();
    json!({
        "inputs": Value::Object(inputs),
        "outputs": Value::Object(outputs),
        "burned": rune_balances_to_json(&io.burned),
        "minted": rune_balances_to_json(&io.minted),
        "etched": io.etched.map(|id| id.to_string()),
    })
}

fn rune_mint_activity_to_json(row: &RuneMintActivity) -> Value {
    json!({
        "id": row.id.to_string(),
        "token": row.id.to_string(),
        "txid": Txid::from_byte_array(row.txid).to_string(),
        "chain_txids": row.chain_txids.iter().map(|txid| Txid::from_byte_array(*txid).to_string()).collect::<Vec<_>>(),
        "cpfp": row.cpfp,
        "height": row.height,
        "tx_index": row.tx_index,
        "timestamp": row.timestamp,
        "amount": row.amount.to_string(),
        "token_delta": row.amount.to_string(),
        "kind": "mint",
        "source": "mint",
        "fee_paid_sats": row.fee_paid_sats.to_string(),
        "mint_price_paid_sats": scaled_u256_bytes_to_decimal(row.mint_price_paid_sats),
        "mint_price_paid_usd": scaled_u256_bytes_to_decimal(row.mint_price_paid_usd),
        "mint_price_pool_usd": Value::Null,
        "mint_price_pool_frbtc_sats": Value::Null,
        "pool": Value::Null,
        "counter_token": Value::Null,
        "counter_delta": "0",
        "destination": row.destination,
        "success": row.success,
    })
}

fn rune_activity_to_json(row: &RuneActivity) -> Value {
    json!({
        "id": row.id.to_string(),
        "token": row.id.to_string(),
        "txid": Txid::from_byte_array(row.txid).to_string(),
        "chain_txids": row.chain_txids.iter().map(|txid| Txid::from_byte_array(*txid).to_string()).collect::<Vec<_>>(),
        "cpfp": row.cpfp,
        "height": row.height,
        "tx_index": row.tx_index,
        "timestamp": row.timestamp,
        "kind": row.kind.key(),
        "source": row.kind.key(),
        "amount": row.amount.to_string(),
        "token_delta": row.amount.to_string(),
        "fee_paid_sats": row.fee_paid_sats.to_string(),
        "mint_price_paid_sats": scaled_u256_bytes_to_decimal(row.mint_price_paid_sats),
        "mint_price_paid_usd": scaled_u256_bytes_to_decimal(row.mint_price_paid_usd),
        "mint_price_pool_usd": Value::Null,
        "mint_price_pool_frbtc_sats": Value::Null,
        "pool": Value::Null,
        "counter_token": Value::Null,
        "counter_delta": "0",
        "destination": row.destination,
        "success": row.success,
    })
}

fn scaled_u256_bytes_to_decimal(bytes: [u8; 32]) -> Option<String> {
    let value = U256::from_be_bytes(bytes);
    if value.is_zero() {
        return None;
    }
    let scale = U256::from(PRICE_SCALE);
    let whole = value / scale;
    let frac = (value % scale).to::<u128>();
    let mut frac_str = frac.to_string();
    if frac_str.len() < 16 {
        frac_str = format!("{}{}", "0".repeat(16 - frac_str.len()), frac_str);
    }
    while frac_str.ends_with('0') {
        frac_str.pop();
    }
    if frac_str.is_empty() { Some(whole.to_string()) } else { Some(format!("{whole}.{frac_str}")) }
}

fn rune_tx_pointer_to_json(row: &RuneTxPointerBlob) -> Value {
    json!({
        "txid": Txid::from_byte_array(row.txid).to_string(),
        "height": row.height,
        "tx_index": row.tx_index,
        "io": tx_io_to_json(&row.io),
    })
}

fn action_tx_pointer_to_json(row: &ActionTxPointerBlob) -> Value {
    json!({
        "txid": Txid::from_byte_array(row.txid).to_string(),
        "height": row.height,
        "tx_index": row.tx_index,
        "has_alkane": row.has_alkane,
        "has_rune": row.has_rune,
    })
}

fn parse_activity_scope(raw: Option<&str>) -> RuneActivityScope {
    match raw.unwrap_or("all").to_ascii_lowercase().as_str() {
        "market" => RuneActivityScope::Market,
        "mint" | "mints" => RuneActivityScope::Mint,
        "etch" | "etches" => RuneActivityScope::Etch,
        _ => RuneActivityScope::All,
    }
}

fn parse_activity_kind(raw: Option<&str>) -> Option<RuneActivityKind> {
    match raw.unwrap_or_default().to_ascii_lowercase().as_str() {
        "mint" | "mints" => Some(RuneActivityKind::Mint),
        "etch" | "etching" | "etches" => Some(RuneActivityKind::Etch),
        _ => None,
    }
}

fn scope_for_kind(kind: Option<RuneActivityKind>, scope: RuneActivityScope) -> RuneActivityScope {
    match (kind, scope) {
        (Some(RuneActivityKind::Mint), RuneActivityScope::All) => RuneActivityScope::Mint,
        (Some(RuneActivityKind::Etch), RuneActivityScope::All) => RuneActivityScope::Etch,
        _ => scope,
    }
}

fn parse_time(raw: Option<&Value>) -> Option<u64> {
    match raw {
        None => None,
        Some(v) => v.as_u64().or_else(|| v.as_str()?.trim().parse::<u64>().ok()),
    }
}

fn parse_activity_sort(raw: Option<&str>) -> RuneActivitySortField {
    match raw.unwrap_or("timestamp").to_ascii_lowercase().as_str() {
        "amount" => RuneActivitySortField::Amount,
        _ => RuneActivitySortField::Timestamp,
    }
}

fn parse_sort_dir(raw: Option<&str>) -> SortDir {
    match raw.unwrap_or("desc").to_ascii_lowercase().as_str() {
        "asc" => SortDir::Asc,
        _ => SortDir::Desc,
    }
}
