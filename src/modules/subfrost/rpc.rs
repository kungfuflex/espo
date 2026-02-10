use crate::config::get_network;
use crate::modules::defs::RpcNsRegistrar;
use crate::modules::subfrost::storage::{
    GetUnwrapEventsAllParams, GetUnwrapEventsByAddressParams, GetWrapEventsAllParams,
    GetWrapEventsByAddressParams, SubfrostProvider,
};
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
                    provider
                        .get_wrap_events_by_address(GetWrapEventsByAddressParams {
                            address_spk: spk,
                            offset,
                            limit: count,
                            successful,
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
                    provider
                        .get_unwrap_events_by_address(GetUnwrapEventsByAddressParams {
                            address_spk: spk,
                            offset,
                            limit: count,
                            successful,
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
                    provider
                        .get_wrap_events_all(GetWrapEventsAllParams {
                            offset,
                            limit: count,
                            successful,
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
                    provider
                        .get_unwrap_events_all(GetUnwrapEventsAllParams {
                            offset,
                            limit: count,
                            successful,
                        })
                        .map(|resp| wrap_events_json(resp.entries, resp.total))
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
            let mut txid = e.txid;
            txid.reverse();
            json!({
                "txid": hex::encode(txid),
                "timestamp": e.timestamp,
                "amount": e.amount.to_string(),
                "address_spk": hex::encode(e.address_spk),
                "success": e.success,
            })
        })
        .collect::<Vec<_>>();
    json!({ "items": items, "total": total })
}
