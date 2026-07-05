use super::schemas::{SchemaTokenActivityV1, TokenActivityKind};
use super::storage::{
    GetAddressActivityPageParams, GetTokenActivityPageParams, SortDir,
    TokenActivityQuoteAmountFilter, TokenActivityScope, TokenActivitySortField, TokenDataProvider,
};
use crate::config::get_network;
use crate::modules::ammdata::consts::{PRICE_SCALE, SATS_PER_BTC};
use crate::modules::ammdata::storage::AmmDataProvider;
use crate::modules::defs::RpcNsRegistrar;
use crate::schemas::SchemaAlkaneId;
use alloy_primitives::U256;
use bitcoin::Address;
use bitcoin::Txid;
use bitcoin::hashes::Hash;
use serde_json::json;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

fn parse_alkane_id(raw: &str) -> Option<SchemaAlkaneId> {
    let (block_raw, tx_raw) = raw.split_once(':')?;
    Some(SchemaAlkaneId { block: block_raw.parse().ok()?, tx: tx_raw.parse().ok()? })
}

fn parse_optional_alkane_id(raw: Option<&str>) -> Option<Option<SchemaAlkaneId>> {
    match raw {
        None => Some(None),
        Some("all") | Some("") => Some(None),
        Some(value) => parse_alkane_id(value).map(Some),
    }
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

fn parse_time(raw: Option<&serde_json::Value>) -> Option<Option<u64>> {
    match raw {
        None => Some(None),
        Some(v) => {
            if let Some(ts) = v.as_u64() {
                return Some(Some(ts));
            }
            if let Some(s) = v.as_str() {
                return s.trim().parse::<u64>().ok().map(Some);
            }
            None
        }
    }
}

fn parse_u128(raw: Option<&serde_json::Value>) -> Option<Option<u128>> {
    match raw {
        None => Some(None),
        Some(v) => {
            if let Some(n) = v.as_u64() {
                return Some(Some(n as u128));
            }
            if let Some(s) = v.as_str() {
                return s.trim().parse::<u128>().ok().map(Some);
            }
            None
        }
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

fn address_spk(address: &str) -> Option<Vec<u8>> {
    Address::from_str(address)
        .ok()
        .and_then(|a| a.require_network(get_network()).ok())
        .map(|a| a.script_pubkey().into_bytes())
}

fn scaled_u256_bytes_to_decimal(bytes: [u8; 32]) -> Option<String> {
    let value = U256::from_be_bytes(bytes);
    if value.is_zero() {
        return None;
    }
    let scale = U256::from(10_000_000_000_000_000u128);
    let whole = value / scale;
    let frac = value % scale;
    if frac.is_zero() {
        return Some(whole.to_string());
    }
    let mut frac_str = frac.to_string();
    if frac_str.len() < 16 {
        frac_str = format!("{}{}", "0".repeat(16 - frac_str.len()), frac_str);
    }
    while frac_str.ends_with('0') {
        frac_str.pop();
    }
    Some(format!("{whole}.{frac_str}"))
}

fn scaled_u256_bytes(bytes: [u8; 32]) -> U256 {
    U256::from_be_bytes(bytes)
}

fn row_json(row: &SchemaTokenActivityV1, btc_price_usd_scaled: Option<u128>) -> serde_json::Value {
    let mint_price_paid_usd = btc_price_usd_scaled.and_then(|btc_usd| {
        let paid_sats = scaled_u256_bytes(row.mint_price_paid_sats);
        if paid_sats.is_zero() {
            return None;
        }
        let usd = paid_sats.saturating_mul(U256::from(btc_usd))
            / U256::from(PRICE_SCALE.saturating_mul(SATS_PER_BTC));
        scaled_u256_bytes_to_decimal(usd.to_be_bytes::<32>())
    });
    json!({
        "height": row.height,
        "timestamp": row.timestamp,
        "txid": Txid::from_byte_array(row.txid).to_string(),
        "chain_txids": row
            .chain_txids
            .iter()
            .map(|txid| Txid::from_byte_array(*txid).to_string())
            .collect::<Vec<_>>(),
        "cpfp": row.cpfp,
        "mint_price_paid_sats": scaled_u256_bytes_to_decimal(row.mint_price_paid_sats),
        "mint_price_paid_usd": mint_price_paid_usd,
        "mint_price_pool_usd": scaled_u256_bytes_to_decimal(row.mint_price_pool_usd),
        "mint_price_pool_frbtc_sats": scaled_u256_bytes_to_decimal(row.mint_price_pool_frbtc_sats),
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

fn btc_price_cache_for_rows(
    rows: &[SchemaTokenActivityV1],
    amm_provider: &AmmDataProvider,
) -> HashMap<u32, Option<u128>> {
    let mut cache = HashMap::<u32, Option<u128>>::new();
    for row in rows {
        cache.entry(row.height).or_insert_with(|| {
            amm_provider.get_btc_usd_price_at_or_before_height(row.height).ok().flatten()
        });
    }
    cache
}

pub fn register_rpc(
    reg: &RpcNsRegistrar,
    provider: Arc<TokenDataProvider>,
    amm_provider: Arc<AmmDataProvider>,
) {
    let reg_token_activity = reg.clone();
    let provider_token_activity = Arc::clone(&provider);
    let amm_provider_token_activity = Arc::clone(&amm_provider);
    tokio::spawn(async move {
        reg_token_activity
            .register("get_token_activity", move |_cx, payload| {
                let provider = Arc::clone(&provider_token_activity);
                let amm_provider = Arc::clone(&amm_provider_token_activity);
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
                    let quote_amount_filter = match (
                        payload
                            .get("canonical_quote")
                            .or_else(|| payload.get("quote"))
                            .and_then(|v| v.as_str())
                            .and_then(parse_alkane_id),
                        parse_u128(
                            payload
                                .get("min_quote_amount")
                                .or_else(|| payload.get("min_amount"))
                                .or_else(|| payload.get("amount")),
                        ),
                    ) {
                        (Some(quote), Some(Some(min_amount))) => {
                            Some(TokenActivityQuoteAmountFilter { quote, min_amount })
                        }
                        _ => None,
                    };
                    let Some(start_time) = parse_time(
                        payload
                            .get("start_time")
                            .or_else(|| payload.get("start_ts"))
                            .or_else(|| payload.get("target_start"))
                            .or_else(|| payload.get("from")),
                    ) else {
                        return json!({ "ok": false, "error": "missing_or_invalid_start_time" });
                    };
                    let Some(end_time) = parse_time(
                        payload
                            .get("end_time")
                            .or_else(|| payload.get("end_ts"))
                            .or_else(|| payload.get("target_end"))
                            .or_else(|| payload.get("to")),
                    ) else {
                        return json!({ "ok": false, "error": "missing_or_invalid_end_time" });
                    };
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
                        start_time,
                        end_time,
                        quote_amount_filter,
                    }) {
                        Ok(resp) => {
                            let btc_price_cache =
                                btc_price_cache_for_rows(&resp.entries, amm_provider.as_ref());
                            let entries = resp
                                .entries
                                .iter()
                                .map(|row| {
                                    let btc_price =
                                        btc_price_cache.get(&row.height).copied().flatten();
                                    row_json(row, btc_price)
                                })
                                .collect::<Vec<_>>();
                            json!({
                                "ok": true,
                                "total": resp.total,
                                "entries": entries,
                            })
                        }
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

    let reg_address_activity = reg.clone();
    let provider_address_activity = Arc::clone(&provider);
    let amm_provider_address_activity = Arc::clone(&amm_provider);
    tokio::spawn(async move {
        reg_address_activity
            .register("get_address_activity", move |_cx, payload| {
                let provider = Arc::clone(&provider_address_activity);
                let amm_provider = Arc::clone(&amm_provider_address_activity);
                async move {
                    let Some(address) = payload.get("address").and_then(|v| v.as_str()) else {
                        return json!({ "ok": false, "error": "missing_or_invalid_address" });
                    };
                    let Some(address_spk) = address_spk(address) else {
                        return json!({ "ok": false, "error": "missing_or_invalid_address" });
                    };
                    let Some(token) = parse_optional_alkane_id(
                        payload
                            .get("token")
                            .or_else(|| payload.get("alkane"))
                            .and_then(|v| v.as_str()),
                    ) else {
                        return json!({ "ok": false, "error": "missing_or_invalid_token" });
                    };
                    let limit = payload.get("limit").and_then(|v| v.as_u64()).unwrap_or(50);
                    let page = payload.get("page").and_then(|v| v.as_u64()).unwrap_or(1).max(1);
                    let Some(start_time) = parse_time(
                        payload
                            .get("start_time")
                            .or_else(|| payload.get("start_ts"))
                            .or_else(|| payload.get("target_start"))
                            .or_else(|| payload.get("from")),
                    ) else {
                        return json!({ "ok": false, "error": "missing_or_invalid_start_time" });
                    };
                    let Some(end_time) = parse_time(
                        payload
                            .get("end_time")
                            .or_else(|| payload.get("end_ts"))
                            .or_else(|| payload.get("target_end"))
                            .or_else(|| payload.get("to")),
                    ) else {
                        return json!({ "ok": false, "error": "missing_or_invalid_end_time" });
                    };
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
                    match view.get_address_activity_page(GetAddressActivityPageParams {
                        blockhash: crate::runtime::state_at::StateAt::Latest,
                        address_spk,
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
                        start_time,
                        end_time,
                    }) {
                        Ok(resp) => {
                            let btc_price_cache =
                                btc_price_cache_for_rows(&resp.entries, amm_provider.as_ref());
                            let entries = resp
                                .entries
                                .iter()
                                .map(|row| {
                                    let btc_price =
                                        btc_price_cache.get(&row.height).copied().flatten();
                                    row_json(row, btc_price)
                                })
                                .collect::<Vec<_>>();
                            json!({
                                "ok": true,
                                "total": resp.total,
                                "entries": entries,
                            })
                        }
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
