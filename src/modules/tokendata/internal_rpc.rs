//! Getter RPCs for the tokendata module (see essentials/internal_rpc.rs for the
//! pattern): every tokendata getter the explorer calls is served as
//! `internal.tokendata_<getter>` so a remote espo explorer can fetch the getter's
//! native result in one round-trip.
//!
//! Simple params/results travel as plain serde JSON; activity pages (nested
//! `SchemaTokenActivityV1` rows) travel as hex-encoded Borsh, and the diesel
//! price getters transport their `[u8; 32]` prices as hex strings.

use crate::modules::defs::RpcNsRegistrar;
use crate::modules::tokendata::schemas::SchemaTokenActivityV1;
use crate::modules::tokendata::storage::{
    GetIndexHeightParams, GetIndexHeightResult, GetTokenActivityPageParams,
    GetTokenActivityPageResult, TokenDataProvider,
};
use crate::runtime::internal_rpc::{borsh_hex, borsh_unhex, register_getter};
use crate::runtime::remote_espo::RemoteEspoClient;
use crate::runtime::state_at::StateAt;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Provider pinned to the wire state so internal reads resolve against the
/// same view a locally height-pinned provider would use.
fn provider_at(state: StateAt) -> TokenDataProvider {
    TokenDataProvider::new(Arc::new(crate::config::espo_mdb(b"tokendata:")))
        .with_view_blockhash(state.to_option())
}

fn price_hex(price: &[u8; 32]) -> String {
    hex::encode(price)
}

fn price_from_hex(raw: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(raw).map_err(|e| anyhow!("bad price hex: {e}"))?;
    let mut out = [0u8; 32];
    if bytes.len() != 32 {
        return Err(anyhow!("bad price length {}", bytes.len()));
    }
    out.copy_from_slice(&bytes);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Wire structs for getters whose native params/results don't serde directly.
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
pub struct WireTokenActivityPageResult {
    /// Borsh hex of `Vec<SchemaTokenActivityV1>`.
    pub entries_borsh: String,
    pub total: u64,
}

#[derive(Serialize, Deserialize)]
pub struct WireDieselPriceByHeightParams {
    pub height: u32,
}

#[derive(Serialize, Deserialize)]
pub struct WireDieselPriceByHeightResult {
    /// 32 bytes, hex.
    pub price: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct WireDieselPricePointsParams {
    pub max_height: u32,
}

#[derive(Serialize, Deserialize)]
pub struct WireDieselPricePointsResult {
    /// `(height, 32-byte price hex)` pairs, ascending by height.
    pub points: Vec<(u32, String)>,
}

// ---------------------------------------------------------------------------
// Client-side helpers: the getters' remote branches call these.
// ---------------------------------------------------------------------------

pub fn remote_get_index_height(
    remote: &RemoteEspoClient,
    params: GetIndexHeightParams,
) -> Result<GetIndexHeightResult> {
    remote.getter("internal.tokendata_get_index_height", &params)
}

pub fn remote_get_token_activity_page(
    remote: &RemoteEspoClient,
    params: GetTokenActivityPageParams,
) -> Result<GetTokenActivityPageResult> {
    let r: WireTokenActivityPageResult =
        remote.getter("internal.tokendata_get_token_activity_page", &params)?;
    let entries: Vec<SchemaTokenActivityV1> = borsh_unhex(&r.entries_borsh)?;
    Ok(GetTokenActivityPageResult { entries, total: r.total as usize })
}

pub fn remote_get_diesel_avg_price_paid_usd_by_height(
    remote: &RemoteEspoClient,
    height: u32,
) -> Result<Option<[u8; 32]>> {
    let r: WireDieselPriceByHeightResult = remote.getter(
        "internal.tokendata_get_diesel_avg_price_paid_usd_by_height",
        &WireDieselPriceByHeightParams { height },
    )?;
    r.price.as_deref().map(price_from_hex).transpose()
}

pub fn remote_get_diesel_avg_price_paid_usd_points_through_height(
    remote: &RemoteEspoClient,
    max_height: u32,
) -> Result<Vec<(u32, [u8; 32])>> {
    let r: WireDieselPricePointsResult = remote.getter(
        "internal.tokendata_get_diesel_avg_price_paid_usd_points_through_height",
        &WireDieselPricePointsParams { max_height },
    )?;
    r.points
        .into_iter()
        .map(|(height, raw)| Ok((height, price_from_hex(&raw)?)))
        .collect()
}

// ---------------------------------------------------------------------------
// Server-side registrations.
// ---------------------------------------------------------------------------

pub fn register_internal_getters(reg: &RpcNsRegistrar) {
    register_getter(reg, "tokendata_get_index_height", |p: GetIndexHeightParams| {
        provider_at(p.blockhash).get_index_height(p)
    });
    register_getter(reg, "tokendata_get_token_activity_page", |p: GetTokenActivityPageParams| {
        let page = provider_at(p.blockhash).get_token_activity_page(p)?;
        Ok(WireTokenActivityPageResult {
            entries_borsh: borsh_hex(&page.entries)?,
            total: page.total as u64,
        })
    });
    register_getter(
        reg,
        "tokendata_get_diesel_avg_price_paid_usd_by_height",
        |p: WireDieselPriceByHeightParams| {
            let price =
                provider_at(StateAt::Latest).get_diesel_avg_price_paid_usd_by_height(p.height)?;
            Ok(WireDieselPriceByHeightResult { price: price.as_ref().map(price_hex) })
        },
    );
    register_getter(
        reg,
        "tokendata_get_diesel_avg_price_paid_usd_points_through_height",
        |p: WireDieselPricePointsParams| {
            let points = provider_at(StateAt::Latest)
                .get_diesel_avg_price_paid_usd_points_through_height(p.max_height)?;
            Ok(WireDieselPricePointsResult {
                points: points
                    .into_iter()
                    .map(|(height, price)| (height, price_hex(&price)))
                    .collect(),
            })
        },
    );
}
