use super::schemas::{SchemaTokenActivityV1, TokenActivityKind};
use super::storage::{
    GetTokenActivityPageParams, SortDir, TokenActivityScope, TokenActivitySortField,
    TokenDataProvider,
};
use crate::modules::defs::RpcNsRegistrar;
use crate::schemas::SchemaAlkaneId;
use bitcoin::hashes::Hash;
use bitcoin::Txid;
use serde_json::json;
use std::sync::Arc;

fn parse_alkane_id(raw: &str) -> Option<SchemaAlkaneId> {
    let (block_raw, tx_raw) = raw.split_once(':')?;
    Some(SchemaAlkaneId { block: block_raw.parse().ok()?, tx: tx_raw.parse().ok()? })
}

fn parse_scope(raw: Option<&str>) -> TokenActivityScope {
    match raw {
        Some("market") | Some("amm") => TokenActivityScope::Market,
        Some("mint") | Some("mints") => TokenActivityScope::Mint,
        _ => TokenActivityScope::All,
    }
}

fn parse_sort(raw: Option<&str>) -> TokenActivitySortField {
    match raw {
        Some("amount") | Some("volume") => TokenActivitySortField::Amount,
        _ => TokenActivitySortField::Timestamp,
    }
}

fn parse_dir(raw: Option<&str>) -> SortDir {
    match raw {
        Some("asc") => SortDir::Asc,
        _ => SortDir::Desc,
    }
}

fn parse_kind(raw: Option<&str>) -> Option<TokenActivityKind> {
    match raw {
        Some("buy") | Some("trade_buy") => Some(TokenActivityKind::Buy),
        Some("sell") | Some("trade_sell") => Some(TokenActivityKind::Sell),
        Some("liquidity_add") | Some("add") => Some(TokenActivityKind::LiquidityAdd),
        Some("liquidity_remove") | Some("remove") => Some(TokenActivityKind::LiquidityRemove),
        Some("pool_create") | Some("create") => Some(TokenActivityKind::PoolCreate),
        Some("mint") => Some(TokenActivityKind::Mint),
        _ => None,
    }
}

fn row_json(row: &SchemaTokenActivityV1) -> serde_json::Value {
    json!({
        "timestamp": row.timestamp,
        "txid": Txid::from_byte_array(row.txid).to_string(),
        "token": format!("{}:{}", row.token.block, row.token.tx),
        "kind": match row.kind {
            TokenActivityKind::Buy => "buy",
            TokenActivityKind::Sell => "sell",
            TokenActivityKind::LiquidityAdd => "liquidity_add",
            TokenActivityKind::LiquidityRemove => "liquidity_remove",
            TokenActivityKind::PoolCreate => "pool_create",
            TokenActivityKind::Mint => "mint",
        },
        "source": match row.source {
            super::schemas::TokenActivitySource::Market => "market",
            super::schemas::TokenActivitySource::Mint => "mint",
        },
        "pool": row.pool.map(|p| format!("{}:{}", p.block, p.tx)),
        "counter_token": row.counter_token.map(|p| format!("{}:{}", p.block, p.tx)),
        "token_delta": row.token_delta.to_string(),
        "counter_delta": row.counter_delta.to_string(),
        "address_spk": hex::encode(&row.address_spk),
        "success": row.success,
    })
}

pub fn register_rpc(reg: &RpcNsRegistrar, provider: Arc<TokenDataProvider>) {
    let reg_token_activity = reg.clone();
    tokio::spawn(async move {
        reg_token_activity
            .register("get_token_activity", move |_cx, payload| {
                let provider = Arc::clone(&provider);
                async move {
                    let Some(token) = payload
                        .get("token")
                        .or_else(|| payload.get("alkane"))
                        .and_then(|v| v.as_str())
                        .and_then(parse_alkane_id)
                    else {
                        return json!({
                            "ok": false,
                            "error": "missing_or_invalid_token"
                        });
                    };
                    let limit = payload.get("limit").and_then(|v| v.as_u64()).unwrap_or(50);
                    let page = payload.get("page").and_then(|v| v.as_u64()).unwrap_or(1).max(1);
                    let view = match provider.with_height(
                        payload.get("height").and_then(|v| v.as_u64()),
                        payload.get("height").is_some(),
                    ) {
                        Ok(v) => v,
                        Err(e) => {
                            return json!({
                                "ok": false,
                                "error": "missing_or_invalid_height",
                                "detail": e.to_string()
                            });
                        }
                    };
                    match view.get_token_activity_page(GetTokenActivityPageParams {
                        blockhash: crate::runtime::state_at::StateAt::Latest,
                        token,
                        offset: usize::try_from((page - 1).saturating_mul(limit)).unwrap_or(0),
                        limit: usize::try_from(limit).unwrap_or(50),
                        kind: parse_kind(payload.get("kind").and_then(|v| v.as_str())),
                        scope: parse_scope(
                            payload
                                .get("activity_type")
                                .or_else(|| payload.get("type"))
                                .or_else(|| payload.get("filter"))
                                .and_then(|v| v.as_str()),
                        ),
                        sort_by: parse_sort(
                            payload
                                .get("sort")
                                .or_else(|| payload.get("sort_by"))
                                .and_then(|v| v.as_str()),
                        ),
                        dir: parse_dir(payload.get("dir").and_then(|v| v.as_str())),
                    }) {
                        Ok(resp) => json!({
                            "ok": true,
                            "total": resp.total,
                            "entries": resp.entries.iter().map(row_json).collect::<Vec<_>>(),
                        }),
                        Err(e) => json!({
                            "ok": false,
                            "error": "internal_error",
                            "detail": e.to_string()
                        }),
                    }
                }
            })
            .await;
    });
}
