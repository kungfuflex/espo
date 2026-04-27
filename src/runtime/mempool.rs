use crate::alkanes::trace::{
    EspoSandshrewLikeTrace, EspoSandshrewLikeTraceEvent, EspoTrace, extract_alkane_storage,
    prettyify_protobuf_trace_json,
};
use crate::bitcoind_flexible::FlexibleBitcoindClient as CoreClient;
use crate::config::{get_bitcoind_rpc_client, get_config, get_metashrew_rpc_url};
use crate::schemas::EspoOutpoint;
use anyhow::{Context, Result};
use bitcoin::block::Version as BlockVersion;
use bitcoin::blockdata::block::Header;
use bitcoin::blockdata::transaction::Version as TxVersion;
use bitcoin::consensus::Encodable;
use bitcoin::consensus::encode::deserialize;
use bitcoin::hashes::Hash;
use bitcoin::{
    Address, Amount, Block, CompactTarget, Network, OutPoint, Sequence, Transaction, TxIn,
    TxMerkleNode, TxOut, Txid, Witness,
};
use bitcoincore_rpc::RpcApi;
use futures::{StreamExt, stream};
use ordinals::{Artifact, Runestone};
use prost::Message;
use protorune_support::proto::protorune;
use protorune_support::protostone::Protostone;
use protorune_support::utils::decode_varint_list;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Cursor;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

/// --- Tunables (edit as needed) ---
pub const MEMPOOL_POLL_SECS: u64 = 5;
pub const MEMPOOL_PREVIEW_BATCH_SIZE: usize = 10;
pub const MEMPOOL_PREVIEW_TX_CONCURRENCY: usize = 6;
pub const MEMPOOL_LOG_STEP: usize = 100;
pub const MEMPOOL_MAX_TXS: usize = 50_000;
pub const MEMPOOL_MIN_FEE_RATE_SATS_VBYTE: f64 = 0.5;
/// --- End tunables ---

