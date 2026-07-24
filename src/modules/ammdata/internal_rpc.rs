//! Getter RPCs for the ammdata module (see essentials/internal_rpc.rs for the
//! pattern): every ammdata getter the explorer calls is served as
//! `internal.ammdata_<getter>` so a remote espo explorer can fetch the getter's
//! native result in one round-trip.
//!
//! All converted ammdata getters have serde-friendly params/results, so
//! everything travels as plain serde JSON. Server handlers pin the provider's
//! view to the wire blockhash so getters that read `StateAt::Latest`
//! internally still serve height-pinned views correctly.

use crate::config::get_espo_db;
use crate::modules::ammdata::storage::{
    AmmDataProvider, GetIndexHeightParams, GetIndexHeightResult, GetLatestBtcUsdPriceParams,
    GetListEntriesDescParams, GetListEntriesDescResult, GetListKeysByPrefixParams,
    GetListKeysByPrefixResult, GetPoolDefsParams, GetPoolDefsResult, GetTokenActivityPageParams,
    GetTokenActivityPageResult, GetTokenDerivedMetricsParams, GetTokenDerivedMetricsResult,
    GetTokenMetricsParams, GetTokenMetricsResult, GetTokenPoolsParams, GetTokenPoolsResult,
    GetTokenSearchIndexPageParams, GetTokenSearchIndexPageResult, RpcGetCandlesParams,
    RpcGetCandlesResult,
};
use crate::modules::defs::RpcNsRegistrar;
use crate::modules::essentials::storage::EssentialsProvider;
use crate::runtime::internal_rpc::register_getter;
use crate::runtime::mdb::Mdb;
use crate::runtime::remote_espo::RemoteEspoClient;
use crate::runtime::state_at::StateAt;
use anyhow::Result;
use std::sync::Arc;

/// Provider pinned to the wire state: getters that hardcode `StateAt::Latest`
/// internally resolve it against the view blockhash, so pinning here makes the
/// remote result match what a locally height-pinned provider would return.
fn provider_at(state: StateAt) -> AmmDataProvider {
    let db = get_espo_db();
    let essentials =
        EssentialsProvider::new(Arc::new(Mdb::from_db(Arc::clone(&db), b"essentials:")));
    AmmDataProvider::new(Arc::new(Mdb::from_db(db, b"ammdata:")), Arc::new(essentials))
        .with_view_blockhash(state.to_option())
}

// ---------------------------------------------------------------------------
// Client-side helpers: the getters' remote branches call these.
// ---------------------------------------------------------------------------

pub fn remote_get_latest_btc_usd_price(
    remote: &RemoteEspoClient,
    params: GetLatestBtcUsdPriceParams,
) -> Result<Option<u128>> {
    remote.getter("internal.ammdata_get_latest_btc_usd_price", &params)
}

pub fn remote_get_pool_defs(
    remote: &RemoteEspoClient,
    params: GetPoolDefsParams,
) -> Result<GetPoolDefsResult> {
    remote.getter("internal.ammdata_get_pool_defs", &params)
}

pub fn remote_get_token_metrics(
    remote: &RemoteEspoClient,
    params: GetTokenMetricsParams,
) -> Result<GetTokenMetricsResult> {
    remote.getter("internal.ammdata_get_token_metrics", &params)
}

pub fn remote_get_token_derived_metrics(
    remote: &RemoteEspoClient,
    params: GetTokenDerivedMetricsParams,
) -> Result<GetTokenDerivedMetricsResult> {
    remote.getter("internal.ammdata_get_token_derived_metrics", &params)
}

pub fn remote_get_token_pools(
    remote: &RemoteEspoClient,
    params: GetTokenPoolsParams,
) -> Result<GetTokenPoolsResult> {
    remote.getter("internal.ammdata_get_token_pools", &params)
}

pub fn remote_get_token_activity_page(
    remote: &RemoteEspoClient,
    params: GetTokenActivityPageParams,
) -> Result<GetTokenActivityPageResult> {
    remote.getter("internal.ammdata_get_token_activity_page", &params)
}

pub fn remote_get_token_search_index_page(
    remote: &RemoteEspoClient,
    params: GetTokenSearchIndexPageParams,
) -> Result<GetTokenSearchIndexPageResult> {
    remote.getter("internal.ammdata_get_token_search_index_page", &params)
}

pub fn remote_rpc_get_candles(
    remote: &RemoteEspoClient,
    params: RpcGetCandlesParams,
) -> Result<RpcGetCandlesResult> {
    remote.getter("internal.ammdata_rpc_get_candles", &params)
}

pub fn remote_get_index_height(
    remote: &RemoteEspoClient,
    params: GetIndexHeightParams,
) -> Result<GetIndexHeightResult> {
    remote.getter("internal.ammdata_get_index_height", &params)
}

pub fn remote_get_list_entries_desc(
    remote: &RemoteEspoClient,
    params: GetListEntriesDescParams,
) -> Result<GetListEntriesDescResult> {
    remote.getter("internal.ammdata_get_list_entries_desc", &params)
}

pub fn remote_get_list_keys_by_prefix(
    remote: &RemoteEspoClient,
    params: GetListKeysByPrefixParams,
) -> Result<GetListKeysByPrefixResult> {
    remote.getter("internal.ammdata_get_list_keys_by_prefix", &params)
}

// ---------------------------------------------------------------------------
// Server-side registrations.
// ---------------------------------------------------------------------------

pub fn register_internal_getters(reg: &RpcNsRegistrar) {
    register_getter(reg, "ammdata_get_latest_btc_usd_price", |p: GetLatestBtcUsdPriceParams| {
        provider_at(p.blockhash).get_latest_btc_usd_price(p)
    });
    register_getter(reg, "ammdata_get_pool_defs", |p: GetPoolDefsParams| {
        provider_at(p.blockhash).get_pool_defs(p)
    });
    register_getter(reg, "ammdata_get_token_metrics", |p: GetTokenMetricsParams| {
        provider_at(p.blockhash).get_token_metrics(p)
    });
    register_getter(reg, "ammdata_get_token_derived_metrics", |p: GetTokenDerivedMetricsParams| {
        provider_at(p.blockhash).get_token_derived_metrics(p)
    });
    register_getter(reg, "ammdata_get_token_pools", |p: GetTokenPoolsParams| {
        provider_at(p.blockhash).get_token_pools(p)
    });
    register_getter(reg, "ammdata_get_token_activity_page", |p: GetTokenActivityPageParams| {
        provider_at(p.blockhash).get_token_activity_page(p)
    });
    register_getter(
        reg,
        "ammdata_get_token_search_index_page",
        |p: GetTokenSearchIndexPageParams| provider_at(p.blockhash).get_token_search_index_page(p),
    );
    register_getter(reg, "ammdata_rpc_get_candles", |p: RpcGetCandlesParams| {
        provider_at(StateAt::Latest).rpc_get_candles(p)
    });
    register_getter(reg, "ammdata_get_index_height", |p: GetIndexHeightParams| {
        provider_at(p.blockhash).get_index_height(p)
    });
    register_getter(reg, "ammdata_get_list_entries_desc", |p: GetListEntriesDescParams| {
        provider_at(p.blockhash).get_list_entries_desc(p)
    });
    register_getter(reg, "ammdata_get_list_keys_by_prefix", |p: GetListKeysByPrefixParams| {
        provider_at(p.blockhash).get_list_keys_by_prefix(p)
    });
}
