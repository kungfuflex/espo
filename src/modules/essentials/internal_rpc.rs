//! Getter RPCs for the essentials module: every essentials getter the
//! explorer calls is served here as `internal.essentials_<getter>` so a
//! remote espo explorer can fetch the getter's exact native result in one
//! round-trip. For each getter this file holds the wire structs, the
//! server-side registration, and the client-side `remote_*` helper the
//! getter's remote branch calls.
//!
//! Simple results travel as plain serde JSON; heavy nested results
//! (creation records, block summaries, tx summaries, pointer blobs, holder
//! pages) travel as hex-encoded Borsh — the same encoding they are stored
//! with, so the wire contract cannot drift from the storage schema.

use crate::modules::defs::RpcNsRegistrar;
use crate::modules::essentials::storage::{
    AddressIndexListKind, AlkaneTxSummary, EssentialsProvider, GetAlkaneIdsByNamePrefixPageParams,
    GetAlkaneIdsByNamePrefixResult, GetBlockSummaryParams, GetCreationCountParams,
    GetCreationCountResult, GetCreationRecordParams, GetCreationRecordsOrderedPageParams,
    GetHoldersCountParams, GetHoldersCountResult, GetHoldersOrderedPageParams,
    GetHoldersOrderedPageResult, GetIndexHeightParams, GetIndexHeightResult,
    GetListEntriesDescParams, GetListEntriesDescResult, GetListKeysByPrefixParams,
    GetListKeysByPrefixResult, OutpointPointerBlobV3, TxPointerBlobV3, encode_creation_record,
};
use crate::modules::essentials::utils::inspections::AlkaneCreationRecord;
use crate::runtime::internal_rpc::{borsh_hex, borsh_unhex, register_getter};
use crate::runtime::remote_espo::RemoteEspoClient;
use crate::runtime::state_at::StateAt;
use crate::schemas::SchemaAlkaneId;
use anyhow::{Context, Result, anyhow};
use bitcoin::Txid;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;

fn provider() -> EssentialsProvider {
    EssentialsProvider::new(Arc::new(crate::config::espo_mdb(b"essentials:")))
}

fn txid_hex(txid: &Txid) -> String {
    txid.to_string()
}

fn txid_from_hex(raw: &str) -> Result<Txid> {
    Txid::from_str(raw).map_err(|e| anyhow!("bad txid: {e}"))
}