#[derive(Clone, Debug)]
pub struct MempoolEntry {
    pub txid: Txid,
    pub tx: Transaction,
    pub traces: Option<Vec<EspoTrace>>,
    pub first_seen: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MempoolTxReadiness {
    MetadataOnly,
    Hydrated,
    TracePending,
    TraceReady,
}

impl Default for MempoolTxReadiness {
    fn default() -> Self {
        Self::MetadataOnly
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct MempoolProjectedPosition {
    pub block: usize,
    pub vsize: u64,
}

#[derive(Clone, Debug)]
pub struct MempoolTransactionStruct {
    pub txid: Txid,
    pub raw_tx: Option<Vec<u8>>,
    pub tx: Option<Transaction>,
    pub protostones: Vec<Protostone>,
    pub fixed_trace: Option<Vec<EspoTrace>>,
    pub diesel_trace: Option<Vec<EspoTrace>>,
    pub first_seen: u64,
    pub fee_sat: u64,
    pub weight: u64,
    pub vsize: u64,
    pub fee_rate: f64,
    pub inputs: Vec<Txid>,
    pub spent_outpoints: Vec<OutPoint>,
    pub addresses: Vec<String>,
    pub is_diesel_mint: bool,
    pub template_index: Option<usize>,
    pub position: Option<MempoolProjectedPosition>,
    pub readiness: MempoolTxReadiness,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MempoolBlockTemplate {
    pub index: usize,
    pub tx_count: usize,
    pub trace_count: usize,
    pub weight: u64,
    pub vsize: u64,
    pub total_fees: u64,
    pub median_fee_rate: Option<f64>,
    pub min_fee_rate: Option<f64>,
    pub max_fee_rate: Option<f64>,
    pub fee_range: Vec<f64>,
    pub transaction_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct MempoolBlockDelta {
    pub index: usize,
    pub sequence: u64,
    pub reset: bool,
    pub added_count: usize,
    pub removed_count: usize,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
    pub full: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct MempoolBlockTx {
    pub txid: Txid,
    pub tx: Transaction,
    pub traces: Option<Vec<EspoTrace>>,
    pub fee_sat: u64,
    pub vsize: u64,
    pub fee_rate: f64,
    pub position: Option<MempoolProjectedPosition>,
    pub readiness: MempoolTxReadiness,
}

#[derive(Clone, Debug)]
pub struct MempoolBlockDetail {
    pub template: MempoolBlockTemplate,
    pub tx_total: usize,
    pub txs: Vec<MempoolBlockTx>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MempoolSyncPhase {
    Starting,
    Syncing,
    Hydrating,
    InSync,
    Stale,
}

impl Default for MempoolSyncPhase {
    fn default() -> Self {
        Self::Starting
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct MempoolSyncStatus {
    pub phase: MempoolSyncPhase,
    pub in_sync: bool,
    pub hydrating: bool,
    pub stale: bool,
    pub hydration_pending: usize,
    pub last_raw_refresh_at: Option<u64>,
    pub last_successful_raw_refresh_at: Option<u64>,
    pub last_error: Option<String>,
    pub clear_protection_until: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct MempoolSnapshot {
    pub tx_count: usize,
    pub updated_at: u64,
    pub sequence: u64,
    pub status: MempoolSyncStatus,
    pub blocks: Vec<MempoolBlockTemplate>,
    pub deltas: Vec<MempoolBlockDelta>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct MempoolBlockSummary {
    pub index: usize,
    pub tx_count: usize,
    pub trace_count: usize,
    pub weight: u64,
    pub vsize: u64,
    pub total_fees: u64,
    pub median_fee_rate: Option<f64>,
    pub min_fee_rate: Option<f64>,
    pub max_fee_rate: Option<f64>,
    pub fee_range: Vec<f64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct MempoolCompactSnapshot {
    pub tx_count: usize,
    pub updated_at: u64,
    pub sequence: u64,
    pub status: MempoolSyncStatus,
    pub blocks: Vec<MempoolBlockSummary>,
    pub deltas: Vec<MempoolBlockDelta>,
}

#[derive(Default)]
struct InMemoryMempool {
    txs: HashMap<Txid, MempoolTransactionStruct>,
    templates: Vec<MempoolBlockTemplate>,
    deltas: Vec<MempoolBlockDelta>,
    status: MempoolSyncStatus,
    sequence: u64,
    updated_at: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct VerboseMempoolEntry {
    #[serde(default)]
    vsize: Option<u64>,
    #[serde(default)]
    weight: Option<u64>,
    #[serde(default)]
    fees: Option<VerboseMempoolFees>,
    #[serde(default)]
    depends: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct VerboseMempoolFees {
    #[serde(default)]
    base: Option<f64>,
    #[serde(default)]
    modified: Option<f64>,
    #[serde(default)]
    ancestor: Option<f64>,
}

#[derive(Clone)]
struct MinerTx {
    fee: u64,
    weight: u64,
    adjusted_vsize: u64,
    fee_rate: f64,
    dependency_rate: f64,
    inputs: Vec<Txid>,
    spent_outpoints: Vec<OutPoint>,
    ancestors: HashSet<Txid>,
    children: HashSet<Txid>,
    ancestor_fee: u64,
    ancestor_vsize: u64,
    score: f64,
    used: bool,
    modified: bool,
}

static IN_MEMORY_MEMPOOL: OnceLock<Arc<RwLock<InMemoryMempool>>> = OnceLock::new();
static TRACE_QUEUE: OnceLock<Arc<Mutex<VecDeque<Txid>>>> = OnceLock::new();
static MEMPOOL_EVENTS: OnceLock<broadcast::Sender<String>> = OnceLock::new();
static HYDRATION_RUNNING: AtomicBool = AtomicBool::new(false);

fn mempool_state() -> &'static Arc<RwLock<InMemoryMempool>> {
    IN_MEMORY_MEMPOOL.get_or_init(|| Arc::new(RwLock::new(InMemoryMempool::default())))
}

fn trace_queue() -> &'static Arc<Mutex<VecDeque<Txid>>> {
    TRACE_QUEUE.get_or_init(|| Arc::new(Mutex::new(VecDeque::new())))
}

pub fn subscribe_mempool_events() -> broadcast::Receiver<String> {
    mempool_event_sender().subscribe()
}

fn mempool_event_sender() -> &'static broadcast::Sender<String> {
    MEMPOOL_EVENTS.get_or_init(|| {
        let (sender, _) = broadcast::channel(128);
        sender
    })
}

fn publish_mempool_event(event: &Value) {
    if let Ok(encoded) = serde_json::to_string(event) {
        let _ = mempool_event_sender().send(encoded);
    }
}

fn update_mempool_status<F>(f: F)
where
    F: FnOnce(&mut MempoolSyncStatus),
{
    let Ok(mut state) = mempool_state().write() else { return };
    f(&mut state.status);
    state.updated_at = now_ts();
}

fn mark_raw_refresh_start() {
    update_mempool_status(|status| {
        status.phase = MempoolSyncPhase::Syncing;
        status.in_sync = false;
        status.stale = false;
        status.last_raw_refresh_at = Some(now_ts());
        status.last_error = None;
    });
}

fn mark_raw_refresh_success() {
    update_mempool_status(|status| {
        status.phase =
            if status.hydrating { MempoolSyncPhase::Hydrating } else { MempoolSyncPhase::InSync };
        status.in_sync = !status.hydrating;
        status.stale = false;
        status.last_successful_raw_refresh_at = Some(now_ts());
        status.last_error = None;
    });
}

fn mark_raw_refresh_error(error: &anyhow::Error) {
    let message = format!("{error:?}");
    update_mempool_status(|status| {
        status.phase = MempoolSyncPhase::Stale;
        status.in_sync = false;
        status.stale = true;
        status.last_error = Some(message);
    });
}

fn set_hydration_status(hydrating: bool, pending: usize) {
    update_mempool_status(|status| {
        status.hydrating = hydrating;
        status.hydration_pending = pending;
        if hydrating {
            status.phase = MempoolSyncPhase::Hydrating;
            status.in_sync = false;
        } else if status.stale {
            status.phase = MempoolSyncPhase::Stale;
            status.in_sync = false;
        } else {
            status.phase = MempoolSyncPhase::InSync;
            status.in_sync = true;
        }
    });
}

fn now_ts() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn protostones_for_tx(tx: &Transaction) -> Vec<Protostone> {
    match Runestone::decipher(tx) {
        Some(Artifact::Runestone(r)) => Protostone::from_runestone(&r).unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn btc_to_sat(value: f64) -> u64 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    (value * 100_000_000.0).round().max(0.0) as u64
}

fn tx_inputs(tx: &Transaction) -> Vec<Txid> {
    tx.input
        .iter()
        .filter_map(|vin| (!vin.previous_output.is_null()).then_some(vin.previous_output.txid))
        .collect()
}

fn tx_spent_outpoints(tx: &Transaction) -> Vec<OutPoint> {
    tx.input
        .iter()
        .filter_map(|vin| (!vin.previous_output.is_null()).then_some(vin.previous_output))
        .collect()
}

fn cellpack_from_protostone(
    protostone: &Protostone,
) -> Option<alkanes_support::cellpack::Cellpack> {
    if protostone.protocol_tag != 1 || protostone.message.is_empty() {
        return None;
    }
    let calldata: Vec<u8> = protostone.message.iter().flat_map(|v| v.to_be_bytes()).collect();
    let Ok(values) = decode_varint_list(&mut Cursor::new(calldata)) else {
        return None;
    };
    TryInto::<alkanes_support::cellpack::Cellpack>::try_into(values).ok()
}

fn is_diesel_mint_protostone(protostones: &[Protostone]) -> bool {
    protostones.iter().any(|protostone| {
        let Some(cellpack) = cellpack_from_protostone(protostone) else {
            return false;
        };
        cellpack.target.block == 2
            && cellpack.target.tx == 0
            && cellpack.inputs.first() == Some(&77)
    })
}

fn block_subsidy_sats(height: u64) -> u64 {
    let halvings = height / 210_000;
    if halvings >= 64 {
        return 0;
    }
    5_000_000_000u64 >> halvings
}

fn hex_u128(value: u128) -> String {
    format!("0x{value:x}")
}

fn diesel_trace_for_tx(
    txid: &Txid,
    tx: &Transaction,
    vout: u32,
    mint_amount: u128,
) -> Option<Vec<EspoTrace>> {
    let mut raw: Value = serde_json::from_str(include_str!("diesel-mint-trace.json")).ok()?;
    if let Some(incoming) = raw
        .get_mut(0)
        .and_then(|v| v.get_mut("data"))
        .and_then(|v| v.get_mut("context"))
        .and_then(|v| v.get_mut("incomingAlkanes"))
        .and_then(|v| v.as_array_mut())
        .and_then(|arr| arr.get_mut(0))
    {
        incoming["value"] = Value::String(hex_u128(mint_amount));
    }
    if let Some(v) = raw
        .get_mut(0)
        .and_then(|v| v.get_mut("data"))
        .and_then(|v| v.get_mut("context"))
        .and_then(|v| v.get_mut("vout"))
    {
        *v = json!(vout);
    }
    if let Some(alkanes) = raw
        .get_mut(3)
        .and_then(|v| v.get_mut("data"))
        .and_then(|v| v.get_mut("response"))
        .and_then(|v| v.get_mut("alkanes"))
        .and_then(|v| v.as_array_mut())
        .and_then(|arr| arr.get_mut(0))
    {
        alkanes["value"] = Value::String(hex_u128(mint_amount));
    }

    let events: Vec<EspoSandshrewLikeTraceEvent> = serde_json::from_value(raw).ok()?;
    let sandshrew_trace = EspoSandshrewLikeTrace { outpoint: format!("{}:{}", txid, vout), events };
    let outpoint = EspoOutpoint { txid: txid.to_byte_array().to_vec(), vout, tx_spent: None };
    let protobuf_trace = alkanes_support::proto::alkanes::AlkanesTrace::default();
    let storage_changes = extract_alkane_storage(&protobuf_trace, tx).unwrap_or_default();
    Some(vec![EspoTrace { sandshrew_trace, protobuf_trace, storage_changes, outpoint }])
}

fn combined_traces(entry: &MempoolTransactionStruct) -> Option<Vec<EspoTrace>> {
    entry.diesel_trace.clone().or_else(|| entry.fixed_trace.clone())
}

fn derive_readiness(entry: &MempoolTransactionStruct) -> MempoolTxReadiness {
    if entry.tx.is_none() {
        MempoolTxReadiness::MetadataOnly
    } else if entry.protostones.is_empty() {
        MempoolTxReadiness::Hydrated
    } else if entry.diesel_trace.is_some() || entry.fixed_trace.is_some() {
        MempoolTxReadiness::TraceReady
    } else {
        MempoolTxReadiness::TracePending
    }
}

fn snapshot_from_state(state: &InMemoryMempool) -> MempoolSnapshot {
    MempoolSnapshot {
        tx_count: state.txs.len(),
        updated_at: state.updated_at,
        sequence: state.sequence,
        status: state.status.clone(),
        blocks: state.templates.clone(),
        deltas: state.deltas.clone(),
    }
}

fn block_summary(template: &MempoolBlockTemplate) -> MempoolBlockSummary {
    MempoolBlockSummary {
        index: template.index,
        tx_count: template.tx_count,
        trace_count: template.trace_count,
        weight: template.weight,
        vsize: template.vsize,
        total_fees: template.total_fees,
        median_fee_rate: template.median_fee_rate,
        min_fee_rate: template.min_fee_rate,
        max_fee_rate: template.max_fee_rate,
        fee_range: template.fee_range.clone(),
    }
}

fn compact_snapshot_from_state(
    state: &InMemoryMempool,
    include_deltas: bool,
) -> MempoolCompactSnapshot {
    MempoolCompactSnapshot {
        tx_count: state.txs.len(),
        updated_at: state.updated_at,
        sequence: state.sequence,
        status: state.status.clone(),
        blocks: state.templates.iter().map(block_summary).collect(),
        deltas: if include_deltas { state.deltas.clone() } else { Vec::new() },
    }
}

pub fn current_mempool_snapshot() -> MempoolSnapshot {
    let Ok(state) = mempool_state().read() else {
        return MempoolSnapshot::default();
    };
    snapshot_from_state(&state)
}

pub fn current_mempool_compact_snapshot() -> MempoolCompactSnapshot {
    let Ok(state) = mempool_state().read() else {
        return MempoolCompactSnapshot::default();
    };
    compact_snapshot_from_state(&state, false)
}

pub fn current_mempool_compact_snapshot_with_deltas() -> MempoolCompactSnapshot {
    let Ok(state) = mempool_state().read() else {
        return MempoolCompactSnapshot::default();
    };
    compact_snapshot_from_state(&state, true)
}

pub fn get_mempool_block_transaction_ids(index: usize) -> Vec<String> {
    let Ok(state) = mempool_state().read() else { return Vec::new() };
    state
        .templates
        .iter()
        .find(|template| template.index == index)
        .map(|template| template.transaction_ids.clone())
        .unwrap_or_default()
}

pub fn get_mempool_transactions(txids: &[Txid]) -> HashMap<Txid, Transaction> {
    let Ok(state) = mempool_state().read() else {
        return HashMap::new();
    };
    txids
        .iter()
        .filter_map(|txid| {
            state.txs.get(txid).and_then(|entry| entry.tx.clone().map(|tx| (*txid, tx)))
        })
        .collect()
}

pub fn get_mempool_block_detail(
    index: usize,
    page: usize,
    limit: usize,
    traces_only: bool,
    hide_diesel_mints: bool,
) -> Option<MempoolBlockDetail> {
    let Ok(state) = mempool_state().read() else {
        return None;
    };
    let template = state.templates.iter().find(|template| template.index == index)?.clone();
    let mut ordered: Vec<Txid> = template
        .transaction_ids
        .iter()
        .filter_map(|txid_str| Txid::from_str(txid_str).ok())
        .collect();
    ordered.retain(|txid| {
        let Some(entry) = state.txs.get(txid) else {
            return false;
        };
        if traces_only && entry.protostones.is_empty() {
            return false;
        }
        if hide_diesel_mints && entry.is_diesel_mint {
            return false;
        }
        true
    });
    ordered.sort_by(|a, b| {
        let aa = state.txs.get(a);
        let bb = state.txs.get(b);
        let a_rate = aa.map(|tx| tx.fee_rate).unwrap_or_default();
        let b_rate = bb.map(|tx| tx.fee_rate).unwrap_or_default();
        let a_fee = aa.map(|tx| tx.fee_sat).unwrap_or_default();
        let b_fee = bb.map(|tx| tx.fee_sat).unwrap_or_default();
        b_rate
            .partial_cmp(&a_rate)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b_fee.cmp(&a_fee))
            .then_with(|| a.cmp(b))
    });
    let tx_total = ordered.len();
    let off = limit.saturating_mul(page.saturating_sub(1));
    let end = off.saturating_add(limit).min(tx_total);
    let txs = if off < tx_total {
        ordered[off..end]
            .iter()
            .filter_map(|txid| {
                let entry = state.txs.get(txid)?;
                let tx = entry.tx.clone()?;
                Some(MempoolBlockTx {
                    txid: *txid,
                    tx,
                    traces: combined_traces(entry),
                    fee_sat: entry.fee_sat,
                    vsize: entry.vsize,
                    fee_rate: entry.fee_rate,
                    position: entry.position.clone(),
                    readiness: derive_readiness(entry),
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    Some(MempoolBlockDetail { template, tx_total, txs })
}

pub fn publish_new_block_event(height: u32, txids: &[Txid]) {
    publish_mempool_event(&json!({
        "type": "block",
        "data": {
            "height": height,
            "txids": txids.iter().map(ToString::to_string).collect::<Vec<_>>(),
        }
    }));
}

fn shadow_base(tx: &Transaction) -> u32 {
    tx.output.len() as u32 + 1
}

fn encode_outpoint_hex(txid: &Txid, vout: u32) -> String {
    let mut outpoint = protorune::Outpoint::default();
    outpoint.txid = txid.to_byte_array().to_vec();
    outpoint.vout = vout;
    let bytes = outpoint.encode_to_vec();
    format!("0x{}", hex::encode(bytes))
}

fn build_preview_block_hex(tx: &Transaction) -> Result<String> {
    let coinbase = Transaction {
        version: TxVersion::TWO,
        lock_time: bitcoin::locktime::absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: bitcoin::OutPoint::null(),
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::from_slice(&[vec![0u8; 32]]),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_00000000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };

    let mut txs = Vec::with_capacity(2);
    txs.push(coinbase);
    txs.push(tx.clone());

    let txids: Vec<Txid> = txs.iter().map(|t| t.compute_txid()).collect();
    let merkle_root_txid =
        bitcoin::merkle_tree::calculate_root(txids.into_iter()).unwrap_or_else(Txid::all_zeros);

    let header = Header {
        version: BlockVersion::TWO,
        prev_blockhash: bitcoin::BlockHash::all_zeros(),
        merkle_root: TxMerkleNode::from(merkle_root_txid),
        time: now_ts() as u32,
        bits: CompactTarget::from_consensus(0x1d00ffff),
        nonce: 0,
    };

    let block = Block { header, txdata: txs };

    let mut buf = Vec::new();
    block.consensus_encode(&mut buf)?;
    Ok(hex::encode(buf))
}

fn decode_trace_hex(data_hex: &str, txid: &Txid, tx: &Transaction, vout: u32) -> Result<EspoTrace> {
    let trimmed = data_hex.strip_prefix("0x").unwrap_or(data_hex);
    let bytes = hex::decode(trimmed)?;
    let protobuf_trace = alkanes_support::proto::alkanes::AlkanesTrace::decode(bytes.as_slice())
        .with_context(|| "failed to decode preview trace protobuf")?;
    let events_json_str = prettyify_protobuf_trace_json(&protobuf_trace)?;
    let events: Vec<EspoSandshrewLikeTraceEvent> =
        serde_json::from_str(&events_json_str).context("deserialize preview trace events")?;

    let sandshrew_trace = EspoSandshrewLikeTrace { outpoint: format!("{}:{}", txid, vout), events };
    let storage_changes = extract_alkane_storage(&protobuf_trace, tx)?;
    let outpoint = EspoOutpoint { txid: txid.to_byte_array().to_vec(), vout, tx_spent: None };

    Ok(EspoTrace { sandshrew_trace, protobuf_trace, storage_changes, outpoint })
}

async fn preview_traces_for_tx(
    http: &Client,
    preview_url: &str,
    txid: &Txid,
    tx: &Transaction,
    protostone_count: usize,
) -> Option<Vec<EspoTrace>> {
    if protostone_count == 0 {
        return None;
    }
    let block_hex = match build_preview_block_hex(tx) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("[mempool] build preview block failed for {}: {e:?}", txid);
            return None;
        }
    };
    let base = shadow_base(tx);
    let mut jobs: Vec<(u32, String)> = Vec::with_capacity(protostone_count);
    for idx in 0..protostone_count {
        let vout = base + idx as u32;
        jobs.push((vout, encode_outpoint_hex(txid, vout)));
    }

    let mut traces: Vec<EspoTrace> = Vec::new();
    for batch in jobs.chunks(MEMPOOL_PREVIEW_BATCH_SIZE) {
        let owned_batch: Vec<(u32, String)> = batch.to_vec();
        let futs = stream::iter(owned_batch.into_iter().map(|(vout, input_hex)| {
            let body = json!({
                "jsonrpc": "2.0",
                "id": format!("{}:{}", txid, vout),
                "method": "metashrew_preview",
                "params": [
                    block_hex,
                    "trace",
                    input_hex,
                    "latest",
                ]
            });
            let http = http.clone();
            let preview_url = preview_url.to_string();
            let txid = *txid;
            async move {
                let resp_json: Value = match http.post(&preview_url).json(&body).send().await {
                    Ok(r) => match r.error_for_status() {
                        Ok(ok) => match ok.json().await {
                            Ok(v) => v,
                            Err(e) => {
                                eprintln!(
                                    "[mempool] preview decode failed for {}@{}: {:?}",
                                    txid, vout, e
                                );
                                return None;
                            }
                        },
                        Err(e) => {
                            eprintln!(
                                "[mempool] preview HTTP error for {}@{}: {:?}",
                                txid, vout, e
                            );
                            return None;
                        }
                    },
                    Err(e) => {
                        eprintln!("[mempool] preview POST failed for {}@{}: {:?}", txid, vout, e);
                        return None;
                    }
                };

                let result_hex = resp_json.get("result").and_then(|v| v.as_str()).or_else(|| {
                    resp_json.get("result").and_then(|v| v.get("trace")).and_then(|v| v.as_str())
                });
                let Some(result_hex) = result_hex else {
                    return None;
                };
                match decode_trace_hex(result_hex, &txid, tx, vout) {
                    Ok(trace) => Some(trace),
                    Err(e) => {
                        eprintln!(
                            "[mempool] decode preview trace {}@{} failed: {:?}",
                            txid, vout, e
                        );
                        None
                    }
                }
            }
        }))
        .buffer_unordered(MEMPOOL_PREVIEW_BATCH_SIZE);

        futures::pin_mut!(futs);
        while let Some(res) = futs.next().await {
            if let Some(t) = res {
                traces.push(t);
            }
        }
    }

    if traces.is_empty() { None } else { Some(traces) }
}

pub fn get_seen_txids_page(page: usize, limit: usize) -> (Vec<Txid>, bool) {
    if limit == 0 {
        return (Vec::new(), false);
    }
    let offset = limit.saturating_mul(page.saturating_sub(1));
    let Ok(state) = mempool_state().read() else {
        return (Vec::new(), false);
    };
    let mut entries: Vec<(u64, Txid)> =
        state.txs.values().map(|entry| (entry.first_seen, entry.txid)).collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let has_more = entries.len() > offset.saturating_add(limit);
    let txids = entries.into_iter().skip(offset).take(limit).map(|(_, txid)| txid).collect();

    (txids, has_more)
}

pub fn decode_seen_key(raw: &[u8]) -> Option<(u64, Txid)> {
    // Kept for compatibility with callers that decode older mempool cursor keys.
    if !raw.starts_with(b"seen/") || raw.len() < 5 + 8 + 1 {
        return None;
    }
    let rest = &raw[5..];
    if rest.len() < 8 + 1 || rest[8] != b'/' {
        return None;
    }
    let mut ts_bytes = [0u8; 8];
    ts_bytes.copy_from_slice(&rest[..8]);
    let ts = u64::from_be_bytes(ts_bytes);
    let txid_str = std::str::from_utf8(&rest[9..]).ok()?;
    let txid = Txid::from_str(txid_str).ok()?;
    Some((ts, txid))
}

pub fn reset_mempool_store() -> Result<()> {
    if let Ok(mut state) = mempool_state().write() {
        let total = state.txs.len();
        state.txs.clear();
        state.templates.clear();
        state.deltas.clear();
        state.sequence = state.sequence.saturating_add(1);
        state.status = MempoolSyncStatus {
            phase: MempoolSyncPhase::Starting,
            in_sync: false,
            hydrating: false,
            stale: false,
            hydration_pending: 0,
            last_raw_refresh_at: None,
            last_successful_raw_refresh_at: None,
            last_error: None,
            clear_protection_until: None,
        };
        state.updated_at = now_ts();
        eprintln!("[mempool] reset in-memory store: deleted {} txs", total);
    }
    if let Ok(mut queue) = trace_queue().lock() {
        queue.clear();
    }
    Ok(())
}

fn enqueue_trace(txid: Txid) {
    let Ok(mut queue) = trace_queue().lock() else { return };
    if !queue.iter().any(|existing| *existing == txid) {
        queue.push_back(txid);
    }
}

fn prune_trace_queue(removed: &HashSet<Txid>) {
    let Ok(mut queue) = trace_queue().lock() else { return };
    queue.retain(|txid| !removed.contains(txid));
}

fn build_memory_entry(
    txid: Txid,
    tx: Transaction,
    raw_tx: Vec<u8>,
    verbose: Option<&VerboseMempoolEntry>,
    network: Network,
    rpc: &CoreClient,
) -> MempoolTransactionStruct {
    let protostones = protostones_for_tx(&tx);
    let is_diesel_mint = is_diesel_mint_protostone(&protostones);
    let readiness = if protostones.is_empty() {
        MempoolTxReadiness::Hydrated
    } else {
        MempoolTxReadiness::TracePending
    };
    let first_seen = now_ts();
    let weight = verbose.and_then(|v| v.weight).unwrap_or_else(|| tx.weight().to_wu() as u64);
    let vsize = verbose.and_then(|v| v.vsize).unwrap_or_else(|| tx.vsize() as u64).max(1);
    let fee_sat = verbose
        .and_then(|v| v.fees.as_ref())
        .and_then(|fees| fees.modified.or(fees.base).or(fees.ancestor))
        .map(btc_to_sat)
        .unwrap_or(0);
    let fee_rate = fee_sat as f64 / vsize as f64;
    let inputs = if let Some(v) = verbose {
        v.depends.iter().filter_map(|s| Txid::from_str(s).ok()).collect()
    } else {
        tx_inputs(&tx)
    };
    let spent_outpoints = tx_spent_outpoints(&tx);
    let addresses = tx
        .output
        .iter()
        .filter_map(|o| Address::from_script(o.script_pubkey.as_script(), network).ok())
        .map(|addr| addr.to_string())
        .collect();
    let _ = rpc;
    MempoolTransactionStruct {
        txid,
        raw_tx: Some(raw_tx),
        tx: Some(tx),
        protostones,
        fixed_trace: None,
        diesel_trace: None,
        first_seen,
        fee_sat,
        weight,
        vsize,
        fee_rate,
        inputs,
        spent_outpoints,
        addresses,
        is_diesel_mint,
        template_index: None,
        position: None,
        readiness,
    }
}

fn build_memory_metadata_entry(
    txid: Txid,
    verbose: &VerboseMempoolEntry,
) -> MempoolTransactionStruct {
    let first_seen = now_ts();
    let vsize = verbose.vsize.unwrap_or(1).max(1);
    let weight = verbose.weight.unwrap_or_else(|| vsize.saturating_mul(4)).max(1);
    let fee_sat = verbose
        .fees
        .as_ref()
        .and_then(|fees| fees.modified.or(fees.base).or(fees.ancestor))
        .map(btc_to_sat)
        .unwrap_or(0);
    let fee_rate = fee_sat as f64 / vsize as f64;
    let inputs = verbose.depends.iter().filter_map(|s| Txid::from_str(s).ok()).collect();
    MempoolTransactionStruct {
        txid,
        raw_tx: None,
        tx: None,
        protostones: Vec::new(),
        fixed_trace: None,
        diesel_trace: None,
        first_seen,
        fee_sat,
        weight,
        vsize,
        fee_rate,
        inputs,
        spent_outpoints: Vec::new(),
        addresses: Vec::new(),
        is_diesel_mint: false,
        template_index: None,
        position: None,
        readiness: MempoolTxReadiness::MetadataOnly,
    }
}

fn upsert_memory_entry(entry: MempoolTransactionStruct) {
    let txid = entry.txid;
    let Ok(mut state) = mempool_state().write() else { return };
    let mut should_enqueue = !entry.protostones.is_empty() && !entry.is_diesel_mint;
    let mut removed_conflicts = HashSet::new();
    if !entry.spent_outpoints.is_empty() && entry.fee_sat > 0 {
        let spent: HashSet<OutPoint> = entry.spent_outpoints.iter().copied().collect();
        let conflicts: Vec<Txid> = state
            .txs
            .iter()
            .filter_map(|(existing_txid, existing)| {
                if *existing_txid == txid {
                    return None;
                }
                let spends_same_input =
                    existing.spent_outpoints.iter().any(|outpoint| spent.contains(outpoint));
                let is_rbf_replacement = spends_same_input
                    && entry.fee_sat > existing.fee_sat
                    && entry.fee_rate > existing.fee_rate;
                is_rbf_replacement.then_some(*existing_txid)
            })
            .collect();
        for conflict in conflicts {
            if state.txs.remove(&conflict).is_some() {
                removed_conflicts.insert(conflict);
            }
        }
    }
    match state.txs.get_mut(&txid) {
        Some(existing) => {
            existing.first_seen = existing.first_seen.min(entry.first_seen);
            existing.weight = entry.weight;
            existing.vsize = entry.vsize.max(1);
            if entry.fee_sat > 0 || existing.fee_sat == 0 {
                existing.fee_sat = entry.fee_sat;
            }
            existing.fee_rate = existing.fee_sat as f64 / existing.vsize as f64;
            existing.inputs = entry.inputs;
            if !entry.spent_outpoints.is_empty() {
                existing.spent_outpoints = entry.spent_outpoints;
            }
            existing.template_index = entry.template_index;
            if entry.tx.is_some() {
                existing.raw_tx = entry.raw_tx;
                existing.tx = entry.tx;
                existing.protostones = entry.protostones;
                existing.addresses = entry.addresses;
                existing.is_diesel_mint = entry.is_diesel_mint;
                existing.readiness = derive_readiness(existing);
                should_enqueue = !existing.protostones.is_empty()
                    && !existing.is_diesel_mint
                    && existing.fixed_trace.is_none();
            } else {
                should_enqueue = false;
            }
        }
        None => {
            state.txs.insert(txid, entry);
        }
    }
    state.updated_at = now_ts();
    drop(state);
    if !removed_conflicts.is_empty() {
        eprintln!(
            "[mempool] rbf removed {} conflicting txs replaced by {}",
            removed_conflicts.len(),
            txid
        );
        prune_trace_queue(&removed_conflicts);
    }
    if should_enqueue {
        enqueue_trace(txid);
    }
}

fn remove_missing_memory_entries(canonical: &HashSet<Txid>) -> usize {
    let Ok(mut state) = mempool_state().write() else { return 0 };
    let before = state.txs.len();
    let removed: HashSet<Txid> =
        state.txs.keys().filter(|txid| !canonical.contains(*txid)).copied().collect();
    state.txs.retain(|txid, _| canonical.contains(txid));
    state.updated_at = now_ts();
    let after = state.txs.len();
    drop(state);
    if !removed.is_empty() {
        prune_trace_queue(&removed);
    }
    before.saturating_sub(after)
}

fn remove_memory_txid(txid: &Txid) -> bool {
    let Ok(mut state) = mempool_state().write() else { return false };
    let removed = state.txs.remove(txid).is_some();
    if removed {
        state.updated_at = now_ts();
    }
    drop(state);
    if removed {
        let mut removed_set = HashSet::new();
        removed_set.insert(*txid);
        prune_trace_queue(&removed_set);
    }
    removed
}

fn set_relatives(txid: Txid, pool: &mut HashMap<Txid, MinerTx>, visiting: &mut HashSet<Txid>) {
    if !visiting.insert(txid) {
        return;
    }
    let inputs = pool.get(&txid).map(|tx| tx.inputs.clone()).unwrap_or_default();
    let mut ancestors = HashSet::new();
    for parent in inputs {
        if !pool.contains_key(&parent) {
            continue;
        }
        set_relatives(parent, pool, visiting);
        ancestors.insert(parent);
        if let Some(parent_tx) = pool.get(&parent) {
            for ancestor in &parent_tx.ancestors {
                ancestors.insert(*ancestor);
            }
        }
        if let Some(parent_tx) = pool.get_mut(&parent) {
            parent_tx.children.insert(txid);
        }
    }
    let mut ancestor_fee = pool.get(&txid).map(|tx| tx.fee).unwrap_or_default();
    let mut ancestor_vsize = pool.get(&txid).map(|tx| tx.adjusted_vsize).unwrap_or(1);
    for ancestor in &ancestors {
        if let Some(parent) = pool.get(ancestor) {
            ancestor_fee = ancestor_fee.saturating_add(parent.fee);
            ancestor_vsize = ancestor_vsize.saturating_add(parent.adjusted_vsize);
        }
    }
    if let Some(tx) = pool.get_mut(&txid) {
        tx.ancestors = ancestors;
        tx.ancestor_fee = ancestor_fee;
        tx.ancestor_vsize = ancestor_vsize.max(1);
        tx.score = tx.ancestor_fee as f64 / tx.ancestor_vsize as f64;
    }
    visiting.remove(&txid);
}

fn update_descendants(
    root: Txid,
    pool: &mut HashMap<Txid, MinerTx>,
    modified: &mut Vec<Txid>,
    cluster_rate: f64,
) {
    let mut stack: Vec<Txid> = pool
        .get(&root)
        .map(|tx| tx.children.iter().copied().collect())
        .unwrap_or_default();
    let mut seen = HashSet::new();
    while let Some(child) = stack.pop() {
        if !seen.insert(child) {
            continue;
        }
        let children: Vec<Txid> = pool
            .get(&child)
            .map(|tx| tx.children.iter().copied().collect())
            .unwrap_or_default();
        let mut changed = false;
        if let (Some(root_tx), Some(child_tx)) = (pool.get(&root).cloned(), pool.get_mut(&child)) {
            if child_tx.ancestors.remove(&root) {
                child_tx.ancestor_fee = child_tx.ancestor_fee.saturating_sub(root_tx.fee);
                child_tx.ancestor_vsize =
                    child_tx.ancestor_vsize.saturating_sub(root_tx.adjusted_vsize).max(1);
                child_tx.score = child_tx.ancestor_fee as f64 / child_tx.ancestor_vsize as f64;
                child_tx.dependency_rate = child_tx.dependency_rate.min(cluster_rate);
                changed = true;
            }
        }
        if changed {
            if let Some(child_tx) = pool.get_mut(&child) {
                if !child_tx.modified {
                    child_tx.modified = true;
                    modified.push(child);
                }
            }
        }
        stack.extend(children);
    }
}

fn calculate_block_templates(
    txs: &HashMap<Txid, MempoolTransactionStruct>,
    max_blocks: usize,
    weight_limit: u64,
) -> (Vec<Vec<Txid>>, HashMap<Txid, f64>) {
    let mut pool: HashMap<Txid, MinerTx> = txs
        .iter()
        .map(|(txid, tx)| {
            let adjusted_vsize = tx.vsize.max(1);
            (
                *txid,
                MinerTx {
                    fee: tx.fee_sat,
                    weight: tx.weight.max(adjusted_vsize * 4),
                    adjusted_vsize,
                    fee_rate: tx.fee_rate,
                    dependency_rate: tx.fee_rate,
                    inputs: tx
                        .inputs
                        .iter()
                        .copied()
                        .filter(|parent| txs.contains_key(parent))
                        .collect(),
                    spent_outpoints: tx.spent_outpoints.clone(),
                    ancestors: HashSet::new(),
                    children: HashSet::new(),
                    ancestor_fee: tx.fee_sat,
                    ancestor_vsize: adjusted_vsize,
                    score: tx.fee_rate,
                    used: false,
                    modified: false,
                },
            )
        })
        .collect();

    let keys: Vec<Txid> = pool.keys().copied().collect();
    for txid in keys {
        set_relatives(txid, &mut pool, &mut HashSet::new());
    }

    let mut mempool_array: Vec<Txid> = pool.keys().copied().collect();
    mempool_array.sort_by(|a, b| {
        let aa = pool.get(a).map(|tx| tx.score).unwrap_or_default();
        let bb = pool.get(b).map(|tx| tx.score).unwrap_or_default();
        bb.partial_cmp(&aa).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.cmp(b))
    });

    let mut blocks = Vec::new();
    let mut block = Vec::new();
    let mut block_weight = 4_000u64;
    let mut modified: Vec<Txid> = Vec::new();
    let mut overflow: Vec<Txid> = Vec::new();
    let mut failures = 0usize;
    let mut top = 0usize;
    let mut spent_outpoints: HashSet<OutPoint> = HashSet::new();
    let mut effective_rates: HashMap<Txid, f64> = HashMap::new();

    while top < mempool_array.len() || !modified.is_empty() {
        while top < mempool_array.len()
            && pool.get(&mempool_array[top]).map(|tx| tx.used || tx.modified).unwrap_or(true)
        {
            top += 1;
        }
        modified.sort_by(|a, b| {
            let aa = pool.get(a).map(|tx| tx.score).unwrap_or_default();
            let bb = pool.get(b).map(|tx| tx.score).unwrap_or_default();
            bb.partial_cmp(&aa).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.cmp(b))
        });

        let next_pool = mempool_array.get(top).copied();
        let next_modified = modified.first().copied();
        let next = match (next_pool, next_modified) {
            (Some(pool_tx), Some(mod_tx)) => {
                let pool_score = pool.get(&pool_tx).map(|tx| tx.score).unwrap_or_default();
                let mod_score = pool.get(&mod_tx).map(|tx| tx.score).unwrap_or_default();
                if pool_score > mod_score {
                    top += 1;
                    Some(pool_tx)
                } else {
                    modified.remove(0);
                    Some(mod_tx)
                }
            }
            (Some(pool_tx), None) => {
                top += 1;
                Some(pool_tx)
            }
            (None, Some(mod_tx)) => {
                modified.remove(0);
                Some(mod_tx)
            }
            (None, None) => None,
        };

        if let Some(next_txid) = next {
            if pool.get(&next_txid).map(|tx| tx.used).unwrap_or(true) {
                continue;
            }
            let next_tx = pool.get(&next_txid).cloned();
            let Some(next_tx) = next_tx else { continue };
            let package_fits = blocks.len() >= max_blocks.saturating_sub(1)
                || block_weight.saturating_add(next_tx.ancestor_vsize.saturating_mul(4))
                    < weight_limit;
            if package_fits {
                let mut package: Vec<Txid> = next_tx.ancestors.iter().copied().collect();
                package.sort_by_key(|txid| {
                    pool.get(txid).map(|tx| tx.ancestors.len()).unwrap_or_default()
                });
                package.push(next_txid);
                let package_conflicts = package.iter().any(|txid| {
                    pool.get(txid)
                        .map(|tx| {
                            tx.spent_outpoints
                                .iter()
                                .any(|outpoint| spent_outpoints.contains(outpoint))
                        })
                        .unwrap_or(false)
                });
                if package_conflicts {
                    if let Some(tx) = pool.get_mut(&next_txid) {
                        tx.used = true;
                    }
                    failures += 1;
                    continue;
                }
                let effective_rate = next_tx
                    .dependency_rate
                    .min(next_tx.ancestor_fee as f64 / next_tx.ancestor_vsize.max(1) as f64);
                let mut used = Vec::new();
                for txid in package {
                    if let Some(tx) = pool.get_mut(&txid) {
                        if tx.used {
                            continue;
                        }
                        tx.used = true;
                        tx.fee_rate = effective_rate;
                        for outpoint in &tx.spent_outpoints {
                            spent_outpoints.insert(*outpoint);
                        }
                        effective_rates.insert(txid, effective_rate);
                        block_weight = block_weight.saturating_add(tx.weight);
                        block.push(txid);
                        used.push(txid);
                    }
                }
                for txid in used {
                    update_descendants(txid, &mut pool, &mut modified, effective_rate);
                }
                failures = 0;
            } else {
                overflow.push(next_txid);
                failures += 1;
            }
        }

        let exceeded_tries = failures > 1000 && block_weight > weight_limit.saturating_sub(4_000);
        let queue_empty = top >= mempool_array.len() && modified.is_empty();
        if (exceeded_tries || queue_empty) && blocks.len() < max_blocks.saturating_sub(1) {
            if block.is_empty() {
                break;
            }
            blocks.push(block);
            block = Vec::new();
            block_weight = 4_000;
            for txid in overflow.drain(..).rev() {
                if pool.get(&txid).map(|tx| tx.modified).unwrap_or(false) {
                    modified.push(txid);
                } else {
                    top = top.saturating_sub(1);
                    if top < mempool_array.len() {
                        mempool_array[top] = txid;
                    }
                }
            }
        }
    }

    if !block.is_empty() {
        blocks.push(block);
    }
    (blocks, effective_rates)
}

fn fee_stats_for_block(
    block_txs: &[Txid],
    state: &HashMap<Txid, MempoolTransactionStruct>,
    effective_rates: &HashMap<Txid, f64>,
) -> (Option<f64>, Option<f64>, Option<f64>, Vec<f64>) {
    let mut rates: Vec<f64> = block_txs
        .iter()
        .filter_map(|txid| {
            effective_rates
                .get(txid)
                .copied()
                .or_else(|| state.get(txid).map(|tx| tx.fee_rate))
        })
        .collect();
    rates.retain(|v| v.is_finite());
    if rates.is_empty() {
        return (None, None, None, Vec::new());
    }
    rates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let min = rates.first().copied();
    let max = rates.last().copied();
    let median = rates.get(rates.len() / 2).copied();
    let last = rates.len().saturating_sub(1);
    let fee_range = [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0]
        .iter()
        .filter_map(|percentile| rates.get((last as f64 * percentile).round() as usize).copied())
        .collect();
    (median, min, max, fee_range)
}

fn calculate_template_deltas(
    previous: &[MempoolBlockTemplate],
    current: &[MempoolBlockTemplate],
    sequence: u64,
) -> Vec<MempoolBlockDelta> {
    const INLINE_DELTA_LIMIT: usize = 512;
    let mut deltas = Vec::new();
    for index in 0..previous.len().max(current.len()) {
        let prev_ids: HashSet<String> = previous
            .get(index)
            .map(|template| template.transaction_ids.iter().cloned().collect())
            .unwrap_or_default();
        let curr_ids: HashSet<String> = current
            .get(index)
            .map(|template| template.transaction_ids.iter().cloned().collect())
            .unwrap_or_default();
        let mut added: Vec<String> = curr_ids.difference(&prev_ids).cloned().collect();
        let mut removed: Vec<String> = prev_ids.difference(&curr_ids).cloned().collect();
        added.sort();
        removed.sort();
        let summary_changed = previous.get(index) != current.get(index);
        if !added.is_empty() || !removed.is_empty() || summary_changed {
            let added_count = added.len();
            let removed_count = removed.len();
            let reset = added_count.saturating_add(removed_count) > INLINE_DELTA_LIMIT;
            if reset {
                added.clear();
                removed.clear();
            }
            deltas.push(MempoolBlockDelta {
                index,
                sequence,
                reset,
                added_count,
                removed_count,
                added,
                removed,
                changed: Vec::new(),
                full: None,
            });
        }
    }
    deltas
}

fn recalculate_memory_templates() {
    let cfg = get_config().mempool.clone();
    let next_height = crate::config::get_espo_next_height() as u64;
    let template_input = {
        let Ok(state) = mempool_state().read() else { return };
        state
            .txs
            .iter()
            .map(|(txid, tx)| {
                (
                    *txid,
                    MempoolTransactionStruct {
                        txid: *txid,
                        raw_tx: None,
                        tx: None,
                        protostones: Vec::new(),
                        fixed_trace: None,
                        diesel_trace: None,
                        first_seen: tx.first_seen,
                        fee_sat: tx.fee_sat,
                        weight: tx.weight,
                        vsize: tx.vsize,
                        fee_rate: tx.fee_rate,
                        inputs: tx.inputs.clone(),
                        spent_outpoints: tx.spent_outpoints.clone(),
                        addresses: Vec::new(),
                        is_diesel_mint: tx.is_diesel_mint,
                        template_index: None,
                        position: None,
                        readiness: tx.readiness.clone(),
                    },
                )
            })
            .collect::<HashMap<_, _>>()
    };
    let (template_txids, effective_rates) =
        calculate_block_templates(&template_input, cfg.template_blocks, cfg.block_weight_units);
    let Ok(mut state) = mempool_state().write() else { return };
    let previous_templates = state.templates.clone();

    for tx in state.txs.values_mut() {
        tx.template_index = None;
        tx.diesel_trace = None;
        tx.position = None;
    }

    let mut templates = Vec::with_capacity(template_txids.len());
    for (index, txids) in template_txids.iter().enumerate() {
        let diesel_mints: Vec<Txid> = txids
            .iter()
            .filter(|txid| state.txs.get(*txid).map(|tx| tx.is_diesel_mint).unwrap_or(false))
            .copied()
            .collect();
        let per_mint = if diesel_mints.is_empty() {
            0
        } else {
            block_subsidy_sats(next_height + index as u64) as u128 / diesel_mints.len() as u128
        };
        for txid in &diesel_mints {
            if let Some(tx) = state.txs.get_mut(txid) {
                if let Some(transaction) = tx.tx.as_ref() {
                    let vout = shadow_base(transaction);
                    tx.diesel_trace = diesel_trace_for_tx(txid, transaction, vout, per_mint);
                }
            }
        }

        let mut weight = 0u64;
        let mut vsize = 0u64;
        let mut fees = 0u64;
        let mut trace_count = 0usize;
        for txid in txids {
            if let Some(tx) = state.txs.get_mut(txid) {
                tx.template_index = Some(index);
                tx.position = Some(MempoolProjectedPosition {
                    block: index,
                    vsize: vsize.saturating_add(tx.vsize / 2),
                });
                weight = weight.saturating_add(tx.weight);
                vsize = vsize.saturating_add(tx.vsize);
                fees = fees.saturating_add(tx.fee_sat);
                if !tx.protostones.is_empty() {
                    trace_count = trace_count.saturating_add(1);
                }
                tx.readiness = derive_readiness(tx);
            }
        }
        let (median_fee_rate, min_fee_rate, max_fee_rate, fee_range) =
            fee_stats_for_block(txids, &state.txs, &effective_rates);
        templates.push(MempoolBlockTemplate {
            index,
            tx_count: txids.len(),
            trace_count,
            weight,
            vsize,
            total_fees: fees,
            median_fee_rate,
            min_fee_rate,
            max_fee_rate,
            fee_range,
            transaction_ids: txids.iter().map(ToString::to_string).collect(),
        });
    }

    let templates_changed = previous_templates != templates;
    if templates_changed {
        state.sequence = state.sequence.saturating_add(1);
        state.deltas = calculate_template_deltas(&previous_templates, &templates, state.sequence);
    } else {
        state.deltas.clear();
    }
    state.templates = templates;
    state.updated_at = now_ts();
    let snapshot = compact_snapshot_from_state(&state, true);
    drop(state);
    publish_mempool_event(&json!({ "type": "mempool-blocks", "data": snapshot }));
}

async fn refresh_memory_mempool(rpc: &CoreClient, network: Network) -> Result<()> {
    mark_raw_refresh_start();
    let verbose: HashMap<String, VerboseMempoolEntry> = match rpc
        .call("getrawmempool", &[json!(true)])
        .context("bitcoind getrawmempool verbose failed")
    {
        Ok(verbose) => verbose,
        Err(e) => {
            mark_raw_refresh_error(&e);
            return Err(e);
        }
    };
    let cfg = get_config().mempool.clone();
    let mut entries: Vec<(&String, &VerboseMempoolEntry)> = verbose.iter().collect();
    if cfg.max_txs > 0 && entries.len() > cfg.max_txs {
        entries.sort_by(|(_, a), (_, b)| {
            let a_fee = a.fees.as_ref().and_then(|f| f.base).map(btc_to_sat).unwrap_or(0) as f64;
            let b_fee = b.fees.as_ref().and_then(|f| f.base).map(btc_to_sat).unwrap_or(0) as f64;
            let a_rate = a
                .vsize
                .filter(|vsize| *vsize > 0)
                .map(|vsize| a_fee / vsize as f64)
                .unwrap_or(0.0);
            let b_rate = b
                .vsize
                .filter(|vsize| *vsize > 0)
                .map(|vsize| b_fee / vsize as f64)
                .unwrap_or(0.0);
            b_rate.partial_cmp(&a_rate).unwrap_or(std::cmp::Ordering::Equal)
        });
        entries.truncate(cfg.max_txs);
    }
    let mut canonical = HashSet::with_capacity(entries.len());
    for (txid_str, entry) in entries {
        let Ok(txid) = Txid::from_str(txid_str) else { continue };
        canonical.insert(txid);
        upsert_memory_entry(build_memory_metadata_entry(txid, entry));
    }
    let current_count =
        mempool_state().read().ok().map(|state| state.txs.len()).unwrap_or_default();
    let now = now_ts();
    let clear_until = mempool_state()
        .read()
        .ok()
        .and_then(|state| state.status.clear_protection_until);
    let clear_active = clear_until.map(|until| until > now).unwrap_or(false);
    let sharp_drop = current_count > 20_000
        && canonical.len().saturating_mul(100) <= current_count.saturating_mul(80);
    let skip_removal = sharp_drop || (clear_active && canonical.len() < current_count);
    let mut protected_refresh = false;
    if skip_removal {
        protected_refresh = true;
        let until = clear_until
            .filter(|until| *until > now)
            .unwrap_or_else(|| now.saturating_add(cfg.clear_protection_secs.max(1)));
        update_mempool_status(|status| {
            status.phase = MempoolSyncPhase::Stale;
            status.in_sync = false;
            status.stale = true;
            status.clear_protection_until = Some(until);
            status.last_error = Some(format!(
                "clear protection active: canonical getrawmempool returned {}/{} txs",
                canonical.len(),
                current_count
            ));
        });
        eprintln!(
            "[mempool] clear protection active: refusing to remove txs after canonical getrawmempool returned {}/{} txs",
            canonical.len(),
            current_count
        );
    } else {
        update_mempool_status(|status| {
            if status.clear_protection_until.map(|until| until <= now).unwrap_or(false) {
                status.clear_protection_until = None;
            }
        });
        let removed = remove_missing_memory_entries(&canonical);
        if removed > 0 {
            eprintln!("[mempool] removed {} txs absent from canonical getrawmempool", removed);
        }
    }
    recalculate_memory_templates();
    start_mempool_hydration(network);
    if !protected_refresh {
        mark_raw_refresh_success();
    }
    Ok(())
}

fn start_mempool_hydration(network: Network) {
    if HYDRATION_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let txids = {
        let Ok(state) = mempool_state().read() else {
            HYDRATION_RUNNING.store(false, Ordering::SeqCst);
            return;
        };
        let mut ordered = Vec::new();
        let mut seen = HashSet::new();
        for template in &state.templates {
            for txid_str in &template.transaction_ids {
                let Ok(txid) = Txid::from_str(txid_str) else { continue };
                if seen.insert(txid)
                    && state.txs.get(&txid).map(|entry| entry.tx.is_none()).unwrap_or(false)
                {
                    ordered.push(txid);
                }
            }
        }
        let mut rest: Vec<Txid> = state
            .txs
            .iter()
            .filter_map(|(txid, entry)| (entry.tx.is_none() && seen.insert(*txid)).then_some(*txid))
            .collect();
        rest.sort_by(|a, b| {
            let aa = state.txs.get(a).map(|tx| tx.fee_rate).unwrap_or_default();
            let bb = state.txs.get(b).map(|tx| tx.fee_rate).unwrap_or_default();
            bb.partial_cmp(&aa).unwrap_or(std::cmp::Ordering::Equal)
        });
        ordered.extend(rest);
        ordered
    };
    let workers = get_config().mempool.hydration_workers.max(1);

    std::thread::spawn(move || {
        let total = txids.len();
        if total == 0 {
            HYDRATION_RUNNING.store(false, Ordering::SeqCst);
            set_hydration_status(false, 0);
            return;
        }
        let worker_count = workers.min(total);
        set_hydration_status(true, total);
        eprintln!("[mempool] hydrating {total} raw transactions with {worker_count} workers");

        let txids = Arc::new(txids);
        let next = Arc::new(AtomicUsize::new(0));
        let hydrated = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::with_capacity(worker_count);

        for _ in 0..worker_count {
            let txids = Arc::clone(&txids);
            let next = Arc::clone(&next);
            let hydrated = Arc::clone(&hydrated);
            handles.push(std::thread::spawn(move || {
                let rpc = get_bitcoind_rpc_client();
                loop {
                    let idx = next.fetch_add(1, Ordering::SeqCst);
                    let Some(txid) = txids.get(idx).copied() else { break };
                    let already_loaded = mempool_state()
                        .read()
                        .ok()
                        .and_then(|state| state.txs.get(&txid).map(|entry| entry.tx.is_some()))
                        .unwrap_or(false);
                    if already_loaded {
                        continue;
                    }
                    let raw_hex = match rpc.get_raw_transaction_hex(&txid, None) {
                        Ok(raw) => raw,
                        Err(e) => {
                            eprintln!("[mempool] hydrate getrawtransaction {} failed: {}", txid, e);
                            continue;
                        }
                    };
                    let raw_tx = match hex::decode(raw_hex.trim()) {
                        Ok(raw) => raw,
                        Err(e) => {
                            eprintln!("[mempool] hydrate decode raw tx {} failed: {}", txid, e);
                            continue;
                        }
                    };
                    let tx = match deserialize::<Transaction>(&raw_tx) {
                        Ok(tx) => tx,
                        Err(e) => {
                            eprintln!(
                                "[mempool] hydrate deserialize raw tx {} failed: {}",
                                txid, e
                            );
                            continue;
                        }
                    };
                    let mem_entry = build_memory_entry(txid, tx, raw_tx, None, network, rpc);
                    upsert_memory_entry(mem_entry);
                    let done = hydrated.fetch_add(1, Ordering::SeqCst) + 1;
                    if done % 1000 == 0 {
                        eprintln!("[mempool] hydrated {done}/{total} raw transactions");
                        recalculate_memory_templates();
                    }
                }
            }));
        }

        for handle in handles {
            if handle.join().is_err() {
                eprintln!("[mempool] hydration worker panicked");
            }
        }

        let hydrated = hydrated.load(Ordering::SeqCst);
        if hydrated > 0 {
            eprintln!("[mempool] hydrated {hydrated}/{total} raw transactions complete");
            recalculate_memory_templates();
        }
        HYDRATION_RUNNING.store(false, Ordering::SeqCst);
        set_hydration_status(false, total.saturating_sub(hydrated));
    });
}

async fn trace_worker(http: Client, preview_url: String) {
    loop {
        let next = trace_queue().lock().ok().and_then(|mut queue| queue.pop_front());
        let Some(txid) = next else {
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        };
        let entry = mempool_state().read().ok().and_then(|state| state.txs.get(&txid).cloned());
        let Some(entry) = entry else { continue };
        if entry.is_diesel_mint || entry.protostones.is_empty() || entry.fixed_trace.is_some() {
            continue;
        }
        let Some(transaction) = entry.tx.as_ref() else {
            continue;
        };
        let traces =
            preview_traces_for_tx(&http, &preview_url, &txid, transaction, entry.protostones.len())
                .await;
        if let Some(traces) = traces {
            if let Ok(mut state) = mempool_state().write() {
                if let Some(current) = state.txs.get_mut(&txid) {
                    current.fixed_trace = Some(traces);
                    current.readiness = derive_readiness(current);
                    state.updated_at = now_ts();
                }
            }
            recalculate_memory_templates();
        }
    }
}

fn ingest_zmq_rawtx(url: String, network: Network) {
    let rpc = get_bitcoind_rpc_client();
    std::thread::spawn(move || {
        let ctx = zmq::Context::new();
        let socket = match ctx.socket(zmq::SUB) {
            Ok(socket) => socket,
            Err(e) => {
                eprintln!("[mempool][zmq] socket init failed: {e}");
                return;
            }
        };
        if let Err(e) = socket.connect(&url) {
            eprintln!("[mempool][zmq] connect {url} failed: {e}");
            return;
        }
        if let Err(e) = socket.set_subscribe(b"rawtx") {
            eprintln!("[mempool][zmq] subscribe rawtx failed: {e}");
            return;
        }
        eprintln!("[mempool][zmq] subscribed to rawtx at {url}");
        loop {
            let topic = match socket.recv_bytes(0) {
                Ok(topic) => topic,
                Err(e) => {
                    eprintln!("[mempool][zmq] recv topic failed: {e}");
                    continue;
                }
            };
            let body = match socket.recv_bytes(0) {
                Ok(body) => body,
                Err(e) => {
                    eprintln!("[mempool][zmq] recv body failed: {e}");
                    continue;
                }
            };
            let _seq = socket.recv_bytes(0);
            if topic.as_slice() != b"rawtx" {
                continue;
            }
            let Ok(tx) = deserialize::<Transaction>(&body) else { continue };
            let txid = tx.compute_txid();
            let verbose: Option<VerboseMempoolEntry> =
                rpc.call("getmempoolentry", &[json!(txid.to_string())]).ok();
            let entry = build_memory_entry(txid, tx, body, verbose.as_ref(), network, &rpc);
            upsert_memory_entry(entry);
            recalculate_memory_templates();
        }
    });
}

fn ingest_zmq_sequence(url: String) {
    std::thread::spawn(move || {
        let ctx = zmq::Context::new();
        let socket = match ctx.socket(zmq::SUB) {
            Ok(socket) => socket,
            Err(e) => {
                eprintln!("[mempool][zmq] sequence socket init failed: {e}");
                return;
            }
        };
        if let Err(e) = socket.connect(&url) {
            eprintln!("[mempool][zmq] sequence connect {url} failed: {e}");
            return;
        }
        if let Err(e) = socket.set_subscribe(b"sequence") {
            eprintln!("[mempool][zmq] subscribe sequence failed: {e}");
            return;
        }
        eprintln!("[mempool][zmq] subscribed to sequence at {url}");
        loop {
            let topic = match socket.recv_bytes(0) {
                Ok(topic) => topic,
                Err(e) => {
                    eprintln!("[mempool][zmq] sequence recv topic failed: {e}");
                    continue;
                }
            };
            let body = match socket.recv_bytes(0) {
                Ok(body) => body,
                Err(e) => {
                    eprintln!("[mempool][zmq] sequence recv body failed: {e}");
                    continue;
                }
            };
            let _seq = socket.recv_bytes(0);
            if topic.as_slice() != b"sequence" || body.len() < 33 {
                continue;
            }
            let label = body[32] as char;
            if label != 'R' {
                continue;
            }
            let mut removed_any = false;
            if let Ok(txid) = Txid::from_slice(&body[..32]) {
                if remove_memory_txid(&txid) {
                    eprintln!("[mempool][zmq] sequence removed {txid}");
                    removed_any = true;
                }
            }
            let mut reversed = body[..32].to_vec();
            reversed.reverse();
            if let Ok(txid) = Txid::from_slice(&reversed) {
                if remove_memory_txid(&txid) {
                    eprintln!("[mempool][zmq] sequence removed {txid}");
                    removed_any = true;
                }
            }
            if removed_any {
                recalculate_memory_templates();
            }
        }
    });
}

pub async fn run_mempool_service(network: Network) -> Result<()> {
    let rpc = get_bitcoind_rpc_client();
    let preview_url = get_metashrew_rpc_url().to_string();
    let http = Client::new();
    let cfg = get_config().mempool.clone();

    if !cfg.enabled {
        eprintln!("[mempool] disabled by config");
        return Ok(());
    }

    eprintln!(
        "[mempool] service starting (raw_poll={}s, template_poll={}s, preview_url={})",
        cfg.raw_poll_secs, cfg.template_poll_secs, preview_url
    );

    eprintln!("[mempool] startup getrawmempool refresh");
    if let Err(e) = refresh_memory_mempool(&rpc, network).await {
        eprintln!("[mempool] startup refresh failed: {e:?}");
    }

    if let Some(zmq_url) = cfg.zmq_rawtx_url.as_ref().and_then(|s| normalize_zmq_url(s)) {
        ingest_zmq_rawtx(zmq_url, network);
    }
    if let Some(zmq_url) = cfg.zmq_sequence_url.as_ref().and_then(|s| normalize_zmq_url(s)) {
        ingest_zmq_sequence(zmq_url);
    }

    for _ in 0..cfg.trace_workers.max(1) {
        tokio::spawn(trace_worker(http.clone(), preview_url.clone()));
    }

    let template_poll = Duration::from_secs(cfg.template_poll_secs.max(1));
    let raw_poll = Duration::from_secs(cfg.raw_poll_secs.max(1));
    let mut last_raw_refresh = SystemTime::now();

    loop {
        let should_refresh = last_raw_refresh.elapsed().unwrap_or_default() >= raw_poll;
        if should_refresh {
            eprintln!("[mempool] canonical getrawmempool refresh");
            if let Err(e) = refresh_memory_mempool(&rpc, network).await {
                eprintln!("[mempool] canonical refresh failed: {e:?}");
            }
            last_raw_refresh = SystemTime::now();
        } else {
            recalculate_memory_templates();
        }

        tokio::time::sleep(template_poll).await;
    }
}

fn normalize_zmq_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

pub fn get_tx_from_mempool(txid: &Txid) -> Option<MempoolEntry> {
    let state = mempool_state().read().ok()?;
    let entry = state.txs.get(txid)?;
    let tx = entry.tx.clone()?;
    Some(MempoolEntry {
        txid: *txid,
        tx,
        traces: combined_traces(entry),
        first_seen: entry.first_seen,
    })
}

pub fn pending_by_txid(txid: &Txid) -> Option<MempoolEntry> {
    get_tx_from_mempool(txid)
}

pub fn pending_for_address(addr: &str) -> Vec<MempoolEntry> {
    let Ok(state) = mempool_state().read() else { return Vec::new() };
    let mut out: Vec<MempoolEntry> = state
        .txs
        .values()
        .filter(|entry| entry.addresses.iter().any(|a| a == addr))
        .filter_map(|entry| {
            let tx = entry.tx.clone()?;
            Some(MempoolEntry {
                txid: entry.txid,
                tx,
                traces: combined_traces(entry),
                first_seen: entry.first_seen,
            })
        })
        .collect();
    out.sort_by(|a, b| b.first_seen.cmp(&a.first_seen));
    out
}

pub fn purge_confirmed_txids(txids: &[Txid]) -> Result<usize> {
    let Ok(mut state) = mempool_state().write() else { return Ok(0) };
    let mut removed = 0usize;
    for txid in txids {
        if state.txs.remove(txid).is_some() {
            removed += 1;
        }
    }
    if removed > 0 {
        state.updated_at = now_ts();
    }
    drop(state);
    if removed > 0 {
        recalculate_memory_templates();
    }
    Ok(removed)
}

pub fn purge_confirmed_from_chain() -> Result<usize> {
    let rpc = get_bitcoind_rpc_client();
    let txids: Vec<Txid> = mempool_state()
        .read()
        .ok()
        .map(|state| state.txs.keys().copied().collect())
        .unwrap_or_default();
    let mut confirmed = Vec::new();
    for txid in txids {
        if let Ok(info) = rpc.get_raw_transaction_info(&txid, None) {
            if info.blockhash.is_some() {
                confirmed.push(txid);
            }
        }
    }

    let removed = purge_confirmed_txids(&confirmed)?;
    if removed > 0 {
        eprintln!("[mempool] purged {} confirmed txs from store", removed);
    }
    Ok(removed)
}
