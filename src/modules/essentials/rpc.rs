use crate::modules::defs::RpcNsRegistrar;
use crate::modules::essentials::storage::{
    EssentialsProvider, RpcGetAddressActivityParams, RpcGetAddressBalancesParams,
    RpcGetAddressCumulativeAlkanesParams, RpcGetAddressOutpointsParams,
    RpcGetAddressSpendableOutpointsParams, RpcGetAddressTransactionsParams, RpcGetAlkabiParams,
    RpcGetAlkaneAddressTxsParams, RpcGetAlkaneBalanceMetashrewParams,
    RpcGetAlkaneBalanceTxsByTokenParams, RpcGetAlkaneBalanceTxsParams, RpcGetAlkaneBalancesParams,
    RpcGetAlkaneBlockTxsParams, RpcGetAlkaneInfoParams, RpcGetAlkaneLatestTracesParams,
    RpcGetAlkaneTxSummaryParams, RpcGetAlkaneVolumesParams, RpcGetAlkaneWasmParams,
    RpcGetAllAlkanesParams, RpcGetBlockSummaryParams, RpcGetBlockTimeParams,
    RpcGetBlockTimesParams, RpcGetBlockTracesParams, RpcGetCirculatingSupplyParams,
    RpcGetFactoryChildrenParams, RpcGetHoldersCountParams, RpcGetHoldersParams, RpcGetKeysParams,
    RpcGetMempoolTracesParams, RpcGetOrbitalBalancesParams, RpcGetOrbitalHoldersParams,
    RpcGetOrbitalVolumesParams, RpcGetOutpointBalancesParams, RpcGetRuntimeBalancesMetashrewParams,
    RpcGetTotalReceivedParams, RpcGetTransferVolumeParams, RpcPingParams, RpcSearchAlkaneParams,
    RpcSearchFactoryKeysParams,
};
use crate::runtime::mempool::{current_mempool_memory_stats, mempool_availability};
use serde_json::{Value, json};
use std::sync::Arc;

fn resolve_view(
    provider: &EssentialsProvider,
    payload: &Value,
) -> Result<EssentialsProvider, Value> {
    provider
        .with_height(
            payload.get("height").and_then(|v| v.as_u64()),
            payload.get("height").is_some(),
        )
        .map_err(|e| {
            json!({
                "ok": false,
                "error": "missing_or_invalid_height",
                "detail": e.to_string()
            })
        })
}

