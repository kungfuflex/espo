use crate::config::get_network;
use crate::modules::defs::RpcNsRegistrar;
use crate::modules::subfrost::storage::{
    GetUnwrapEventsAllParams, GetUnwrapEventsByAddressParams, GetUnwrapRequestsAllParams,
    GetUnwrapRequestsByAddressParams, GetWrapEventsAllParams, GetWrapEventsByAddressParams,
    SubfrostProvider,
};
use crate::runtime::state_at::StateAt;
use bitcoin::Address;
use serde_json::{Value, json};
use std::str::FromStr;
use std::sync::Arc;

#[allow(dead_code)]
pub fn register_rpc(reg: &RpcNsRegistrar, provider: Arc<SubfrostProvider>) {
    let reg_wrap_addr = reg.clone();
    let provider_wrap_addr = Arc::clone(&provider);
    tokio::spawn(async move {
        reg_wrap_addr
            .register("get_wrap_events_by_address", move |_cx, payload| {
                let provider = Arc::clone(&provider_wrap_addr);
                async move {
                    let Some(address) = payload.get("address").and_then(|v| v.as_str()) else {
                        return json!({ "ok": false, "error": "invalid_address" });
                    };
                    let Some(spk) = address_spk(address) else {
                        return json!({ "ok": false, "error": "invalid_address" });
                    };
                    let count = clamp_count(payload.get("count").and_then(|v| v.as_u64()));
                    let offset =
                        payload.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    let successful = payload.get("successful").and_then(|v| v.as_bool());
                    let height = payload.get("height").and_then(|v| v.as_u64());
                    let height_present = payload.get("height").is_some();
                    let view = match provider.with_height(height, height_present) {
                        Ok(v) => v,
                        Err(_) => return json!({ "ok": false, "error": "invalid_height" }),
                    };
                    view.get_wrap_events_by_address(GetWrapEventsByAddressParams {
                        blockhash: StateAt::Latest,
                        address_spk: spk,
                        offset,
                        limit: count,
                        successful,
                        height,
                        height_present,
                    })
                    .map(|resp| wrap_events_json(resp.entries, resp.total))
                    .unwrap_or_else(|_| json!({ "ok": false, "error": "internal_error" }))
                }
            })
            .await;
    });

    let reg_unwrap_addr = reg.clone();
    let provider_unwrap_addr = Arc::clone(&provider);
    tokio::spawn(async move {
        reg_unwrap_addr
            .register("get_unwrap_events_by_address", move |_cx, payload| {
                let provider = Arc::clone(&provider_unwrap_addr);
                async move {
                    let Some(address) = payload.get("address").and_then(|v| v.as_str()) else {
                        return json!({ "ok": false, "error": "invalid_address" });
                    };
                    let Some(spk) = address_spk(address) else {
                        return json!({ "ok": false, "error": "invalid_address" });
                    };
                    let count = clamp_count(payload.get("count").and_then(|v| v.as_u64()));
                    let offset =
                        payload.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    let successful = payload.get("successful").and_then(|v| v.as_bool());
                    let height = payload.get("height").and_then(|v| v.as_u64());
                    let height_present = payload.get("height").is_some();
                    let view = match provider.with_height(height, height_present) {
                        Ok(v) => v,
                        Err(_) => return json!({ "ok": false, "error": "invalid_height" }),
                    };
                    view.get_unwrap_events_by_address(GetUnwrapEventsByAddressParams {
                        blockhash: StateAt::Latest,
                        address_spk: spk,
                        offset,
                        limit: count,
                        successful,
                        height,
                        height_present,
                    })
                    .map(|resp| wrap_events_json(resp.entries, resp.total))
                    .unwrap_or_else(|_| json!({ "ok": false, "error": "internal_error" }))
                }
            })
            .await;
    });

    let reg_wrap_all = reg.clone();
    let provider_wrap_all = Arc::clone(&provider);
    tokio::spawn(async move {
        reg_wrap_all
            .register("get_wrap_events_all", move |_cx, payload| {
                let provider = Arc::clone(&provider_wrap_all);
                async move {
                    let count = clamp_count(payload.get("count").and_then(|v| v.as_u64()));
                    let offset =
                        payload.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    let successful = payload.get("successful").and_then(|v| v.as_bool());
                    let height = payload.get("height").and_then(|v| v.as_u64());
                    let height_present = payload.get("height").is_some();
                    let view = match provider.with_height(height, height_present) {
                        Ok(v) => v,
                        Err(_) => return json!({ "ok": false, "error": "invalid_height" }),
                    };
                    view.get_wrap_events_all(GetWrapEventsAllParams {
                        blockhash: StateAt::Latest,
                        offset,
                        limit: count,
                        successful,
                        height,
                        height_present,
                    })
                    .map(|resp| wrap_events_json(resp.entries, resp.total))
                    .unwrap_or_else(|_| json!({ "ok": false, "error": "internal_error" }))
                }
            })
            .await;
    });

    let reg_unwrap_all = reg.clone();
    let provider_unwrap_all = Arc::clone(&provider);
    tokio::spawn(async move {
        reg_unwrap_all
            .register("get_unwrap_events_all", move |_cx, payload| {
                let provider = Arc::clone(&provider_unwrap_all);
                async move {
                    let count = clamp_count(payload.get("count").and_then(|v| v.as_u64()));
                    let offset =
                        payload.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    let successful = payload.get("successful").and_then(|v| v.as_bool());
                    let height = payload.get("height").and_then(|v| v.as_u64());
                    let height_present = payload.get("height").is_some();
                    let view = match provider.with_height(height, height_present) {
                        Ok(v) => v,
                        Err(_) => return json!({ "ok": false, "error": "invalid_height" }),
                    };
                    view.get_unwrap_events_all(GetUnwrapEventsAllParams {
                        blockhash: StateAt::Latest,
                        offset,
                        limit: count,
                        successful,
                        height,
                        height_present,
                    })
                    .map(|resp| wrap_events_json(resp.entries, resp.total))
                    .unwrap_or_else(|_| json!({ "ok": false, "error": "internal_error" }))
                }
            })
            .await;
    });

    let reg_unwrap_requests_addr = reg.clone();
    let provider_unwrap_requests_addr = Arc::clone(&provider);
    tokio::spawn(async move {
        reg_unwrap_requests_addr
            .register("get_unwrap_requests_by_address", move |_cx, payload| {
                let provider = Arc::clone(&provider_unwrap_requests_addr);
                async move {
                    let Some(address) = payload.get("address").and_then(|v| v.as_str()) else {
                        return json!({ "ok": false, "error": "invalid_address" });
                    };
                    let Some(spk) = address_spk(address) else {
                        return json!({ "ok": false, "error": "invalid_address" });
                    };
                    let count = clamp_count(payload.get("count").and_then(|v| v.as_u64()));
                    let offset =
                        payload.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    let fulfilled = payload.get("fulfilled").and_then(|v| v.as_bool());
                    let height = payload.get("height").and_then(|v| v.as_u64());
                    let height_present = payload.get("height").is_some();
                    let view = match provider.with_height(height, height_present) {
                        Ok(v) => v,
                        Err(_) => return json!({ "ok": false, "error": "invalid_height" }),
                    };
                    view.get_unwrap_requests_by_address(GetUnwrapRequestsByAddressParams {
                        blockhash: StateAt::Latest,
                        address_spk: spk,
                        offset,
                        limit: count,
                        fulfilled,
                        height,
                        height_present,
                    })
                    .map(|resp| unwrap_requests_json(resp.entries, resp.total))
                    .unwrap_or_else(|_| json!({ "ok": false, "error": "internal_error" }))
                }
            })
            .await;
    });

    let reg_unwrap_requests_all = reg.clone();
    let provider_unwrap_requests_all = Arc::clone(&provider);
    tokio::spawn(async move {
        reg_unwrap_requests_all
            .register("get_unwrap_requests_all", move |_cx, payload| {
                let provider = Arc::clone(&provider_unwrap_requests_all);
                async move {
                    let count = clamp_count(payload.get("count").and_then(|v| v.as_u64()));
                    let offset =
                        payload.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    let fulfilled = payload.get("fulfilled").and_then(|v| v.as_bool());
                    let height = payload.get("height").and_then(|v| v.as_u64());
                    let height_present = payload.get("height").is_some();
                    let view = match provider.with_height(height, height_present) {
                        Ok(v) => v,
                        Err(_) => return json!({ "ok": false, "error": "invalid_height" }),
                    };
                    view.get_unwrap_requests_all(GetUnwrapRequestsAllParams {
                        blockhash: StateAt::Latest,
                        offset,
                        limit: count,
                        fulfilled,
                        height,
                        height_present,
                    })
                    .map(|resp| unwrap_requests_json(resp.entries, resp.total))
                    .unwrap_or_else(|_| json!({ "ok": false, "error": "internal_error" }))
                }
            })
            .await;
    });
}

