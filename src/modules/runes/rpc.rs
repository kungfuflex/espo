use super::storage::{RunesProvider, rune_entry_to_json};
use crate::modules::defs::RpcNsRegistrar;
use serde_json::{Value, json};
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
                let id = payload.get("id").and_then(|v| v.as_str()).unwrap_or_default();
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
                let address = payload.get("address").and_then(|v| v.as_str()).unwrap_or_default();
                match p.get_address_balances(address) {
                    Ok(rows) => json!({
                        "balances": rows.into_iter().map(|(id, amount)| json!({
                            "id": id.to_string(),
                            "amount": amount.to_string(),
                        })).collect::<Vec<_>>()
                    }),
                    Err(e) => json!({ "error": e.to_string() }),
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
                        "activity": rows.into_iter().map(|row| json!({
                            "id": row.id.to_string(),
                            "txid": hex::encode(row.txid),
                            "height": row.height,
                            "tx_index": row.tx_index,
                            "timestamp": row.timestamp,
                            "amount": row.amount.to_string(),
                            "destination": row.destination,
                        })).collect::<Vec<_>>()
                    }),
                    Err(e) => json!({ "error": e.to_string() }),
                }
            }
        })
        .await;
    });
}
