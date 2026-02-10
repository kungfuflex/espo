use crate::modules::defs::RpcNsRegistrar;
use crate::modules::essentials::storage::{
    EssentialsProvider, RpcGetAddressActivityParams, RpcGetAddressBalancesParams,
    RpcGetAddressOutpointsParams, RpcGetAddressTransactionsParams, RpcGetAlkaneAddressTxsParams,
    RpcGetAlkaneBalanceMetashrewParams, RpcGetAlkaneBalanceTxsByTokenParams,
    RpcGetAlkaneBalanceTxsParams, RpcGetAlkaneBalancesParams, RpcGetAlkaneBlockTxsParams,
    RpcGetAlkaneInfoParams, RpcGetAlkaneLatestTracesParams, RpcGetAlkaneTxSummaryParams,
    RpcGetAllAlkanesParams, RpcGetBlockSummaryParams, RpcGetBlockTracesParams,
    RpcGetCirculatingSupplyParams, RpcGetHoldersCountParams, RpcGetHoldersParams, RpcGetKeysParams,
    RpcGetMempoolTracesParams, RpcGetOutpointBalancesParams, RpcGetTotalReceivedParams,
    RpcGetTransferVolumeParams, RpcPingParams,
};
use serde_json::{Value, json};
use std::sync::Arc;

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
                        let params = RpcGetMempoolTracesParams {
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                            address: payload
                                .get("address")
                                .and_then(|v| v.as_str())
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty()),
                        };
                        mdb.rpc_get_mempool_traces(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        mdb.rpc_get_keys(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        let params = RpcGetAllAlkanesParams {
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        mdb.rpc_get_all_alkanes(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        let params = RpcGetAlkaneInfoParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        mdb.rpc_get_alkane_info(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        let params = RpcGetBlockSummaryParams {
                            height: payload.get("height").and_then(|v| v.as_u64()),
                        };
                        mdb.rpc_get_block_summary(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        let params = RpcGetHoldersParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        mdb.rpc_get_holders(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        let params = RpcGetTransferVolumeParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        mdb.rpc_get_transfer_volume(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        let params = RpcGetTotalReceivedParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        mdb.rpc_get_total_received(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        let params = RpcGetCirculatingSupplyParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            height: payload.get("height").and_then(|v| v.as_u64()),
                            height_present: payload.get("height").is_some(),
                        };
                        mdb.rpc_get_circulating_supply(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        let params = RpcGetAddressActivityParams {
                            address: payload
                                .get("address")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        mdb.rpc_get_address_activity(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        let params = RpcGetAddressBalancesParams {
                            address: payload
                                .get("address")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            include_outpoints: payload
                                .get("include_outpoints")
                                .and_then(|v| v.as_bool()),
                        };
                        mdb.rpc_get_address_balances(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        let params = RpcGetAlkaneBalancesParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            height: payload.get("height").and_then(|v| v.as_u64()),
                            height_present: payload.get("height").is_some(),
                        };
                        mdb.rpc_get_alkane_balances(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        mdb.rpc_get_alkane_balance_metashrew(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        let params = RpcGetAlkaneBalanceTxsParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        mdb.rpc_get_alkane_balance_txs(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        };
                        mdb.rpc_get_alkane_balance_txs_by_token(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        let params = RpcGetOutpointBalancesParams {
                            outpoint: payload
                                .get("outpoint")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        mdb.rpc_get_outpoint_balances(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        let params = RpcGetBlockTracesParams {
                            height: payload.get("height").and_then(|v| v.as_u64()),
                        };
                        mdb.rpc_get_block_traces(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        let params = RpcGetHoldersCountParams {
                            alkane: payload
                                .get("alkane")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        mdb.rpc_get_holders_count(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        let params = RpcGetAddressOutpointsParams {
                            address: payload
                                .get("address")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        mdb.rpc_get_address_outpoints(params)
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
                        let params = RpcGetAlkaneTxSummaryParams {
                            txid: payload
                                .get("txid")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        };
                        mdb.rpc_get_alkane_tx_summary(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        let params = RpcGetAlkaneBlockTxsParams {
                            height: payload.get("height").and_then(|v| v.as_u64()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        mdb.rpc_get_alkane_block_txs(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        let params = RpcGetAlkaneAddressTxsParams {
                            address: payload
                                .get("address")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            page: payload.get("page").and_then(|v| v.as_u64()),
                            limit: payload.get("limit").and_then(|v| v.as_u64()),
                        };
                        mdb.rpc_get_alkane_address_txs(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                        };
                        mdb.rpc_get_address_transactions(params)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
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
                .register("get_alkane_latest_traces", move |_cx, _payload| {
                    let mdb = Arc::clone(&mdb_latest_traces);
                    async move {
                        mdb.rpc_get_alkane_latest_traces(RpcGetAlkaneLatestTracesParams)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| json!({"ok": false, "error": "internal_error"}))
                    }
                })
                .await;
        });
    }

    {
        let reg_ping = reg.clone();
        let mdb_ping = Arc::clone(&mdb);
        tokio::spawn(async move {
            reg_ping
                .register("ping", move |_cx, _payload| {
                    let mdb = Arc::clone(&mdb_ping);
                    async move {
                        mdb.rpc_ping(RpcPingParams)
                            .map(|resp| resp.value)
                            .unwrap_or_else(|_| Value::String("pong".to_string()))
                    }
                })
                .await;
        });
    }
}