// ---------------------------------------------------------------------------
// Wire structs for getters whose native params/results don't serde directly.
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
pub struct WireBlockSummaryResult {
    pub summary_borsh: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct WireBlockSummariesByHeightsParams {
    pub heights: Vec<u32>,
}

#[derive(Serialize, Deserialize)]
pub struct WireBlockSummariesByHeightsResult {
    pub summaries_borsh: Vec<Option<String>>,
}

#[derive(Serialize, Deserialize)]
pub struct WireCreationRecordResult {
    /// `encode_creation_record` bytes, hex.
    pub record: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct WireCreationRecordsPageResult {
    pub records: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct WireTxSummaryParams {
    pub txid: String,
}

#[derive(Serialize, Deserialize)]
pub struct WireTxSummaryResult {
    pub summary_borsh: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct WirePointerBlobParams {
    pub id: u64,
}

#[derive(Serialize, Deserialize)]
pub struct WirePointerBlobResult {
    pub blob_borsh: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct WireAddressIndexLenParams {
    pub blockhash: StateAt,
    pub kind: AddressIndexListKind,
    pub address: String,
}

#[derive(Serialize, Deserialize)]
pub struct WireAddressIndexLenResult {
    pub len: u64,
}

#[derive(Serialize, Deserialize)]
pub struct WireAddressIndexRangeParams {
    pub blockhash: StateAt,
    pub kind: AddressIndexListKind,
    pub address: String,
    pub start: u64,
    pub end: u64,
}

#[derive(Serialize, Deserialize)]
pub struct WireAddressIndexRangeResult {
    pub ids: Vec<u64>,
}

#[derive(Serialize, Deserialize)]
pub struct WireBalancePairsParamsAddress {
    pub blockhash: StateAt,
    pub address: String,
}

#[derive(Serialize, Deserialize)]
pub struct WireBalancePairsParamsOwner {
    pub blockhash: StateAt,
    pub owner: SchemaAlkaneId,
}

#[derive(Serialize, Deserialize)]
pub struct WireBalancePairsResult {
    /// Borsh hex of `Vec<(SchemaAlkaneId, u128)>`.
    pub pairs_borsh: String,
}

#[derive(Serialize, Deserialize)]
pub struct WireAlkanePageParams {
    pub blockhash: StateAt,
    pub alkane: SchemaAlkaneId,
    pub page: u64,
    pub limit: u64,
}

#[derive(Serialize, Deserialize)]
pub struct WireHoldersPageResult {
    pub total: u64,
    /// Borsh hex of `u128`.
    pub supply_borsh: String,
    /// Borsh hex of `Vec<HolderEntry>`.
    pub holders_borsh: String,
}

#[derive(Serialize, Deserialize)]
pub struct WireAddressAmountPageResult {
    pub total: u64,
    /// Borsh hex of `Vec<AddressAmountEntry>`.
    pub entries_borsh: String,
}

#[derive(Serialize, Deserialize)]
pub struct WireOutpointLookup {
    /// Borsh hex of `Vec<BalanceEntry>`.
    pub balances_borsh: String,
    pub spent_by: Option<String>,
    pub address: Option<String>,
    pub spk: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct WireOutpointParams {
    pub blockhash: StateAt,
    pub txid: String,
    pub vout: u32,
}

#[derive(Serialize, Deserialize)]
pub struct WireOutpointLookupResult {
    pub lookup: WireOutpointLookup,
}

#[derive(Serialize, Deserialize)]
pub struct WireOutpointBatchParams {
    pub blockhash: StateAt,
    pub outpoints: Vec<(String, u32)>,
}

#[derive(Serialize, Deserialize)]
pub struct WireOutpointBatchResult {
    pub rows: Vec<((String, u32), WireOutpointLookup)>,
}

impl WireOutpointLookup {
    pub fn from_native(
        lookup: &crate::modules::essentials::utils::balances::OutpointLookup,
    ) -> Result<Self> {
        Ok(Self {
            balances_borsh: borsh_hex(&lookup.balances)?,
            spent_by: lookup.spent_by.map(|t| t.to_string()),
            address: lookup.address.clone(),
            spk: lookup.spk.as_ref().map(|s| hex::encode(s.as_bytes())),
        })
    }

