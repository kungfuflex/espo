//! Getter RPCs for the subfrost module (see essentials/internal_rpc.rs for the
//! pattern): every subfrost getter the explorer calls is served as
//! `internal.subfrost_<getter>` so a remote espo explorer can fetch the getter's
//! native result in one round-trip. All params/results are serde-friendly and
//! travel as plain serde JSON.

use crate::config::get_espo_db;
use crate::modules::defs::RpcNsRegistrar;
use crate::modules::subfrost::storage::{
    GetIndexHeightParams, GetIndexHeightResult, SubfrostProvider,
};
use crate::runtime::internal_rpc::register_getter;
use crate::runtime::mdb::Mdb;
use crate::runtime::remote_espo::RemoteEspoClient;
use crate::runtime::state_at::StateAt;
use anyhow::Result;
use std::sync::Arc;

/// Provider pinned to the wire state so internal reads resolve against the
/// same view a locally height-pinned provider would use.
fn provider_at(state: StateAt) -> SubfrostProvider {
    SubfrostProvider::new(Arc::new(Mdb::from_db(get_espo_db(), b"subfrost:")))
        .with_view_blockhash(state.to_option())
}

// ---------------------------------------------------------------------------
// Client-side helpers: the getters' remote branches call these.
// ---------------------------------------------------------------------------

pub fn remote_get_index_height(
    remote: &RemoteEspoClient,
    params: GetIndexHeightParams,
) -> Result<GetIndexHeightResult> {
    remote.getter("internal.subfrost_get_index_height", &params)
}

// ---------------------------------------------------------------------------
// Server-side registrations.
// ---------------------------------------------------------------------------

pub fn register_internal_getters(reg: &RpcNsRegistrar) {
    register_getter(reg, "subfrost_get_index_height", |p: GetIndexHeightParams| {
        provider_at(p.blockhash).get_index_height(p)
    });
}
