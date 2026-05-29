use crate::config::{
    get_block_source, // NEW: use BlockSource for full blocks
    get_metashrew,
    get_metashrew_sdb,
    get_network,
    recover_missing_traces_by_txid,
};
use crate::consts::alkanes_genesis_block;
use crate::core::blockfetcher::BlockSource;
use crate::schemas::EspoOutpoint;
use crate::schemas::SchemaAlkaneId;
use crate::utils::fee_rates::BlockFeeRateSummary;
use alkanes_cli_common::alkanes_pb::AlkanesTrace;
use alkanes_support::cellpack::Cellpack;
use alkanes_support::id::AlkaneId;
use alkanes_support::proto::alkanes;
use anyhow::{Context, Result};
use bitcoin::block::Header;
use bitcoin::consensus::Encodable;
use bitcoin::hashes::Hash;
use bitcoin::{Transaction, Txid};
// use bitcoincore_rpc::RpcApi; // REMOVED: block fetch now via BlockSource
use borsh::{BorshDeserialize, BorshSerialize};
use ordinals::{Artifact, Runestone};
use protorune_support::protostone::Protostone;
use protorune_support::utils::decode_varint_list;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::io::Cursor;

#[derive(Debug, Clone, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct EspoSandshrewLikeTrace {
    pub outpoint: String,
    pub events: Vec<EspoSandshrewLikeTraceEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
#[serde(tag = "event", content = "data")]
pub enum EspoSandshrewLikeTraceEvent {
    #[serde(rename = "invoke")]
    Invoke(EspoSandshrewLikeTraceInvokeData),

    #[serde(rename = "return")]
    Return(EspoSandshrewLikeTraceReturnData),

    #[serde(rename = "create")]
    Create(EspoSandshrewLikeTraceShortId),
}

