//! Getter RPCs for the runes module (see essentials/internal_rpc.rs for the
//! pattern): every runes getter the explorer calls is served as
//! `internal.runes_<getter>` so a remote espo explorer can fetch the getter's
//! native result in one round-trip. For each getter this file holds the wire
//! structs, the server-side registration, and the client-side `remote_*`
//! helper the getter's remote branch calls.
//!
//! Simple results travel as plain serde JSON; heavy nested results (rune
//! entries, activity pages, tx pointer blobs, tx io) travel as hex-encoded
//! Borsh — the same encoding they are stored with, so the wire contract
//! cannot drift from the storage schema.

use crate::config::get_espo_db;
use crate::modules::defs::RpcNsRegistrar;
use crate::modules::runes::inscriptions::RuneIcon;
use crate::modules::runes::storage::{
    ActionTxPointerBlob, GetRuneActivityPageParams, OutpointRuneBalances, RuneActivityPage,
    RuneAddressAmountEntry, RuneBalanceHistoryPoint, RuneEntry, RuneTxPointerBlob, RuneVolumeKind,
    RunesProvider, SchemaRuneId, TxRuneIo,
};
use crate::runtime::internal_rpc::{borsh_hex, borsh_unhex, register_getter};
use crate::runtime::mdb::Mdb;
use crate::runtime::remote_espo::RemoteEspoClient;
use anyhow::{Result, anyhow};
use bitcoin::Txid;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;

fn provider() -> RunesProvider {
    RunesProvider::new(Arc::new(Mdb::from_db(get_espo_db(), b"runes:")))
}

fn txid_hex(txid: &Txid) -> String {
    txid.to_string()
}

fn txid_from_hex(raw: &str) -> Result<Txid> {
    Txid::from_str(raw).map_err(|e| anyhow!("bad txid: {e}"))
}

fn arr32_hex(value: &[u8; 32]) -> String {
    hex::encode(value)
}

fn arr32_from_hex(raw: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(raw).map_err(|e| anyhow!("bad hex: {e}"))?;
    bytes.as_slice().try_into().map_err(|_| anyhow!("expected 32 bytes"))
}

// ---------------------------------------------------------------------------
// Wire structs.
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
pub struct WireNoParams {}

#[derive(Serialize, Deserialize)]
pub struct WireIndexHeightResult {
    pub height: Option<u32>,
}

#[derive(Serialize, Deserialize)]
pub struct WireRuneIdParams {
    pub id: SchemaRuneId,
}