    pub fn into_native(
        self,
    ) -> Result<crate::modules::essentials::utils::balances::OutpointLookup> {
        Ok(crate::modules::essentials::utils::balances::OutpointLookup {
            balances: borsh_unhex(&self.balances_borsh)?,
            spent_by: self.spent_by.as_deref().map(txid_from_hex).transpose()?,
            address: self.address,
            spk: self
                .spk
                .as_deref()
                .map(|s| hex::decode(s).map(bitcoin::ScriptBuf::from_bytes))
                .transpose()
                .map_err(|e| anyhow!("bad spk hex: {e}"))?,
        })
    }
}

// ---------------------------------------------------------------------------
// Client-side helpers: the getters' remote branches call these.
// ---------------------------------------------------------------------------

pub fn remote_get_index_height(
    remote: &RemoteEspoClient,
    params: GetIndexHeightParams,
) -> Result<GetIndexHeightResult> {
    remote.getter("internal.essentials_get_index_height", &params)
}

pub fn remote_get_list_keys_by_prefix(
    remote: &RemoteEspoClient,
    params: GetListKeysByPrefixParams,
) -> Result<GetListKeysByPrefixResult> {
    remote.getter("internal.essentials_get_list_keys_by_prefix", &params)
}

pub fn remote_get_list_entries_desc(
    remote: &RemoteEspoClient,
    params: GetListEntriesDescParams,
) -> Result<GetListEntriesDescResult> {
    remote.getter("internal.essentials_get_list_entries_desc", &params)
}

pub fn remote_get_alkane_ids_by_name_prefix_page(
    remote: &RemoteEspoClient,
    params: GetAlkaneIdsByNamePrefixPageParams,
) -> Result<GetAlkaneIdsByNamePrefixResult> {
    remote.getter("internal.essentials_get_alkane_ids_by_name_prefix_page", &params)
}

pub fn remote_get_creation_count(
    remote: &RemoteEspoClient,
    params: GetCreationCountParams,
) -> Result<GetCreationCountResult> {
    remote.getter("internal.essentials_get_creation_count", &params)
}

pub fn remote_get_latest_trace_txids(
    remote: &RemoteEspoClient,
    params: crate::modules::essentials::storage::GetLatestTraceTxidsParams,
) -> Result<crate::modules::essentials::storage::GetLatestTraceTxidsResult> {
    remote.getter("internal.essentials_get_latest_trace_txids", &params)
}

pub fn remote_get_holders_count(
    remote: &RemoteEspoClient,
    params: GetHoldersCountParams,
) -> Result<GetHoldersCountResult> {
    remote.getter("internal.essentials_get_holders_count", &params)
}

pub fn remote_get_holders_ordered_page(
    remote: &RemoteEspoClient,
    params: GetHoldersOrderedPageParams,
) -> Result<GetHoldersOrderedPageResult> {
    remote.getter("internal.essentials_get_holders_ordered_page", &params)
}

pub fn remote_get_block_summary(
    remote: &RemoteEspoClient,
    params: GetBlockSummaryParams,
) -> Result<Option<crate::modules::essentials::storage::BlockSummary>> {
    let r: WireBlockSummaryResult =
        remote.getter("internal.essentials_get_block_summary", &params)?;
    r.summary_borsh.as_deref().map(borsh_unhex).transpose()
}

pub fn remote_get_block_summaries_by_heights(
    remote: &RemoteEspoClient,
    heights: &[u32],
) -> Result<Vec<Option<crate::modules::essentials::storage::BlockSummary>>> {
    let r: WireBlockSummariesByHeightsResult = remote.getter(
        "internal.essentials_get_block_summaries_by_heights",
        &WireBlockSummariesByHeightsParams { heights: heights.to_vec() },
    )?;
    r.summaries_borsh
        .into_iter()
        .map(|opt| opt.as_deref().map(borsh_unhex).transpose())
        .collect()
}

pub fn remote_get_creation_record(
    remote: &RemoteEspoClient,
    params: GetCreationRecordParams,
) -> Result<Option<AlkaneCreationRecord>> {
    let r: WireCreationRecordResult =
        remote.getter("internal.essentials_get_creation_record", &params)?;
    r.record
        .as_deref()
        .map(|raw| {
            let bytes = hex::decode(raw).map_err(|e| anyhow!("bad record hex: {e}"))?;
            crate::modules::essentials::storage::decode_creation_record(&bytes)
        })
        .transpose()
}

pub fn remote_get_creation_records_ordered_page(
    remote: &RemoteEspoClient,
    params: GetCreationRecordsOrderedPageParams,
) -> Result<Vec<AlkaneCreationRecord>> {
    let r: WireCreationRecordsPageResult =
        remote.getter("internal.essentials_get_creation_records_ordered_page", &params)?;
    r.records
        .iter()
        .map(|raw| {
            let bytes = hex::decode(raw).map_err(|e| anyhow!("bad record hex: {e}"))?;
            crate::modules::essentials::storage::decode_creation_record(&bytes)
        })
        .collect()
}

pub fn remote_load_tx_summary_v2(
    remote: &RemoteEspoClient,
    txid: &Txid,
) -> Result<Option<AlkaneTxSummary>> {
    let r: WireTxSummaryResult = remote.getter(
        "internal.essentials_load_tx_summary_v2",
        &WireTxSummaryParams { txid: txid_hex(txid) },
    )?;
    r.summary_borsh.as_deref().map(borsh_unhex).transpose()
}

pub fn remote_load_tx_pointer_blob_v3_by_id(
    remote: &RemoteEspoClient,
    id: u64,
) -> Result<Option<TxPointerBlobV3>> {
    let r: WirePointerBlobResult = remote.getter(
        "internal.essentials_load_tx_pointer_blob_v3_by_id",
        &WirePointerBlobParams { id },
    )?;
    r.blob_borsh.as_deref().map(borsh_unhex).transpose()
}

pub fn remote_load_outpoint_pointer_blob_v3_by_id(
    remote: &RemoteEspoClient,
    id: u64,
) -> Result<Option<OutpointPointerBlobV3>> {
    let r: WirePointerBlobResult = remote.getter(
        "internal.essentials_load_outpoint_pointer_blob_v3_by_id",
        &WirePointerBlobParams { id },
    )?;
    r.blob_borsh.as_deref().map(borsh_unhex).transpose()
}

pub fn remote_get_address_index_list_len(
    remote: &RemoteEspoClient,
    blockhash: StateAt,
    kind: AddressIndexListKind,
    address: &str,
) -> Result<u64> {
    let r: WireAddressIndexLenResult = remote.getter(
        "internal.essentials_get_address_index_list_len",
        &WireAddressIndexLenParams { blockhash, kind, address: address.to_string() },
    )?;
    Ok(r.len)
}

pub fn remote_get_address_index_list_range(
    remote: &RemoteEspoClient,
    blockhash: StateAt,
    kind: AddressIndexListKind,
    address: &str,
    start: u64,
    end: u64,
) -> Result<Vec<u64>> {
    let r: WireAddressIndexRangeResult = remote.getter(
        "internal.essentials_get_address_index_list_range",
        &WireAddressIndexRangeParams { blockhash, kind, address: address.to_string(), start, end },
    )?;
    Ok(r.ids)
}

pub fn remote_get_balance_for_address(
    remote: &RemoteEspoClient,
    blockhash: StateAt,
    address: &str,
) -> Result<std::collections::HashMap<SchemaAlkaneId, u128>> {
    let r: WireBalancePairsResult = remote.getter(
        "internal.essentials_get_balance_for_address",
        &WireBalancePairsParamsAddress { blockhash, address: address.to_string() },
    )?;
    let pairs: Vec<(SchemaAlkaneId, u128)> = borsh_unhex(&r.pairs_borsh)?;
    Ok(pairs.into_iter().collect())
}

pub fn remote_get_alkane_balances(
    remote: &RemoteEspoClient,
    blockhash: StateAt,
    owner: &SchemaAlkaneId,
) -> Result<std::collections::HashMap<SchemaAlkaneId, u128>> {
    let r: WireBalancePairsResult = remote.getter(
        "internal.essentials_get_alkane_balances",
        &WireBalancePairsParamsOwner { blockhash, owner: *owner },
    )?;
    let pairs: Vec<(SchemaAlkaneId, u128)> = borsh_unhex(&r.pairs_borsh)?;
    Ok(pairs.into_iter().collect())
}

pub fn remote_get_holders_for_alkane(
    remote: &RemoteEspoClient,
    blockhash: StateAt,
    alk: SchemaAlkaneId,
    page: usize,
    limit: usize,
) -> Result<(usize, u128, Vec<crate::modules::essentials::storage::HolderEntry>)> {
    let r: WireHoldersPageResult = remote.getter(
        "internal.essentials_get_holders_for_alkane",
        &WireAlkanePageParams { blockhash, alkane: alk, page: page as u64, limit: limit as u64 },
    )?;
    Ok((r.total as usize, borsh_unhex(&r.supply_borsh)?, borsh_unhex(&r.holders_borsh)?))
}

fn remote_address_amount_page(
    remote: &RemoteEspoClient,
    method: &str,
    blockhash: StateAt,
    alk: SchemaAlkaneId,
    page: usize,
    limit: usize,
) -> Result<(usize, Vec<crate::modules::essentials::storage::AddressAmountEntry>)> {
    let r: WireAddressAmountPageResult = remote.getter(
        method,
        &WireAlkanePageParams { blockhash, alkane: alk, page: page as u64, limit: limit as u64 },
    )?;
    Ok((r.total as usize, borsh_unhex(&r.entries_borsh)?))
}

pub fn remote_get_transfer_volume_for_alkane(
    remote: &RemoteEspoClient,
    blockhash: StateAt,
    alk: SchemaAlkaneId,
    page: usize,
    limit: usize,
) -> Result<(usize, Vec<crate::modules::essentials::storage::AddressAmountEntry>)> {
    remote_address_amount_page(
        remote,
        "internal.essentials_get_transfer_volume_for_alkane",
        blockhash,
        alk,
        page,
        limit,
    )
}

pub fn remote_get_total_received_for_alkane(
    remote: &RemoteEspoClient,
    blockhash: StateAt,
    alk: SchemaAlkaneId,
    page: usize,
    limit: usize,
) -> Result<(usize, Vec<crate::modules::essentials::storage::AddressAmountEntry>)> {
    remote_address_amount_page(
        remote,
        "internal.essentials_get_total_received_for_alkane",
        blockhash,
        alk,
        page,
        limit,
    )
}

pub fn remote_get_outpoint_balances_with_spent(
    remote: &RemoteEspoClient,
    blockhash: StateAt,
    txid: &Txid,
    vout: u32,
) -> Result<crate::modules::essentials::utils::balances::OutpointLookup> {
    let r: WireOutpointLookupResult = remote.getter(
        "internal.essentials_get_outpoint_balances_with_spent",
        &WireOutpointParams { blockhash, txid: txid_hex(txid), vout },
    )?;
    r.lookup.into_native()
}

fn remote_outpoint_batch(
    remote: &RemoteEspoClient,
    method: &str,
    blockhash: StateAt,
    outpoints: &[(Txid, u32)],
) -> Result<
    std::collections::HashMap<
        (Txid, u32),
        crate::modules::essentials::utils::balances::OutpointLookup,
    >,
> {
    let r: WireOutpointBatchResult = remote.getter(
        method,
        &WireOutpointBatchParams {
            blockhash,
            outpoints: outpoints.iter().map(|(t, v)| (txid_hex(t), *v)).collect(),
        },
    )?;
    let mut out = std::collections::HashMap::with_capacity(r.rows.len());
    for ((txid_raw, vout), lookup) in r.rows {
        out.insert((txid_from_hex(&txid_raw)?, vout), lookup.into_native()?);
    }
    Ok(out)
}

pub fn remote_get_outpoint_rows_batch(
    remote: &RemoteEspoClient,
    blockhash: StateAt,
    outpoints: &[(Txid, u32)],
) -> Result<
    std::collections::HashMap<
        (Txid, u32),
        crate::modules::essentials::utils::balances::OutpointLookup,
    >,
> {
    remote_outpoint_batch(
        remote,
        "internal.essentials_get_outpoint_rows_batch",
        blockhash,
        outpoints,
    )
}

pub fn remote_get_outpoint_balances_with_spent_batch(
    remote: &RemoteEspoClient,
    blockhash: StateAt,
    outpoints: &[(Txid, u32)],
) -> Result<
    std::collections::HashMap<
        (Txid, u32),
        crate::modules::essentials::utils::balances::OutpointLookup,
    >,
> {
    remote_outpoint_batch(
        remote,
        "internal.essentials_get_outpoint_balances_with_spent_batch",
        blockhash,
        outpoints,
    )
}

// ---------------------------------------------------------------------------
// Server-side registrations.
// ---------------------------------------------------------------------------

pub fn register_internal_getters(reg: &RpcNsRegistrar) {
    register_getter(reg, "essentials_get_index_height", |p: GetIndexHeightParams| {
        provider().get_index_height(p)
    });
    register_getter(reg, "essentials_get_list_keys_by_prefix", |p: GetListKeysByPrefixParams| {
        provider().get_list_keys_by_prefix(p)
    });
    register_getter(reg, "essentials_get_list_entries_desc", |p: GetListEntriesDescParams| {
        provider().get_list_entries_desc(p)
    });
    register_getter(
        reg,
        "essentials_get_alkane_ids_by_name_prefix_page",
        |p: GetAlkaneIdsByNamePrefixPageParams| provider().get_alkane_ids_by_name_prefix_page(p),
    );
    register_getter(reg, "essentials_get_creation_count", |p: GetCreationCountParams| {
        provider().get_creation_count(p)
    });
    register_getter(reg, "essentials_get_holders_count", |p: GetHoldersCountParams| {
        provider().get_holders_count(p)
    });
    register_getter(
        reg,
        "essentials_get_latest_trace_txids",
        |p: crate::modules::essentials::storage::GetLatestTraceTxidsParams| {
            provider().get_latest_trace_txids(p)
        },
    );
    register_getter(
        reg,
        "essentials_get_holders_ordered_page",
        |p: GetHoldersOrderedPageParams| provider().get_holders_ordered_page(p),
    );
    register_getter(reg, "essentials_get_block_summary", |p: GetBlockSummaryParams| {
        let summary = provider().get_block_summary(p)?.summary;
        Ok(WireBlockSummaryResult { summary_borsh: summary.as_ref().map(borsh_hex).transpose()? })
    });
    register_getter(
        reg,
        "essentials_get_block_summaries_by_heights",
        |p: WireBlockSummariesByHeightsParams| {
            let summaries = provider().get_block_summaries_by_heights(&p.heights)?;
            Ok(WireBlockSummariesByHeightsResult {
                summaries_borsh: summaries
                    .iter()
                    .map(|s| s.as_ref().map(borsh_hex).transpose())
                    .collect::<Result<_>>()?,
            })
        },
    );
    register_getter(reg, "essentials_get_creation_record", |p: GetCreationRecordParams| {
        let record = provider().get_creation_record(p)?.record;
        Ok(WireCreationRecordResult {
            record: record
                .as_ref()
                .map(|r| encode_creation_record(r).map(hex::encode))
                .transpose()
                .context("encode creation record")?,
        })
    });
    register_getter(
        reg,
        "essentials_get_creation_records_ordered_page",
        |p: GetCreationRecordsOrderedPageParams| {
            let records = provider().get_creation_records_ordered_page(p)?.records;
            Ok(WireCreationRecordsPageResult {
                records: records
                    .iter()
                    .map(|r| encode_creation_record(r).map(hex::encode))
                    .collect::<Result<_>>()
                    .context("encode creation records")?,
            })
        },
    );
    register_getter(reg, "essentials_load_tx_summary_v2", |p: WireTxSummaryParams| {
        let txid = txid_from_hex(&p.txid)?;
        let summary = crate::modules::essentials::storage::load_tx_summary_v2(&provider(), &txid);
        Ok(WireTxSummaryResult { summary_borsh: summary.as_ref().map(borsh_hex).transpose()? })
    });
    register_getter(reg, "essentials_load_tx_pointer_blob_v3_by_id", |p: WirePointerBlobParams| {
        let blob =
            crate::modules::essentials::storage::load_tx_pointer_blob_v3_by_id(&provider(), p.id);
        Ok(WirePointerBlobResult { blob_borsh: blob.as_ref().map(borsh_hex).transpose()? })
    });
    register_getter(
        reg,
        "essentials_load_outpoint_pointer_blob_v3_by_id",
        |p: WirePointerBlobParams| {
            let blob = crate::modules::essentials::storage::load_outpoint_pointer_blob_v3_by_id(
                &provider(),
                p.id,
            );
            Ok(WirePointerBlobResult { blob_borsh: blob.as_ref().map(borsh_hex).transpose()? })
        },
    );
    register_getter(
        reg,
        "essentials_get_address_index_list_len",
        |p: WireAddressIndexLenParams| {
            let len = crate::modules::essentials::storage::get_address_index_list_len(
                &provider(),
                p.blockhash,
                p.kind,
                &p.address,
            )?;
            Ok(WireAddressIndexLenResult { len })
        },
    );
    register_getter(
        reg,
        "essentials_get_address_index_list_range",
        |p: WireAddressIndexRangeParams| {
            let ids = crate::modules::essentials::storage::get_address_index_list_range(
                &provider(),
                p.blockhash,
                p.kind,
                &p.address,
                p.start,
                p.end,
            )?;
            Ok(WireAddressIndexRangeResult { ids })
        },
    );
    register_getter(
        reg,
        "essentials_get_balance_for_address",
        |p: WireBalancePairsParamsAddress| {
            let map = crate::modules::essentials::utils::balances::get_balance_for_address(
                p.blockhash,
                &provider(),
                &p.address,
            )?;
            let mut pairs: Vec<(SchemaAlkaneId, u128)> = map.into_iter().collect();
            pairs.sort();
            Ok(WireBalancePairsResult { pairs_borsh: borsh_hex(&pairs)? })
        },
    );
    register_getter(reg, "essentials_get_alkane_balances", |p: WireBalancePairsParamsOwner| {
        let map = crate::modules::essentials::utils::balances::get_alkane_balances(
            p.blockhash,
            &provider(),
            &p.owner,
        )?;
        let mut pairs: Vec<(SchemaAlkaneId, u128)> = map.into_iter().collect();
        pairs.sort();
        Ok(WireBalancePairsResult { pairs_borsh: borsh_hex(&pairs)? })
    });
    register_getter(reg, "essentials_get_holders_for_alkane", |p: WireAlkanePageParams| {
        let (total, supply, holders) =
            crate::modules::essentials::utils::balances::get_holders_for_alkane(
                p.blockhash,
                &provider(),
                p.alkane,
                p.page as usize,
                p.limit as usize,
            )?;
        Ok(WireHoldersPageResult {
            total: total as u64,
            supply_borsh: borsh_hex(&supply)?,
            holders_borsh: borsh_hex(&holders)?,
        })
    });
    register_getter(reg, "essentials_get_transfer_volume_for_alkane", |p: WireAlkanePageParams| {
        let (total, entries) =
            crate::modules::essentials::utils::balances::get_transfer_volume_for_alkane(
                p.blockhash,
                &provider(),
                p.alkane,
                p.page as usize,
                p.limit as usize,
            )?;
        Ok(WireAddressAmountPageResult { total: total as u64, entries_borsh: borsh_hex(&entries)? })
    });
    register_getter(reg, "essentials_get_total_received_for_alkane", |p: WireAlkanePageParams| {
        let (total, entries) =
            crate::modules::essentials::utils::balances::get_total_received_for_alkane(
                p.blockhash,
                &provider(),
                p.alkane,
                p.page as usize,
                p.limit as usize,
            )?;
        Ok(WireAddressAmountPageResult { total: total as u64, entries_borsh: borsh_hex(&entries)? })
    });
    register_getter(reg, "essentials_get_outpoint_balances_with_spent", |p: WireOutpointParams| {
        let txid = txid_from_hex(&p.txid)?;
        let lookup = crate::modules::essentials::utils::balances::get_outpoint_balances_with_spent(
            p.blockhash,
            &provider(),
            &txid,
            p.vout,
        )?;
        Ok(WireOutpointLookupResult { lookup: WireOutpointLookup::from_native(&lookup)? })
    });
    register_getter(reg, "essentials_get_outpoint_rows_batch", |p: WireOutpointBatchParams| {
        let outpoints: Vec<(Txid, u32)> = p
            .outpoints
            .iter()
            .map(|(t, v)| Ok((txid_from_hex(t)?, *v)))
            .collect::<Result<_>>()?;
        let rows = crate::modules::essentials::utils::balances::get_outpoint_rows_batch(
            p.blockhash,
            &provider(),
            &outpoints,
        )?;
        Ok(WireOutpointBatchResult {
            rows: rows
                .iter()
                .map(|((t, v), lookup)| {
                    Ok(((txid_hex(t), *v), WireOutpointLookup::from_native(lookup)?))
                })
                .collect::<Result<_>>()?,
        })
    });
    register_getter(
        reg,
        "essentials_get_outpoint_balances_with_spent_batch",
        |p: WireOutpointBatchParams| {
            let outpoints: Vec<(Txid, u32)> = p
                .outpoints
                .iter()
                .map(|(t, v)| Ok((txid_from_hex(t)?, *v)))
                .collect::<Result<_>>()?;
            let rows =
                crate::modules::essentials::utils::balances::get_outpoint_balances_with_spent_batch(
                    p.blockhash,
                    &provider(),
                    &outpoints,
                )?;
            Ok(WireOutpointBatchResult {
                rows: rows
                    .iter()
                    .map(|((t, v), lookup)| {
                        Ok(((txid_hex(t), *v), WireOutpointLookup::from_native(lookup)?))
                    })
                    .collect::<Result<_>>()?,
            })
        },
    );
}