fn address_spk(address: &str) -> Option<Vec<u8>> {
    let network = get_network();
    Address::from_str(address)
        .ok()
        .and_then(|a| a.require_network(network).ok())
        .map(|a| a.script_pubkey().into_bytes())
}

fn clamp_count(count: Option<u64>) -> usize {
    let count = count.unwrap_or(50);
    let count = count.clamp(1, 200);
    count as usize
}

fn wrap_events_json(events: Vec<super::schemas::SchemaWrapEventV1>, total: usize) -> Value {
    let items = events
        .into_iter()
        .map(|e| {
            json!({
                "txid": txid_hex(e.txid),
                "timestamp": e.timestamp,
                "amount": e.amount.to_string(),
                "address_spk": hex::encode(e.address_spk),
                "success": e.success,
            })
        })
        .collect::<Vec<_>>();
    json!({ "items": items, "total": total })
}

fn unwrap_requests_json(
    requests: Vec<super::schemas::SchemaUnwrapRequestV1>,
    total: usize,
) -> Value {
    let items = requests
        .into_iter()
        .map(|request| {
            let fulfilled = request.fulfilled();
            let fulfillment_tx = request.fulfillment_tx.map(txid_hex).map(Value::String);
            json!({
                "txid": txid_hex(request.txid),
                "vout": request.vout,
                "timestamp": request.timestamp,
                "amount": request.amount.to_string(),
                "address_spk": hex::encode(request.address_spk),
                "fulfilled": fulfilled,
                "fulfillment_tx": fulfillment_tx.unwrap_or(Value::Null),
            })
        })
        .collect::<Vec<_>>();
    json!({ "items": items, "total": total })
}

fn txid_hex(mut txid: [u8; 32]) -> String {
    txid.reverse();
    hex::encode(txid)
}
