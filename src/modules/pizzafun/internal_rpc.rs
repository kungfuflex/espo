//! Getter RPCs for the pizzafun module (see essentials/internal_rpc.rs for the
//! pattern): every pizzafun getter the explorer calls is served as
//! `internal.pizzafun_<getter>` so a remote espo explorer can fetch the getter's
//! native result in one round-trip. All params/results are serde-friendly and
//! travel as plain serde JSON.

use crate::config::get_espo_db;
use crate::modules::defs::RpcNsRegistrar;
use crate::modules::pizzafun::storage::{
    GetIndexHeightParams, GetIndexHeightResult, GetSeriesByAlkaneParams, PizzafunProvider,
    SeriesEntry,
};
use crate::runtime::internal_rpc::register_getter;
use crate::runtime::mdb::Mdb;
use crate::runtime::remote_espo::RemoteEspoClient;
use crate::runtime::state_at::StateAt;
use anyhow::Result;
use std::sync::Arc;

/// Provider pinned to the wire state so internal reads resolve against the
/// same view a locally height-pinned provider would use.
fn provider_at(state: StateAt) -> PizzafunProvider {
    PizzafunProvider::new(Arc::new(Mdb::from_db(get_espo_db(), b"pizzafun:")))
        .with_view_blockhash(state.to_option())
}

// ---------------------------------------------------------------------------
// Client-side helpers: the getters' remote branches call these.
// ---------------------------------------------------------------------------

pub fn remote_get_index_height(
    remote: &RemoteEspoClient,
    params: GetIndexHeightParams,
) -> Result<GetIndexHeightResult> {
    remote.getter("internal.pizzafun_get_index_height", &params)
}

pub fn remote_get_series_by_alkane(
    remote: &RemoteEspoClient,
    params: GetSeriesByAlkaneParams,
) -> Result<Option<SeriesEntry>> {
    remote.getter("internal.pizzafun_get_series_by_alkane", &params)
}

// ---------------------------------------------------------------------------
// Server-side registrations.
// ---------------------------------------------------------------------------

pub fn register_internal_getters(reg: &RpcNsRegistrar) {
    register_getter(reg, "pizzafun_get_index_height", |p: GetIndexHeightParams| {
        provider_at(p.blockhash).get_index_height(p)
    });
    register_getter(reg, "pizzafun_get_series_by_alkane", |p: GetSeriesByAlkaneParams| {
        provider_at(p.blockhash).get_series_by_alkane(p)
    });
}