pub fn register_rpc(reg: RpcNsRegistrar, provider: Arc<EssentialsProvider>) {
    let mdb = Arc::clone(&provider);

    eprintln!("[RPC::ESSENTIALS] registering RPC handlers…");

    {
        let reg_mem = reg.clone();
        let mdb_mem = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_mem
                .register("get_mempool_traces", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_mem);
                    async move {
                        // Refuse rather than return an empty trace list: with
                        // ingest off there is nothing pending to trace, and an
                        // empty `traces` array is indistinguishable from "the
                        // mempool is quiet right now".
                        if let Err(unavailable) = mempool_availability() {
                            return json!({
                                "ok": false,
                                "error": unavailable.reason(),
                                "message": unavailable.message(),
                            });
                        }
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetMempoolTracesParams {
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                            address: payload
                                .get("address")
                                .and_then(|v| v.as_str())
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty()),
                            fee_paid: payload.get("fee_paid").and_then(|v| v.as_f64()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_mempool_traces(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_mem_stats = reg.clone();
        tokio::spawn(async move {
            reg_mem_stats
                .register("get_mempool_memory_stats", move |_cx, _payload| async move {
                    // Distinguish "switched off" from the pre-existing generic
                    // unavailable, so an operator reading this can tell a
                    // configured shutdown from a fault.
                    if let Err(unavailable) = mempool_availability() {
                        return json!({
                            "ok": false,
                            "error": unavailable.reason(),
                            "message": unavailable.message(),
                        });
                    }
                    match current_mempool_memory_stats() {
                        Some(stats) => json!({"ok": true, "stats": stats}),
                        None => json!({"ok": false, "error": "mempool_unavailable"}),
                    }
                })
                .await;
        });
    }

    {
        let reg_get = reg.clone();
        let mdb_get = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_get
                .register("get_keys", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_get);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let keys = payload.get("keys").and_then(|v| v.as_array()).map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect::<Vec<String>>()
                        });
                        let params = RpcGetKeysParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            try_decode_utf8: payload
                                .get("try_decode_utf8")
                                .and_then(|v| v.as_bool()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            keys,
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_keys(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_all = reg.clone();
        let mdb_all = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_all
                .register("get_all_alkanes", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_all);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAllAlkanesParams {
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_all_alkanes(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_search = reg.clone();
        let mdb_search = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_search
                .register("search_alkane", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_search);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcSearchAlkaneParams {
                            prefix: payload
                                .get("prefix")
                                .and_then(|v| v.as_str())
                                .map(str::to_string),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_search_alkane(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_info = reg.clone();
        let mdb_info = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_info
                .register("get_alkane_info", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_info);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAlkaneInfoParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_alkane_info(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_alkabi = reg.clone();
        let mdb_alkabi = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_alkabi
                .register("get_alkabi", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_alkabi);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(view) => view,
                            Err(error) => return error,
                        };
                        let params = RpcGetAlkabiParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|value| value.as_str())
                                .map(str::to_string),
                            format: payload
                                .get("format")
                                .and_then(|value| value.as_str())
                                .map(str::to_string),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_alkabi(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_wasm = reg.clone();
        let mdb_wasm = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_wasm
                .register("get_alkane_wasm", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_wasm);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(view) => view,
                            Err(error) => return error,
                        };
                        let params = RpcGetAlkaneWasmParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|value| value.as_str())
                                .map(str::to_string),
                            gzip: payload.get("gzip").and_then(|value| value.as_bool()),
                            resolve: payload
                                .get("resolve")
                                .and_then(|value| value.as_bool())
                                // `no_resolution: true` is accepted as the
                                // inverse spelling of `resolve: false`.
                                .or_else(|| {
                                    payload
                                        .get("no_resolution")
                                        .and_then(|value| value.as_bool())
                                        .map(|no_resolution| !no_resolution)
                                }),
                            first_version: payload
                                .get("first_version")
                                .and_then(|value| value.as_bool()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_alkane_wasm(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_factory_children = reg.clone();
        let mdb_factory_children = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_factory_children
                .register("get_factory_children", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_factory_children);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetFactoryChildrenParams {
                            factory: payload
                                .get("factory")
                                .or_else(|| payload.get("factory_alkane"))
                                .or_else(|| payload.get("alkane"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_factory_children(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_search_factory_keys = reg.clone();
        let mdb_search_factory_keys = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_search_factory_keys
                .register("search_factory_keys", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_search_factory_keys);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcSearchFactoryKeysParams {
                            factory: payload
                                .get("factory")
                                .or_else(|| payload.get("factory_alkane"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            conditions: payload
                                .get("conditions")
                                .and_then(|v| v.as_array())
                                .map(|arr| arr.to_vec()),
                            key: payload.get("key").and_then(|v| v.as_str()).map(|s| s.to_string()),
                            op: payload.get("op").and_then(|v| v.as_str()).map(|s| s.to_string()),
                            value: payload.get("value").cloned(),
                            try_decode_utf8: payload
                                .get("try_decode_utf8")
                                .and_then(|v| v.as_bool()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_search_factory_keys(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_summary = reg.clone();
        let mdb_summary = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_summary
                .register("get_block_summary", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_summary);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetBlockSummaryParams {
                            height: payload.get("height").and_then(|v| v.as_u64()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_block_summary(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_block_time = reg.clone();
        let mdb_block_time = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_block_time
                .register("get_block_time", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_block_time);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetBlockTimeParams {
                            height: payload.get("height").and_then(|v| v.as_u64()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_block_time(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_block_times = reg.clone();
        let mdb_block_times = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_block_times
                .register("get_block_times", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_block_times);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let heights = payload.get("heights").and_then(|value| {
                            value.as_array().and_then(|items| {
                                items.iter().map(Value::as_u64).collect::<Option<Vec<_>>>()
                            })
                        });
                        let params = RpcGetBlockTimesParams { heights };
                        tokio::task::spawn_blocking(move || view.rpc_get_block_times(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_holders = reg.clone();
        let mdb_holders = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_holders
                .register("get_holders", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_holders);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetHoldersParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_holders(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_orbital_holders = reg.clone();
        let mdb_orbital_holders = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_orbital_holders
                .register("get_orbital_holders", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_orbital_holders);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetOrbitalHoldersParams {
                            factory: payload
                                .get("factory")
                                .or_else(|| payload.get("factory_alkane"))
                                .or_else(|| payload.get("alkane"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_orbital_holders(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_orbital_send_volumes = reg.clone();
        let mdb_orbital_send_volumes = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_orbital_send_volumes
                .register("get_orbital_send_volumes", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_orbital_send_volumes);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetOrbitalVolumesParams {
                            factory: payload
                                .get("factory")
                                .or_else(|| payload.get("factory_alkane"))
                                .or_else(|| payload.get("orbital"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        tokio::task::spawn_blocking(move || {
                            view.rpc_get_orbital_send_volumes(params)
                        })
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .map(|response| response.value)
                        .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_orbital_balances = reg.clone();
        let mdb_orbital_balances = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_orbital_balances
                .register("get_orbital_balances", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_orbital_balances);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetOrbitalBalancesParams {
                            address: payload
                                .get("address")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_orbital_balances(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_orbital_receive_volumes = reg.clone();
        let mdb_orbital_receive_volumes = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_orbital_receive_volumes
                .register("get_orbital_receive_volumes", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_orbital_receive_volumes);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetOrbitalVolumesParams {
                            factory: payload
                                .get("factory")
                                .or_else(|| payload.get("factory_alkane"))
                                .or_else(|| payload.get("orbital"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        tokio::task::spawn_blocking(move || {
                            view.rpc_get_orbital_receive_volumes(params)
                        })
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .map(|response| response.value)
                        .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_alkane_send_volumes = reg.clone();
        let mdb_alkane_send_volumes = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_alkane_send_volumes
                .register("get_alkane_send_volumes", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_alkane_send_volumes);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAlkaneVolumesParams {
                            source_alkane: payload
                                .get("source_alkane")
                                .or_else(|| payload.get("source"))
                                .or_else(|| payload.get("contract"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        tokio::task::spawn_blocking(move || {
                            view.rpc_get_alkane_send_volumes(params)
                        })
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .map(|response| response.value)
                        .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_alkane_receive_volumes = reg.clone();
        let mdb_alkane_receive_volumes = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_alkane_receive_volumes
                .register("get_alkane_receive_volumes", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_alkane_receive_volumes);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAlkaneVolumesParams {
                            source_alkane: payload
                                .get("source_alkane")
                                .or_else(|| payload.get("source"))
                                .or_else(|| payload.get("contract"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        tokio::task::spawn_blocking(move || {
                            view.rpc_get_alkane_receive_volumes(params)
                        })
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .map(|response| response.value)
                        .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_transfer = reg.clone();
        let mdb_transfer = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_transfer
                .register("get_transfer_volume", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_transfer);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetTransferVolumeParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_transfer_volume(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_received = reg.clone();
        let mdb_received = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_received
                .register("get_total_received", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_received);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetTotalReceivedParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_total_received(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_supply = reg.clone();
        let mdb_supply = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_supply
                .register("get_circulating_supply", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_supply);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetCirculatingSupplyParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            height: payload.get("height").and_then(|v| v.as_u64()),
                            height_present: payload.get("height").is_some(),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_circulating_supply(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_activity = reg.clone();
        let mdb_activity = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_activity
                .register("get_address_activity", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_activity);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAddressActivityParams {
                            address: payload
                                .get("address")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_address_activity(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_send_alkanes = reg.clone();
        let mdb_send_alkanes = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_send_alkanes
                .register("address_cumulative_send_alkanes", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_send_alkanes);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAddressCumulativeAlkanesParams {
                            address: payload
                                .get("address")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        tokio::task::spawn_blocking(move || {
                            view.rpc_address_cumulative_send_alkanes(params)
                        })
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .map(|response| response.value)
                        .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_receive_alkanes = reg.clone();
        let mdb_receive_alkanes = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_receive_alkanes
                .register("address_cumulative_receive_alkanes", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_receive_alkanes);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAddressCumulativeAlkanesParams {
                            address: payload
                                .get("address")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        tokio::task::spawn_blocking(move || {
                            view.rpc_address_cumulative_receive_alkanes(params)
                        })
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .map(|response| response.value)
                        .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_send_orbitals = reg.clone();
        let mdb_send_orbitals = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_send_orbitals
                .register("address_cumulative_send_orbitals", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_send_orbitals);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAddressCumulativeAlkanesParams {
                            address: payload
                                .get("address")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        tokio::task::spawn_blocking(move || {
                            view.rpc_address_cumulative_send_orbitals(params)
                        })
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .map(|response| response.value)
                        .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_receive_orbitals = reg.clone();
        let mdb_receive_orbitals = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_receive_orbitals
                .register("address_cumulative_receive_orbitals", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_receive_orbitals);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAddressCumulativeAlkanesParams {
                            address: payload
                                .get("address")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        tokio::task::spawn_blocking(move || {
                            view.rpc_address_cumulative_receive_orbitals(params)
                        })
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .map(|response| response.value)
                        .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_addr_bal = reg.clone();
        let mdb_addr_bal = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_addr_bal
                .register("get_address_balances", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_addr_bal);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAddressBalancesParams {
                            address: payload
                                .get("address")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            include_outpoints: payload
                                .get("include_outpoints")
                                .and_then(|v| v.as_bool()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_address_balances(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_alk_bal = reg.clone();
        let mdb_alk_bal = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_alk_bal
                .register("get_alkane_balances", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_alk_bal);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAlkaneBalancesParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            height: payload.get("height").and_then(|v| v.as_u64()),
                            height_present: payload.get("height").is_some(),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_alkane_balances(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_live_bal = reg.clone();
        let mdb_live_bal = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_live_bal
                .register("get_alkane_balance_metashrew", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_live_bal);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let height_present = payload.get("height").is_some();
                        let params = RpcGetAlkaneBalanceMetashrewParams {
                            owner: payload
                                .get("owner")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            target: payload
                                .get("alkane")
                                .or_else(|| payload.get("target"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            height: payload.get("height").and_then(|v| v.as_u64()),
                            height_present,
                        };
                        tokio::task::spawn_blocking(move || {
                            view.rpc_get_alkane_balance_metashrew(params)
                        })
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .map(|response| response.value)
                        .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_bal_txs = reg.clone();
        let mdb_bal_txs = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_bal_txs
                .register("get_alkane_balance_txs", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_bal_txs);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAlkaneBalanceTxsParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                            cursor: payload
                                .get("cursor")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_alkane_balance_txs(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_bal_txs_tok = reg.clone();
        let mdb_bal_txs_tok = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_bal_txs_tok
                .register("get_alkane_balance_txs_by_token", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_bal_txs_tok);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAlkaneBalanceTxsByTokenParams {
                            owner: payload
                                .get("owner")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            token: payload
                                .get("token")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                            cursor: payload
                                .get("cursor")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        tokio::task::spawn_blocking(move || {
                            view.rpc_get_alkane_balance_txs_by_token(params)
                        })
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .map(|response| response.value)
                        .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_op_bal = reg.clone();
        let mdb_op_bal = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_op_bal
                .register("get_outpoint_balances", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_op_bal);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetOutpointBalancesParams {
                            outpoint: payload
                                .get("outpoint")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        // 🔴 spawn_blocking IS LOAD-BEARING — do not inline this
                        // back into the async body.
                        //
                        // rpc_get_outpoint_balances is a SYNCHRONOUS RocksDB
                        // read. Awaiting it directly on a Tokio worker parks
                        // that worker for the whole read, so concurrent callers
                        // do not overlap — they queue. Measured from the
                        // explorer side: 380ms at N=1, 3,794ms with ten in
                        // flight, and 349ms -> 6.2s at N=100. That is linear,
                        // i.e. perfect serialization, and it is why concurrency
                        // bought nothing and only issuing FEWER calls helped.
                        //
                        // This is the single hottest call the explorer makes —
                        // one per non-OP_RETURN outpoint when assembling a
                        // TxView. A 4-minute sample showed 6,034 of these, some
                        // taking 10.6s, and it is what capped
                        // ADDRESS_HISTORY_ENRICH at 10 in
                        // apps/explorer/lib/queries.ts (see the latency-budget
                        // note there). Raise that cap only after re-measuring
                        // N=1/10/100 against THIS build.
                        //
                        // Matches the idiom already used by get_alkabi and
                        // get_alkane_wasm in this file.
                        tokio::task::spawn_blocking(move || view.rpc_get_outpoint_balances(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_runtime_bal = reg.clone();
        let mdb_runtime_bal = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_runtime_bal
                .register("get_runtime_balances_metashrew", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_runtime_bal);
                    async move {
                        // Takes no parameters — there is one runtime sheet for
                        // the whole protocol. The view is still resolved so the
                        // method is registered like every other one here.
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        tokio::task::spawn_blocking(move || {
                            view.rpc_get_runtime_balances_metashrew(
                                RpcGetRuntimeBalancesMetashrewParams {},
                            )
                        })
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .map(|response| response.value)
                        .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_traces = reg.clone();
        let mdb_traces = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_traces
                .register("get_block_traces", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_traces);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetBlockTracesParams {
                            height: payload.get("height").and_then(|v| v.as_u64()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_block_traces(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_holders_count = reg.clone();
        let mdb_holders_count = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_holders_count
                .register("get_holders_count", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_holders_count);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetHoldersCountParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_holders_count(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_addr_ops = reg.clone();
        let mdb_addr_ops = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_addr_ops
                .register("get_address_outpoints", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_addr_ops);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAddressOutpointsParams {
                            address: payload
                                .get("address")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_address_outpoints(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_addr_spendable_ops = reg.clone();
        let mdb_addr_spendable_ops = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_addr_spendable_ops
                .register("get_address_spendable_outpoints", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_addr_spendable_ops);
                    async move {
                        let params = RpcGetAddressSpendableOutpointsParams {
                            address: payload
                                .get("address")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            omit_raw_tx: payload.get("omit_raw_tx").and_then(|v| v.as_bool()),
                        };
                        mdb.rpc_get_address_spendable_outpoints(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_tx_summary = reg.clone();
        let mdb_tx_summary = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_tx_summary
                .register("get_alkane_tx_summary", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_tx_summary);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAlkaneTxSummaryParams {
                            txid: payload
                                .get("txid")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_alkane_tx_summary(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_block_txs = reg.clone();
        let mdb_block_txs = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_block_txs
                .register("get_alkane_block_txs", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_block_txs);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAlkaneBlockTxsParams {
                            height: payload.get("height").and_then(|v| v.as_u64()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_alkane_block_txs(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_addr_txs = reg.clone();
        let mdb_addr_txs = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_addr_txs
                .register("get_alkane_address_txs", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_addr_txs);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAlkaneAddressTxsParams {
                            address: payload
                                .get("address")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        tokio::task::spawn_blocking(move || view.rpc_get_alkane_address_txs(params))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_addr_txs = reg.clone();
        let mdb_addr_txs = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_addr_txs
                .register("get_address_transactions", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_addr_txs);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        let params = RpcGetAddressTransactionsParams {
                            address: payload
                                .get("address")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                            only_alkane_txs: payload
                                .get("only_alkane_txs")
                                .and_then(|v| v.as_bool()),
                            include_mempool: payload
                                .get("include_mempool")
                                .and_then(|v| v.as_bool()),
                            filter: payload
                                .get("filter")
                                .and_then(|v| v.as_str())
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty()),
                        };
                        tokio::task::spawn_blocking(move || {
                            view.rpc_get_address_transactions(params)
                        })
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .map(|response| response.value)
                        .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_latest_traces = reg.clone();
        let mdb_latest_traces = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_latest_traces
                .register("get_alkane_latest_traces", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_latest_traces);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        tokio::task::spawn_blocking(move || {
                            view.rpc_get_alkane_latest_traces(RpcGetAlkaneLatestTracesParams)
                        })
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .map(|response| response.value)
                        .unwrap_or_else(|| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_debug_timers = reg.clone();
        tokio::spawn(async move {
            reg_debug_timers
                .register("get_debug_timer_totals", move |_cx, payload| async move {
                    let limit = payload.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize);
                    let reset_requested =
                        payload.get("reset").and_then(|v| v.as_bool()).unwrap_or(false);
                    let reset_deleted = if reset_requested {
                        match crate::debug::reset_timer_totals() {
                            Ok(deleted) => Some(deleted),
                            Err(e) => {
                                return json!({
                                    "ok": false,
                                    "error": "timer_reset_failed",
                                    "message": e,
                                });
                            }
                        }
                    } else {
                        None
                    };
                    let snapshot = crate::debug::get_timer_totals(limit);
                    json!({
                        "ok": true,
                        "reset": reset_requested,
                        "reset_deleted": reset_deleted,
                        "timers": snapshot.entries,
                        "returned": snapshot.entries.len(),
                        "total_entries": snapshot.total_entries,
                        "total_ms": snapshot.total_ms,
                        "total_calls": snapshot.total_calls,
                    })
                })
                .await;
        });
    }

    {
        let reg_ping = reg.clone();
        let mdb_ping = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_ping
                .register("ping", move |_cx, payload| {
                    let mdb = Arc::clone(&mdb_ping);
                    async move {
                        let view = match resolve_view(mdb.as_ref(), &payload) {
                            Ok(v) => v,
                            Err(err) => return err,
                        };
                        tokio::task::spawn_blocking(move || view.rpc_ping(RpcPingParams))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .map(|response| response.value)
                            .unwrap_or_else(|| Value::String("pong".to_string()))
                    }
                })
                .await;
        });
    }
}