#[derive(Debug, Clone, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct EspoSandshrewLikeTraceInvokeData {
    #[serde(rename = "type")]
    pub typ: String,
    pub context: EspoSandshrewLikeTraceInvokeContext,
    pub fuel: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct EspoSandshrewLikeTraceInvokeContext {
    pub myself: EspoSandshrewLikeTraceShortId,
    pub caller: EspoSandshrewLikeTraceShortId,
    pub inputs: Vec<String>,
    #[serde(rename = "incomingAlkanes")]
    pub incoming_alkanes: Vec<EspoSandshrewLikeTraceTransfer>,
    pub vout: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct EspoSandshrewLikeTraceShortId {
    pub block: String,
    pub tx: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct EspoSandshrewLikeTraceTransfer {
    pub id: EspoSandshrewLikeTraceShortId,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct EspoSandshrewLikeTraceReturnData {
    pub status: EspoSandshrewLikeTraceStatus,
    pub response: EspoSandshrewLikeTraceReturnResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[serde(rename_all = "lowercase")]
pub enum EspoSandshrewLikeTraceStatus {
    Success,
    Failure,
}

#[derive(Debug, Clone, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct EspoSandshrewLikeTraceReturnResponse {
    pub alkanes: Vec<EspoSandshrewLikeTraceTransfer>,
    pub data: String,
    pub storage: Vec<EspoSandshrewLikeTraceStorageKV>,
}

#[derive(Debug, Clone, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct EspoSandshrewLikeTraceStorageKV {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug)]
pub struct PartialEspoTrace {
    pub protobuf_trace: AlkanesTrace,
    pub outpoint: Vec<u8>, // [32 txid_le | 4 vout_le]
}

#[derive(Clone, Debug)]
pub struct EspoTrace {
    pub sandshrew_trace: EspoSandshrewLikeTrace,
    pub protobuf_trace: AlkanesTrace,
    pub storage_changes: AlkaneStorageChanges,
    pub outpoint: EspoOutpoint,
}

#[derive(Clone, Debug)]
pub struct EspoAlkanesTransaction {
    pub traces: Option<Vec<EspoTrace>>,
    pub transaction: Transaction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspoHostFunctionType {
    BlockHeader = 0,
    CoinbaseTxResponse = 1,
    DieselMints = 2,
    TotalMinerFee = 3,
}

pub type EspoHostFunctionValues = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);

#[derive(Clone)]
pub struct EspoBlock {
    pub is_latest: bool,
    pub height: u32,
    pub block_header: Header,
    pub host_function_values: EspoHostFunctionValues,
    pub fee_summary: Option<BlockFeeRateSummary>,
    pub tx_count: usize,
    pub transactions: Vec<EspoAlkanesTransaction>,
}

#[derive(Clone, Debug)]
pub struct GetEspoBlockOpts {
    pub page: usize,
    pub limit: usize,
}

impl GetEspoBlockOpts {
    fn page_range(&self, total: usize) -> (usize, usize) {
        let limit = self.limit.max(1);
        let page = self.page.max(1);
        let off = limit.saturating_mul(page.saturating_sub(1));
        let end = (off + limit).min(total);
        (off, end)
    }
}

/// Map of AlkaneId -> (key -> value), last-write-wins per key within a single trace.
pub type AlkaneStorageChanges = HashMap<SchemaAlkaneId, HashMap<Vec<u8>, (Txid, Vec<u8>)>>;

/// Extract last-write-wins storage mutations per Alkane from a single protobuf trace.
pub fn extract_alkane_storage(
    trace: &alkanes::AlkanesTrace,
    transaction: &Transaction,
) -> anyhow::Result<AlkaneStorageChanges> {
    let mut out: AlkaneStorageChanges = HashMap::new();
    let mut stack: Vec<SchemaAlkaneId> = Vec::with_capacity(16);
    let txid: Txid = transaction.compute_txid();

    use alkanes::alkanes_trace_event::Event;
    for ev in &trace.events {
        if let Some(evt) = &ev.event {
            match evt {
                Event::EnterContext(enter) => {
                    if let Some(ctx) = enter.context.as_ref() {
                        if let Some(inner) = ctx.inner.as_ref() {
                            if let Some(myself) = inner.myself.as_ref() {
                                let owner: SchemaAlkaneId = myself.clone().try_into()?;
                                stack.push(owner);
                            }
                        }
                    }
                }
                Event::ExitContext(exit) => {
                    let Some(owner) = stack.pop() else { continue };
                    if let Some(resp) = exit.response.as_ref() {
                        let entry = out.entry(owner).or_insert_with(HashMap::new);
                        for kv in &resp.storage {
                            let k = kv.key.clone();
                            let v = kv.value.clone();
                            entry.insert(k, (txid, v));
                        }
                    }
                }
                Event::CreateAlkane(_create) => {}
                Event::ReceiveIntent(_) => {}
                Event::ValueTransfer(_) => {}
            }
        }
    }

    Ok(out)
}

fn trace_id_to_short_id(id: Option<&alkanes::AlkaneId>) -> EspoSandshrewLikeTraceShortId {
    EspoSandshrewLikeTraceShortId {
        block: id
            .and_then(|x| x.block.as_ref())
            .map(fmt_u128_hex)
            .unwrap_or_else(|| "0x0".to_string()),
        tx: id
            .and_then(|x| x.tx.as_ref())
            .map(fmt_u128_hex)
            .unwrap_or_else(|| "0x0".to_string()),
    }
}

fn trace_transfer_to_espo(transfer: &alkanes::AlkaneTransfer) -> EspoSandshrewLikeTraceTransfer {
    EspoSandshrewLikeTraceTransfer {
        id: trace_id_to_short_id(transfer.id.as_ref()),
        value: transfer.value.as_ref().map(fmt_u128_hex).unwrap_or_else(|| "0x0".to_string()),
    }
}

pub fn protobuf_trace_events(trace: &AlkanesTrace) -> Result<Vec<EspoSandshrewLikeTraceEvent>> {
    let mut out: Vec<EspoSandshrewLikeTraceEvent> = Vec::with_capacity(trace.events.len());

    for ev in &trace.events {
        if let Some(event) = &ev.event {
            use alkanes::alkanes_trace_event::Event;
            match event {
                Event::EnterContext(enter) => {
                    let typ = match enter.call_type() {
                        alkanes::AlkanesTraceCallType::Call => "call",
                        alkanes::AlkanesTraceCallType::Delegatecall => "delegatecall",
                        alkanes::AlkanesTraceCallType::Staticcall => "staticcall",
                        _ => "unknown",
                    };

                    let ctx = enter.context.as_ref().context("enter.context missing")?;
                    let inner = ctx.inner.as_ref().context("enter.context.inner missing")?;

                    out.push(EspoSandshrewLikeTraceEvent::Invoke(
                        EspoSandshrewLikeTraceInvokeData {
                            typ: typ.to_string(),
                            context: EspoSandshrewLikeTraceInvokeContext {
                                myself: trace_id_to_short_id(inner.myself.as_ref()),
                                caller: trace_id_to_short_id(inner.caller.as_ref()),
                                inputs: inner.inputs.iter().map(fmt_u128_hex).collect(),
                                incoming_alkanes: inner
                                    .incoming_alkanes
                                    .iter()
                                    .map(trace_transfer_to_espo)
                                    .collect(),
                                vout: inner.vout,
                            },
                            fuel: ctx.fuel,
                        },
                    ));
                }

                Event::ExitContext(exit) => {
                    let status = match exit.status() {
                        alkanes::AlkanesTraceStatusFlag::Failure => {
                            EspoSandshrewLikeTraceStatus::Failure
                        }
                        _ => EspoSandshrewLikeTraceStatus::Success,
                    };

                    let response = if let Some(resp) = exit.response.as_ref() {
                        EspoSandshrewLikeTraceReturnResponse {
                            alkanes: resp.alkanes.iter().map(trace_transfer_to_espo).collect(),
                            data: fmt_bytes_hex(&resp.data),
                            storage: resp
                                .storage
                                .iter()
                                .map(|kv| EspoSandshrewLikeTraceStorageKV {
                                    key: bytes_to_string_or_hex(&kv.key),
                                    value: fmt_bytes_hex(&kv.value),
                                })
                                .collect(),
                        }
                    } else {
                        EspoSandshrewLikeTraceReturnResponse {
                            alkanes: Vec::new(),
                            data: "0x".to_string(),
                            storage: Vec::new(),
                        }
                    };

                    out.push(EspoSandshrewLikeTraceEvent::Return(
                        EspoSandshrewLikeTraceReturnData { status, response },
                    ));
                }

                Event::CreateAlkane(create) => {
                    out.push(EspoSandshrewLikeTraceEvent::Create(trace_id_to_short_id(
                        create.new_alkane.as_ref(),
                    )));
                }

                Event::ReceiveIntent(_) => {}
                Event::ValueTransfer(_) => {}
            }
        }
    }

    Ok(out)
}

fn fmt_u128_hex(u: &alkanes::Uint128) -> String {
    let v = ((u.hi as u128) << 64) | (u.lo as u128);
    format!("0x{:x}", v)
}

fn fmt_bytes_hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(2 + b.len() * 2);
    s.push_str("0x");
    for byte in b {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", byte);
    }
    s
}

fn bytes_to_string_or_hex(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => fmt_bytes_hex(b),
    }
}

pub fn prettyify_protobuf_trace_json(trace: &AlkanesTrace) -> Result<String> {
    Ok(serde_json::to_string(&protobuf_trace_events(trace)?)
        .context("serialize normalized events")?)
}

fn outpoint_bytes_to_display(outpoint: &[u8]) -> String {
    let (txid_le, vout_le) = outpoint.split_at(32);
    let mut txid_be = txid_le.to_vec();
    txid_be.reverse();
    let vout = u32::from_le_bytes(vout_le.try_into().expect("vout 4 bytes"));
    format!("{}:{}", hex::encode(txid_be), vout)
}

fn alkane_cellpack_from_protostone(protostone: &Protostone) -> Option<Cellpack> {
    if protostone.protocol_tag != 1 || protostone.message.is_empty() {
        return None;
    }

    let calldata: Vec<u8> = protostone.message.iter().flat_map(|v| v.to_be_bytes()).collect();
    let Ok(varint_list) = decode_varint_list(&mut Cursor::new(calldata)) else {
        return None;
    };

    TryInto::<Cellpack>::try_into(varint_list).ok()
}

#[cfg(test)]
fn tx_has_alkanes_protocol(tx: &Transaction) -> bool {
    let Some(Artifact::Runestone(ref runestone)) = Runestone::decipher(tx) else {
        return false;
    };
    let Ok(protostones) = Protostone::from_runestone(runestone) else {
        return false;
    };
    protostones
        .iter()
        .any(|protostone| alkane_cellpack_from_protostone(protostone).is_some())
}

// parse possibly-tailed trace (strip trailing u32 if needed)
pub fn traces_for_block_as_prost(block: u64) -> Result<Vec<PartialEspoTrace>> {
    get_metashrew().traces_for_block_as_prost(block)
}

pub fn traces_for_block_as_json_str(block: u64) -> Result<String> {
    let partial_traces = traces_for_block_as_prost(block)?;
    let mut entries: Vec<serde_json::Value> = Vec::new();

    for partial_trace in partial_traces {
        let events_v: serde_json::Value =
            serde_json::from_str(&prettyify_protobuf_trace_json(&partial_trace.protobuf_trace)?)?;

        entries.push(json!({
            "outpoint": outpoint_bytes_to_display(&partial_trace.outpoint),
            "events": events_v,
        }));
    }

    let final_json =
        serde_json::to_string(&entries).context("ESPO: failed to serialize final entries array")?;

    Ok(final_json)
}

fn match_trace_outpoint_txid(
    outpoint: &[u8],
    allow_txids: Option<&HashSet<Txid>>,
) -> Option<(Txid, u32)> {
    if outpoint.len() < 36 {
        return None;
    }
    let (txid_bytes, vout_le) = outpoint.split_at(32);
    let vout = u32::from_le_bytes(vout_le[..4].try_into().ok()?);

    let direct = Txid::from_slice(txid_bytes).ok();

    let mut reversed = [0u8; 32];
    reversed.copy_from_slice(txid_bytes);
    reversed.reverse();
    let reversed = Txid::from_slice(&reversed).ok();

    if let Some(allow) = allow_txids {
        if let Some(txid) = direct {
            if allow.contains(&txid) {
                return Some((txid, vout));
            }
        }
        if let Some(txid) = reversed {
            if allow.contains(&txid) {
                return Some((txid, vout));
            }
        }
        return None;
    }

    reversed.or(direct).map(|txid| (txid, vout))
}

/// Build a map { canonical txid => Vec<(vout, PartialEspoTrace)> } for quick attach later.
fn partial_traces_indexed(
    partials: Vec<PartialEspoTrace>,
    allow_txids: Option<&HashSet<Txid>>,
) -> Result<HashMap<Txid, Vec<(u32, PartialEspoTrace)>>> {
    let mut map: HashMap<Txid, Vec<(u32, PartialEspoTrace)>> = HashMap::new();
    for p in partials {
        let Some((txid, vout)) = match_trace_outpoint_txid(&p.outpoint, allow_txids) else {
            continue;
        };
        map.entry(txid).or_default().push((vout, p));
    }

    for v in map.values_mut() {
        v.sort_by_key(|(vout, _)| *vout);
    }
    Ok(map)
}

#[derive(Debug, Default)]
struct CanonicalTraceSelection {
    traces_by_txid: HashMap<Txid, Vec<(u32, PartialEspoTrace)>>,
    recovered_txids: Vec<String>,
    missing_candidate_txids: Vec<String>,
    unexpected_height_trace_txids: Vec<String>,
}

fn trace_txid_key_variants(txid: &Txid) -> [[u8; 32]; 2] {
    let direct = *txid.as_byte_array();
    let mut reversed = direct;
    reversed.reverse();
    [direct, reversed]
}

fn trace_txid_allow_set(selected: &[(Txid, Transaction)]) -> HashSet<[u8; 32]> {
    let mut out = HashSet::with_capacity(selected.len().saturating_mul(2));
    for (txid, _) in selected {
        for key in trace_txid_key_variants(txid) {
            out.insert(key);
        }
    }
    out
}

fn select_canonical_traces(
    block: u64,
    canonical_txids: &HashSet<Txid>,
    selected: &[(Txid, Transaction)],
    alkane_protocol_txids: &HashSet<Txid>,
) -> Result<CanonicalTraceSelection> {
    if std::env::var_os("ESPO_SKIP_CANONICAL_TRACES").is_some() {
        return Ok(CanonicalTraceSelection::default());
    }

    let metashrew = get_metashrew();
    let metashrew_sdb = get_metashrew_sdb();
    metashrew_sdb
        .catch_up_now()
        .with_context(|| format!("metashrew catch_up before validating block {block}"))?;
    let block_u32: u32 = block
        .try_into()
        .context("block height does not fit into u32 for canonical trace selection")?;
    metashrew
        .ensure_canonical_height_with_db(metashrew_sdb.as_ref(), block_u32)
        .with_context(|| format!("metashrew not canonical at block {block}"))?;

    let allow_txids = trace_txid_allow_set(selected);
    let height_partials = metashrew
        .traces_for_block_as_prost_with_db_uncaught_filtered(
            metashrew_sdb.as_ref(),
            block,
            Some(&allow_txids),
        )
        .with_context(|| format!("failed traces_for_block_as_prost_with_db_uncaught({block})"))?;

    let mut traces_by_txid: HashMap<Txid, Vec<(u32, PartialEspoTrace)>> = HashMap::new();
    let mut recovered_txids: Vec<String> = Vec::new();
    let mut missing_candidate_txids: Vec<String> = Vec::new();
    let selected_txids: HashSet<Txid> = selected.iter().map(|(txid, _)| *txid).collect();

    let mut height_index = partial_traces_indexed(height_partials, Some(&selected_txids))?;

    for (txid, _tx) in selected {
        if let Some(vouts_partials) = height_index.remove(txid) {
            traces_by_txid.insert(*txid, vouts_partials);
            continue;
        }

        if !alkane_protocol_txids.contains(txid) {
            continue;
        }
        if !recover_missing_traces_by_txid() {
            missing_candidate_txids.push(txid.to_string());
            continue;
        }

        let fallback_partials = metashrew
            .traces_for_tx_with_db_uncaught(metashrew_sdb.as_ref(), txid)
            .with_context(|| format!("failed traces_for_tx_with_db_uncaught({txid})"))?;
        let allow = HashSet::from([*txid]);
        let mut fallback_index = partial_traces_indexed(fallback_partials, Some(&allow))?;
        if let Some(vouts_partials) = fallback_index.remove(txid) {
            traces_by_txid.insert(*txid, vouts_partials);
            recovered_txids.push(txid.to_string());
            continue;
        }

        missing_candidate_txids.push(txid.to_string());
    }

    let unexpected_height_trace_txids: Vec<String> = height_index
        .keys()
        .filter(|txid| !canonical_txids.contains(*txid))
        .map(ToString::to_string)
        .collect();

    Ok(CanonicalTraceSelection {
        traces_by_txid,
        recovered_txids,
        missing_candidate_txids,
        unexpected_height_trace_txids,
    })
}

/// Use the BlockSource for the block (header + transactions), Electrum for prevouts.
/// Traces are now **multiple per transaction** and are stitched in per outpoint (vout).
pub fn get_espo_block(block: u64, tip: u64) -> Result<EspoBlock> {
    get_espo_block_with_opts(block, tip, None)
}

pub fn get_espo_block_with_opts(
    block: u64,
    tip: u64,
    opts: Option<GetEspoBlockOpts>,
) -> Result<EspoBlock> {
    eprintln!("[TRACE::get_espo_block] start block={block}, tip={tip}");

    let block_source = get_block_source();

    // Block height conversions
    let h32: u32 = block
        .try_into()
        .context("block height does not fit into u32 for get_espo_block")?;
    let tip32: u32 =
        tip.try_into().context("tip height does not fit into u32 for get_espo_block")?;
    eprintln!("[TRACE::get_espo_block] converted block heights h32={h32}, tip32={tip32}");

    // Fetch block
    let block_result = block_source
        .get_block_result_by_height(h32, tip32)
        .context("BlockSource: get_block_result_by_height")?;
    let fee_summary = block_result.fee_summary;
    let full_block = block_result.block;
    let total_txs = full_block.txdata.len();
    eprintln!("[TRACE::get_espo_block] got block at height={}, txs={}", h32, total_txs);

    // Header from block source
    let block_header: Header = full_block.header.clone();
    let (host_function_values, alkane_protocol_txids) = {
        let mut header_bytes = Vec::new();
        full_block
            .header
            .consensus_encode(&mut header_bytes)
            .context("consensus encode block header for host function values")?;

        let coinbase_tx = full_block
            .txdata
            .get(0)
            .cloned()
            .context("block has no coinbase transaction for host function values")?;
        let mut coinbase_bytes = Vec::new();
        coinbase_tx
            .consensus_encode(&mut coinbase_bytes)
            .context("consensus encode coinbase tx for host function values")?;

        let total_fees: u128 =
            coinbase_tx.output.iter().map(|out| out.value.to_sat() as u128).sum();
        let total_fees_bytes = total_fees.to_le_bytes().to_vec();

        let mut diesel_mints: u128 = 0;
        let mut alkane_protocol_txids: HashSet<Txid> = HashSet::new();
        for (tx_idx, tx) in full_block.txdata.iter().enumerate() {
            if let Some(Artifact::Runestone(ref runestone)) = Runestone::decipher(tx) {
                let protostones = match Protostone::from_runestone(runestone) {
                    Ok(items) => items,
                    Err(err) => {
                        if std::env::var_os("ESPO_LOG_DIESEL_MINTS").is_some() {
                            eprintln!(
                                "[TRACE::get_espo_block] diesel mint protostone parse failed: tx_index={tx_idx} txid={} err={err:#}",
                                tx.compute_txid()
                            );
                        }
                        continue;
                    }
                };
                let mut has_alkane_protocol = false;
                for protostone in protostones {
                    let Some(cellpack) = alkane_cellpack_from_protostone(&protostone) else {
                        continue;
                    };

                    has_alkane_protocol = true;
                    if cellpack.target == AlkaneId::new(2, 0)
                        && !cellpack.inputs.is_empty()
                        && cellpack.inputs[0] == 77
                    {
                        diesel_mints = diesel_mints.saturating_add(1);
                        break;
                    }
                }
                if has_alkane_protocol {
                    alkane_protocol_txids.insert(tx.compute_txid());
                }
            }
        }
        let diesel_mints_bytes = diesel_mints.to_le_bytes().to_vec();

        (
            (header_bytes, coinbase_bytes, diesel_mints_bytes, total_fees_bytes),
            alkane_protocol_txids,
        )
    };

    let (page_start, page_end) =
        opts.as_ref().map(|o| o.page_range(total_txs)).unwrap_or((0, total_txs));

    // Select only the requested page of transactions
    let mut canonical_txids: HashSet<Txid> = HashSet::with_capacity(total_txs);
    let mut selected: Vec<(Txid, Transaction)> =
        Vec::with_capacity(page_end.saturating_sub(page_start));
    for (idx, tx) in full_block.txdata.into_iter().enumerate() {
        let txid = tx.compute_txid();
        canonical_txids.insert(txid);
        if idx < page_start || idx >= page_end {
            continue;
        }
        selected.push((txid, tx));
    }

    let mut canonical_traces = if h32 >= alkanes_genesis_block(get_network()) {
        let canonical_traces =
            select_canonical_traces(block, &canonical_txids, &selected, &alkane_protocol_txids)?;
        if !canonical_traces.recovered_txids.is_empty()
            || !canonical_traces.missing_candidate_txids.is_empty()
            || !canonical_traces.unexpected_height_trace_txids.is_empty()
        {
            eprintln!(
                "[reorg] metashrew trace mismatch at block {}: recovered_missing_txids={} missing_candidate_txids={} unexpected_height_trace_txids={}",
                block,
                canonical_traces.recovered_txids.join(","),
                canonical_traces.missing_candidate_txids.join(","),
                canonical_traces.unexpected_height_trace_txids.join(",")
            );
        }
        canonical_traces
    } else {
        CanonicalTraceSelection::default()
    };
    eprintln!(
        "[TRACE::get_espo_block] built canonical traces_index for block={} ({} txs with traces)",
        block,
        canonical_traces.traces_by_txid.len()
    );

    // Build transactions
    let mut txs: Vec<EspoAlkanesTransaction> = Vec::with_capacity(selected.len());
    for (txid, tx) in selected.into_iter() {
        let traces_opt: Option<Vec<EspoTrace>> =
            if let Some(vouts_partials) = canonical_traces.traces_by_txid.remove(&txid) {
                let txid_hex = txid.to_string();
                let mut traces_vec: Vec<EspoTrace> = Vec::with_capacity(vouts_partials.len());
                for (vout, partial) in vouts_partials {
                    let events = protobuf_trace_events(&partial.protobuf_trace)?;

                    let sandshrew_trace =
                        EspoSandshrewLikeTrace { outpoint: format!("{txid_hex}:{vout}"), events };

                    let storage_changes = extract_alkane_storage(&partial.protobuf_trace, &tx)?;
                    let outpoint =
                        EspoOutpoint { txid: txid.as_byte_array().to_vec(), vout, tx_spent: None };

                    traces_vec.push(EspoTrace {
                        sandshrew_trace,
                        protobuf_trace: partial.protobuf_trace,
                        storage_changes,
                        outpoint,
                    });
                }
                Some(traces_vec)
            } else {
                None
            };
        txs.push(EspoAlkanesTransaction { traces: traces_opt, transaction: tx });
    }
    eprintln!(
        "[TRACE::get_espo_block] built {} EspoAlkanesTransaction(s) (page {}..{})",
        txs.len(),
        page_start,
        page_end
    );

    eprintln!("[TRACE::get_espo_block] done block={block}");
    Ok(EspoBlock {
        block_header,
        tx_count: total_txs,
        transactions: txs,
        host_function_values,
        fee_summary,
        height: block
            .try_into()
            .context("block height does not fit into u32 for EspoBlock::height")?,
        is_latest: block == tip,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness, absolute};

    fn sample_tx(lock_time: u32) -> Transaction {
        Transaction {
            version: bitcoin::transaction::Version(2),
            lock_time: absolute::LockTime::from_consensus(lock_time),
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::default(),
            }],
            output: vec![TxOut {
                value: bitcoin::Amount::from_sat(0),
                script_pubkey: ScriptBuf::new(),
            }],
        }
    }

    fn sample_partial(txid: &Txid, vout: u32) -> PartialEspoTrace {
        let mut outpoint = txid.as_byte_array().to_vec();
        outpoint.reverse();
        outpoint.extend_from_slice(&vout.to_le_bytes());
        PartialEspoTrace { protobuf_trace: AlkanesTrace { events: Vec::new() }, outpoint }
    }

    #[test]
    fn partial_traces_indexed_accepts_le_outpoints_with_be_allow_list() {
        let tx = sample_tx(1);
        let txid = tx.compute_txid();
        let indexed = partial_traces_indexed(vec![sample_partial(&txid, 2)], None)
            .expect("index partial traces");
        let entries = indexed.get(&txid).expect("entry for txid");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, 2);
    }

    #[test]
    fn tx_has_alkanes_protocol_returns_false_for_plain_tx() {
        let tx = sample_tx(2);
        assert!(!tx_has_alkanes_protocol(&tx));
    }

    #[test]
    fn tx_has_alkanes_protocol_detects_cellpack_without_canonical_trace() {
        let raw = hex::decode(concat!(
            "0200000000010173490f9241ff4f2b11e59555233b2c1aae44c58b5553ad9023e39fc9ea67a33b",
            "0200000000ffffffff0322020000000000002251200a3571c7d2419230a38031eb2fe9a6f9a61a",
            "048475b20b3381e05a87ffcd94e00000000000000000296a5d26ff7f8190ec82d08bc0a886",
            "ad82c48892a0f4c601ff7f86d1aee5ce95edc0ea958688d5b9b10b241f030000000000160014",
            "96b87abec6cea3a5a15d7dc7ba748a9bbeef884602473044022004a05eaae7a4af1ca384cd8ff0",
            "b46af478ff22d14d50638b76e70434e319db91022061a667262a1dcf405c2937e21aa2c0f7bd508",
            "3298e9513aff5f69b5f39f7cfb801210374e106c95d47879e75196e5e82d0d38fa5a4441d7e8e",
            "106ab618bf673b20058000000000"
        ))
        .expect("valid hex");
        let tx: Transaction = bitcoin::consensus::deserialize(&raw).expect("valid tx");

        assert_eq!(
            tx.compute_txid().to_string(),
            "fc40bc89baf56dfcccf53bfaafac930518c658caac70929808a2a21c1e4a8aa0"
        );
        assert!(tx_has_alkanes_protocol(&tx));
    }
}