#[derive(Serialize, Deserialize)]
pub struct WireRuneEntryResult {
    /// Borsh hex of `RuneEntry`.
    pub entry_borsh: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct WireQueryParams {
    pub query: String,
}

#[derive(Serialize, Deserialize)]
pub struct WireCountResult {
    pub count: u64,
}

#[derive(Serialize, Deserialize)]
pub struct WirePageDescParams {
    pub page: u64,
    pub limit: u64,
    pub desc: bool,
}

#[derive(Serialize, Deserialize)]
pub struct WireRuneEntriesWithHoldersResult {
    /// Borsh hex of `Vec<(RuneEntry, u64)>`.
    pub rows_borsh: String,
}

#[derive(Serialize, Deserialize)]
pub struct WireNamePrefixParams {
    pub query: String,
    pub limit: u64,
}

#[derive(Serialize, Deserialize)]
pub struct WireRuneEntriesResult {
    /// Borsh hex of `Vec<RuneEntry>`.
    pub entries_borsh: String,
}

#[derive(Serialize, Deserialize)]
pub struct WireHoldersPageParams {
    pub id: SchemaRuneId,
    pub page: u64,
    pub limit: u64,
}

#[derive(Serialize, Deserialize)]
pub struct WireHoldersResult {
    pub rows: Vec<(String, u128)>,
}

#[derive(Serialize, Deserialize)]
pub struct WireRuneIconResult {
    /// Borsh hex of `RuneIcon`.
    pub icon_borsh: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct WireRuneActivityPageResult {
    pub total: u64,
    /// Borsh hex of `Vec<RuneActivity>`.
    pub entries_borsh: String,
}

#[derive(Serialize, Deserialize)]
pub struct WireAddressParams {
    pub address: String,
}

#[derive(Serialize, Deserialize)]
pub struct WireAddressBalancesResult {
    pub balances: Vec<(SchemaRuneId, u128)>,
}

#[derive(Serialize, Deserialize)]
pub struct WireBalanceHistoryParams {
    pub address: String,
    pub id: SchemaRuneId,
    pub range_min: u32,
    pub range_max: u32,
    pub interval: u32,
}

#[derive(Serialize, Deserialize)]
pub struct WireBalanceHistoryResult {
    pub points: Vec<RuneBalanceHistoryPoint>,
}

#[derive(Serialize, Deserialize)]
pub struct WireAddressOutpointsResult {
    /// `(txid hex, vout, Borsh hex of OutpointRuneBalances)` rows.
    pub rows: Vec<(String, u32, String)>,
}

#[derive(Serialize, Deserialize)]
pub struct WireAddressRangeParams {
    pub address: String,
    pub start: u64,
    pub end: u64,
}

#[derive(Serialize, Deserialize)]
pub struct WireHeightParams {
    pub height: u64,
}

#[derive(Serialize, Deserialize)]
pub struct WireHeightRangeParams {
    pub height: u64,
    pub start: u64,
    pub end: u64,
}

#[derive(Serialize, Deserialize)]
pub struct WirePointerBlobsResult {
    /// Borsh hex of `Vec<RuneTxPointerBlob>` or `Vec<ActionTxPointerBlob>`.
    pub rows_borsh: String,
}

#[derive(Serialize, Deserialize)]
pub struct WireTxIoParams {
    pub txid: String,
}

#[derive(Serialize, Deserialize)]
pub struct WireTxIoResult {
    /// Borsh hex of `TxRuneIo`.
    pub io_borsh: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct WireVolumeParams {
    pub id: SchemaRuneId,
    pub kind: RuneVolumeKind,
    pub page: u64,
    pub limit: u64,
}

#[derive(Serialize, Deserialize)]
pub struct WireVolumeResult {
    pub total: u64,
    pub entries: Vec<RuneAddressAmountEntry>,
}

#[derive(Serialize, Deserialize)]
pub struct WireUgPriceByHeightParams {
    pub height: u32,
}

#[derive(Serialize, Deserialize)]
pub struct WireUgPriceByHeightResult {
    /// 32 bytes, hex.
    pub price: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct WireUgPricePointsParams {
    pub max_height: u32,
}

#[derive(Serialize, Deserialize)]
pub struct WireUgPricePointsResult {
    /// `(height, 32-byte price hex)` rows, ascending by height.
    pub points: Vec<(u32, String)>,
}

// ---------------------------------------------------------------------------
// Client-side helpers: the getters' remote branches call these.
// ---------------------------------------------------------------------------

pub fn remote_get_index_height(remote: &RemoteEspoClient) -> Result<Option<u32>> {
    let r: WireIndexHeightResult =
        remote.getter("internal.runes_get_index_height", &WireNoParams {})?;
    Ok(r.height)
}

pub fn remote_get_rune(remote: &RemoteEspoClient, id: SchemaRuneId) -> Result<Option<RuneEntry>> {
    let r: WireRuneEntryResult =
        remote.getter("internal.runes_get_rune", &WireRuneIdParams { id })?;
    r.entry_borsh.as_deref().map(borsh_unhex).transpose()
}

pub fn remote_get_rune_by_query(
    remote: &RemoteEspoClient,
    query: &str,
) -> Result<Option<RuneEntry>> {
    let r: WireRuneEntryResult = remote.getter(
        "internal.runes_get_rune_by_query",
        &WireQueryParams { query: query.to_string() },
    )?;
    r.entry_borsh.as_deref().map(borsh_unhex).transpose()
}

pub fn remote_get_rune_count(remote: &RemoteEspoClient) -> Result<u64> {
    let r: WireCountResult = remote.getter("internal.runes_get_rune_count", &WireNoParams {})?;
    Ok(r.count)
}

fn remote_rune_entries_with_holders(
    remote: &RemoteEspoClient,
    method: &str,
    page: usize,
    limit: usize,
    desc: bool,
) -> Result<Vec<(RuneEntry, u64)>> {
    let r: WireRuneEntriesWithHoldersResult = remote
        .getter(method, &WirePageDescParams { page: page as u64, limit: limit as u64, desc })?;
    borsh_unhex(&r.rows_borsh)
}

pub fn remote_get_runes_by_age(
    remote: &RemoteEspoClient,
    page: usize,
    limit: usize,
    desc: bool,
) -> Result<Vec<(RuneEntry, u64)>> {
    remote_rune_entries_with_holders(remote, "internal.runes_get_runes_by_age", page, limit, desc)
}

pub fn remote_get_runes_by_holders(
    remote: &RemoteEspoClient,
    page: usize,
    limit: usize,
    desc: bool,
) -> Result<Vec<(RuneEntry, u64)>> {
    remote_rune_entries_with_holders(
        remote,
        "internal.runes_get_runes_by_holders",
        page,
        limit,
        desc,
    )
}

pub fn remote_get_runes_by_name_prefix(
    remote: &RemoteEspoClient,
    query: &str,
    limit: usize,
) -> Result<Vec<RuneEntry>> {
    let r: WireRuneEntriesResult = remote.getter(
        "internal.runes_get_runes_by_name_prefix",
        &WireNamePrefixParams { query: query.to_string(), limit: limit as u64 },
    )?;
    borsh_unhex(&r.entries_borsh)
}

pub fn remote_get_holders(
    remote: &RemoteEspoClient,
    id: SchemaRuneId,
    page: usize,
    limit: usize,
) -> Result<Vec<(String, u128)>> {
    let r: WireHoldersResult = remote.getter(
        "internal.runes_get_holders",
        &WireHoldersPageParams { id, page: page as u64, limit: limit as u64 },
    )?;
    Ok(r.rows)
}

pub fn remote_get_holders_count(remote: &RemoteEspoClient, id: SchemaRuneId) -> Result<u64> {
    let r: WireCountResult =
        remote.getter("internal.runes_get_holders_count", &WireRuneIdParams { id })?;
    Ok(r.count)
}

pub fn remote_get_rune_icon(
    remote: &RemoteEspoClient,
    id: SchemaRuneId,
) -> Result<Option<RuneIcon>> {
    let r: WireRuneIconResult =
        remote.getter("internal.runes_get_rune_icon", &WireRuneIdParams { id })?;
    r.icon_borsh.as_deref().map(borsh_unhex).transpose()
}

pub fn remote_get_rune_activity_page(
    remote: &RemoteEspoClient,
    params: GetRuneActivityPageParams,
) -> Result<RuneActivityPage> {
    let r: WireRuneActivityPageResult =
        remote.getter("internal.runes_get_rune_activity_page", &params)?;
    Ok(RuneActivityPage { total: r.total as usize, entries: borsh_unhex(&r.entries_borsh)? })
}

pub fn remote_get_address_balances(
    remote: &RemoteEspoClient,
    address: &str,
) -> Result<Vec<(SchemaRuneId, u128)>> {
    let r: WireAddressBalancesResult = remote.getter(
        "internal.runes_get_address_balances",
        &WireAddressParams { address: address.to_string() },
    )?;
    Ok(r.balances)
}

pub fn remote_get_address_balance_history_points(
    remote: &RemoteEspoClient,
    address: &str,
    id: SchemaRuneId,
    range_min: u32,
    range_max: u32,
    interval: u32,
) -> Result<Vec<RuneBalanceHistoryPoint>> {
    let r: WireBalanceHistoryResult = remote.getter(
        "internal.runes_get_address_balance_history_points",
        &WireBalanceHistoryParams {
            address: address.to_string(),
            id,
            range_min,
            range_max,
            interval,
        },
    )?;
    Ok(r.points)
}

pub fn remote_get_address_outpoints(
    remote: &RemoteEspoClient,
    address: &str,
) -> Result<Vec<(Txid, u32, OutpointRuneBalances)>> {
    let r: WireAddressOutpointsResult = remote.getter(
        "internal.runes_get_address_outpoints",
        &WireAddressParams { address: address.to_string() },
    )?;
    r.rows
        .into_iter()
        .map(|(txid_raw, vout, balances_raw)| {
            Ok((txid_from_hex(&txid_raw)?, vout, borsh_unhex(&balances_raw)?))
        })
        .collect()
}

fn remote_tx_count(remote: &RemoteEspoClient, method: &str, address: &str) -> Result<u64> {
    let r: WireCountResult =
        remote.getter(method, &WireAddressParams { address: address.to_string() })?;
    Ok(r.count)
}

pub fn remote_get_address_tx_count(remote: &RemoteEspoClient, address: &str) -> Result<u64> {
    remote_tx_count(remote, "internal.runes_get_address_tx_count", address)
}

pub fn remote_get_address_tx_range(
    remote: &RemoteEspoClient,
    address: &str,
    start: u64,
    end: u64,
) -> Result<Vec<RuneTxPointerBlob>> {
    let r: WirePointerBlobsResult = remote.getter(
        "internal.runes_get_address_tx_range",
        &WireAddressRangeParams { address: address.to_string(), start, end },
    )?;
    borsh_unhex(&r.rows_borsh)
}

pub fn remote_get_action_address_tx_count(remote: &RemoteEspoClient, address: &str) -> Result<u64> {
    remote_tx_count(remote, "internal.runes_get_action_address_tx_count", address)
}

pub fn remote_get_action_address_tx_range(
    remote: &RemoteEspoClient,
    address: &str,
    start: u64,
    end: u64,
) -> Result<Vec<ActionTxPointerBlob>> {
    let r: WirePointerBlobsResult = remote.getter(
        "internal.runes_get_action_address_tx_range",
        &WireAddressRangeParams { address: address.to_string(), start, end },
    )?;
    borsh_unhex(&r.rows_borsh)
}

pub fn remote_get_block_tx_count(remote: &RemoteEspoClient, height: u64) -> Result<u64> {
    let r: WireCountResult =
        remote.getter("internal.runes_get_block_tx_count", &WireHeightParams { height })?;
    Ok(r.count)
}

pub fn remote_get_block_tx_range(
    remote: &RemoteEspoClient,
    height: u64,
    start: u64,
    end: u64,
) -> Result<Vec<RuneTxPointerBlob>> {
    let r: WirePointerBlobsResult = remote.getter(
        "internal.runes_get_block_tx_range",
        &WireHeightRangeParams { height, start, end },
    )?;
    borsh_unhex(&r.rows_borsh)
}

pub fn remote_get_action_block_tx_count(remote: &RemoteEspoClient, height: u64) -> Result<u64> {
    let r: WireCountResult =
        remote.getter("internal.runes_get_action_block_tx_count", &WireHeightParams { height })?;
    Ok(r.count)
}

pub fn remote_get_action_block_tx_range(
    remote: &RemoteEspoClient,
    height: u64,
    start: u64,
    end: u64,
) -> Result<Vec<ActionTxPointerBlob>> {
    let r: WirePointerBlobsResult = remote.getter(
        "internal.runes_get_action_block_tx_range",
        &WireHeightRangeParams { height, start, end },
    )?;
    borsh_unhex(&r.rows_borsh)
}

pub fn remote_get_tx_io(remote: &RemoteEspoClient, txid: &Txid) -> Result<Option<TxRuneIo>> {
    let r: WireTxIoResult =
        remote.getter("internal.runes_get_tx_io", &WireTxIoParams { txid: txid_hex(txid) })?;
    r.io_borsh.as_deref().map(borsh_unhex).transpose()
}

pub fn remote_get_volume(
    remote: &RemoteEspoClient,
    id: SchemaRuneId,
    kind: RuneVolumeKind,
    page: usize,
    limit: usize,
) -> Result<(usize, Vec<RuneAddressAmountEntry>)> {
    let r: WireVolumeResult = remote.getter(
        "internal.runes_get_volume",
        &WireVolumeParams { id, kind, page: page as u64, limit: limit as u64 },
    )?;
    Ok((r.total as usize, r.entries))
}

pub fn remote_get_uncommon_goods_avg_price_paid_usd_by_height(
    remote: &RemoteEspoClient,
    height: u32,
) -> Result<Option<[u8; 32]>> {
    let r: WireUgPriceByHeightResult = remote.getter(
        "internal.runes_get_uncommon_goods_avg_price_paid_usd_by_height",
        &WireUgPriceByHeightParams { height },
    )?;
    r.price.as_deref().map(arr32_from_hex).transpose()
}

pub fn remote_get_uncommon_goods_avg_price_paid_usd_points_through_height(
    remote: &RemoteEspoClient,
    max_height: u32,
) -> Result<Vec<(u32, [u8; 32])>> {
    let r: WireUgPricePointsResult = remote.getter(
        "internal.runes_get_uncommon_goods_avg_price_paid_usd_points_through_height",
        &WireUgPricePointsParams { max_height },
    )?;
    r.points
        .into_iter()
        .map(|(height, price_raw)| Ok((height, arr32_from_hex(&price_raw)?)))
        .collect()
}

// ---------------------------------------------------------------------------
// Server-side registrations.
// ---------------------------------------------------------------------------

pub fn register_internal_getters(reg: &RpcNsRegistrar) {
    register_getter(reg, "runes_get_index_height", |_p: WireNoParams| {
        Ok(WireIndexHeightResult { height: provider().get_index_height()? })
    });
    register_getter(reg, "runes_get_rune", |p: WireRuneIdParams| {
        let entry = provider().get_rune(p.id)?;
        Ok(WireRuneEntryResult { entry_borsh: entry.as_ref().map(borsh_hex).transpose()? })
    });
    register_getter(reg, "runes_get_rune_by_query", |p: WireQueryParams| {
        let entry = provider().get_rune_by_query(&p.query)?;
        Ok(WireRuneEntryResult { entry_borsh: entry.as_ref().map(borsh_hex).transpose()? })
    });
    register_getter(reg, "runes_get_rune_count", |_p: WireNoParams| {
        Ok(WireCountResult { count: provider().get_rune_count()? })
    });
    register_getter(reg, "runes_get_runes_by_age", |p: WirePageDescParams| {
        let rows = provider().get_runes_by_age(p.page as usize, p.limit as usize, p.desc)?;
        Ok(WireRuneEntriesWithHoldersResult { rows_borsh: borsh_hex(&rows)? })
    });
    register_getter(reg, "runes_get_runes_by_holders", |p: WirePageDescParams| {
        let rows = provider().get_runes_by_holders(p.page as usize, p.limit as usize, p.desc)?;
        Ok(WireRuneEntriesWithHoldersResult { rows_borsh: borsh_hex(&rows)? })
    });
    register_getter(reg, "runes_get_runes_by_name_prefix", |p: WireNamePrefixParams| {
        let entries = provider().get_runes_by_name_prefix(&p.query, p.limit as usize)?;
        Ok(WireRuneEntriesResult { entries_borsh: borsh_hex(&entries)? })
    });
    register_getter(reg, "runes_get_holders", |p: WireHoldersPageParams| {
        let rows = provider().get_holders(p.id, p.page as usize, p.limit as usize)?;
        Ok(WireHoldersResult { rows })
    });
    register_getter(reg, "runes_get_holders_count", |p: WireRuneIdParams| {
        Ok(WireCountResult { count: provider().get_holders_count(p.id)? })
    });
    register_getter(reg, "runes_get_rune_icon", |p: WireRuneIdParams| {
        let icon = provider().get_rune_icon(p.id)?;
        Ok(WireRuneIconResult { icon_borsh: icon.as_ref().map(borsh_hex).transpose()? })
    });
    register_getter(reg, "runes_get_rune_activity_page", |p: GetRuneActivityPageParams| {
        let page = provider().get_rune_activity_page(p)?;
        Ok(WireRuneActivityPageResult {
            total: page.total as u64,
            entries_borsh: borsh_hex(&page.entries)?,
        })
    });
    register_getter(reg, "runes_get_address_balances", |p: WireAddressParams| {
        Ok(WireAddressBalancesResult { balances: provider().get_address_balances(&p.address)? })
    });
    register_getter(
        reg,
        "runes_get_address_balance_history_points",
        |p: WireBalanceHistoryParams| {
            let points = provider().get_address_balance_history_points(
                &p.address,
                p.id,
                p.range_min,
                p.range_max,
                p.interval,
            )?;
            Ok(WireBalanceHistoryResult { points })
        },
    );
    register_getter(reg, "runes_get_address_outpoints", |p: WireAddressParams| {
        let rows = provider().get_address_outpoints(&p.address)?;
        Ok(WireAddressOutpointsResult {
            rows: rows
                .iter()
                .map(|(txid, vout, balances)| Ok((txid_hex(txid), *vout, borsh_hex(balances)?)))
                .collect::<Result<_>>()?,
        })
    });
    register_getter(reg, "runes_get_address_tx_count", |p: WireAddressParams| {
        Ok(WireCountResult { count: provider().get_address_tx_count(&p.address)? })
    });
    register_getter(reg, "runes_get_address_tx_range", |p: WireAddressRangeParams| {
        let rows = provider().get_address_tx_range(&p.address, p.start, p.end)?;
        Ok(WirePointerBlobsResult { rows_borsh: borsh_hex(&rows)? })
    });
    register_getter(reg, "runes_get_action_address_tx_count", |p: WireAddressParams| {
        Ok(WireCountResult { count: provider().get_action_address_tx_count(&p.address)? })
    });
    register_getter(reg, "runes_get_action_address_tx_range", |p: WireAddressRangeParams| {
        let rows = provider().get_action_address_tx_range(&p.address, p.start, p.end)?;
        Ok(WirePointerBlobsResult { rows_borsh: borsh_hex(&rows)? })
    });
    register_getter(reg, "runes_get_block_tx_count", |p: WireHeightParams| {
        Ok(WireCountResult { count: provider().get_block_tx_count(p.height)? })
    });
    register_getter(reg, "runes_get_block_tx_range", |p: WireHeightRangeParams| {
        let rows = provider().get_block_tx_range(p.height, p.start, p.end)?;
        Ok(WirePointerBlobsResult { rows_borsh: borsh_hex(&rows)? })
    });
    register_getter(reg, "runes_get_action_block_tx_count", |p: WireHeightParams| {
        Ok(WireCountResult { count: provider().get_action_block_tx_count(p.height)? })
    });
    register_getter(reg, "runes_get_action_block_tx_range", |p: WireHeightRangeParams| {
        let rows = provider().get_action_block_tx_range(p.height, p.start, p.end)?;
        Ok(WirePointerBlobsResult { rows_borsh: borsh_hex(&rows)? })
    });
    register_getter(reg, "runes_get_tx_io", |p: WireTxIoParams| {
        let txid = txid_from_hex(&p.txid)?;
        let io = provider().get_tx_io(&txid)?;
        Ok(WireTxIoResult { io_borsh: io.as_ref().map(borsh_hex).transpose()? })
    });
    register_getter(reg, "runes_get_volume", |p: WireVolumeParams| {
        let (total, entries) =
            provider().get_volume(p.id, p.kind, p.page as usize, p.limit as usize)?;
        Ok(WireVolumeResult { total: total as u64, entries })
    });
    register_getter(
        reg,
        "runes_get_uncommon_goods_avg_price_paid_usd_by_height",
        |p: WireUgPriceByHeightParams| {
            let price = provider().get_uncommon_goods_avg_price_paid_usd_by_height(p.height)?;
            Ok(WireUgPriceByHeightResult { price: price.as_ref().map(arr32_hex) })
        },
    );
    register_getter(
        reg,
        "runes_get_uncommon_goods_avg_price_paid_usd_points_through_height",
        |p: WireUgPricePointsParams| {
            let points = provider()
                .get_uncommon_goods_avg_price_paid_usd_points_through_height(p.max_height)?;
            Ok(WireUgPricePointsResult {
                points: points.iter().map(|(height, price)| (*height, arr32_hex(price))).collect(),
            })
        },
    );
}
