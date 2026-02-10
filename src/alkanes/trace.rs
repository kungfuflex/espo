use crate::config::{
    get_block_source, // NEW: use BlockSource for full blocks
    get_metashrew,
};
use crate::core::blockfetcher::BlockSource;
use crate::schemas::EspoOutpoint;
use crate::schemas::SchemaAlkaneId;
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
use serde_json::{Value, json};
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
    let mut out: Vec<Value> = Vec::with_capacity(trace.events.len() as usize);

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

                    let myself = inner.myself.as_ref();
                    let caller = inner.caller.as_ref();

                    let my_block = myself.and_then(|m| m.block.as_ref());
                    let my_tx = myself.and_then(|m| m.tx.as_ref());
                    let caller_block = caller.and_then(|c| c.block.as_ref());
                    let caller_tx = caller.and_then(|c| c.tx.as_ref());

                    let inputs_hex: Vec<String> = inner.inputs.iter().map(fmt_u128_hex).collect();

                    let incoming_alkanes: Vec<Value> = inner
                        .incoming_alkanes
                        .iter()
                        .map(|t| {
                            let id = t.id.as_ref();
                            json!({
                                "id": {
                                    "block": id.and_then(|x| x.block.as_ref()).map(fmt_u128_hex).unwrap_or_else(|| "0x0".to_string()),
                                    "tx":    id.and_then(|x| x.tx.as_ref()).map(fmt_u128_hex).unwrap_or_else(|| "0x0".to_string()),
                                },
                                "value": t.value.as_ref().map(fmt_u128_hex).unwrap_or_else(|| "0x0".to_string()),
                            })
                        })
                        .collect();

                    out.push(json!({
                        "event": "invoke",
                        "data": {
                            "type": typ,
                            "context": {
                                "myself": {
                                    "block": my_block.map(fmt_u128_hex).unwrap_or_else(|| "0x0".to_string()),
                                    "tx":    my_tx.map(fmt_u128_hex).unwrap_or_else(|| "0x0".to_string()),
                                },
                                "caller": {
                                    "block": caller_block.map(fmt_u128_hex).unwrap_or_else(|| "0x0".to_string()),
                                    "tx":    caller_tx.map(fmt_u128_hex).unwrap_or_else(|| "0x0".to_string()),
                                },
                                "inputs": inputs_hex,
                                "incomingAlkanes": incoming_alkanes,
                                "vout": inner.vout,
                            },
                            "fuel": ctx.fuel,
                        }
                    }));
                }

                Event::ExitContext(exit) => {
                    let status = match exit.status() {
                        alkanes::AlkanesTraceStatusFlag::Failure => "failure",
                        _ => "success",
                    };

                    let resp = exit.response.as_ref();

                    let alkanes_list: Vec<Value> = resp
                        .map(|r| {
                            r.alkanes
                                .iter()
                                .map(|t| {
                                    let id = t.id.as_ref();
                                    json!({
                                        "id": {
                                            "block": id.and_then(|x| x.block.as_ref()).map(fmt_u128_hex).unwrap_or_else(|| "0x0".to_string()),
                                            "tx":    id.and_then(|x| x.tx.as_ref()).map(fmt_u128_hex).unwrap_or_else(|| "0x0".to_string()),
                                        },
                                        "value": t.value.as_ref().map(fmt_u128_hex).unwrap_or_else(|| "0x0".to_string()),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    let storage_list: Vec<Value> = resp
                        .map(|r| {
                            r.storage
                                .iter()
                                .map(|kv| {
                                    let k = bytes_to_string_or_hex(&kv.key);
                                    let v = fmt_bytes_hex(&kv.value);
                                    json!({ "key": k, "value": v })
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    let data_hex =
                        resp.map(|r| fmt_bytes_hex(&r.data)).unwrap_or_else(|| "0x".to_string());

                    out.push(json!({
                        "event": "return",
                        "data": {
                            "status": status,
                            "response": {
                                "alkanes": alkanes_list,
                                "data": data_hex,
                                "storage": storage_list,
                            }
                        }
                    }));
                }

                Event::CreateAlkane(create) => {
                    let id: Option<&alkanes::AlkaneId> = create.new_alkane.as_ref();
                    let block_hex = id
                        .and_then(|x| x.block.as_ref())
                        .map(fmt_u128_hex)
                        .unwrap_or_else(|| "0x0".to_string());
                    let tx_hex = id
                        .and_then(|x| x.tx.as_ref())
                        .map(fmt_u128_hex)
                        .unwrap_or_else(|| "0x0".to_string());
                    out.push(json!({
                        "event": "create",
                        "data": { "block": block_hex, "tx": tx_hex }
                    }));
                }

                Event::ReceiveIntent(_) => {}
                Event::ValueTransfer(_) => {}
            }
        }
    }

    Ok(serde_json::to_string(&out).context("serialize normalized events")?)
}

fn outpoint_bytes_to_display(outpoint: &[u8]) -> String {
    let (txid_le, vout_le) = outpoint.split_at(32);
    let mut txid_be = txid_le.to_vec();
    txid_be.reverse();
    let vout = u32::from_le_bytes(vout_le.try_into().expect("vout 4 bytes"));
    format!("{}:{}", hex::encode(txid_be), vout)
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

/// Build a map { txid_be_hex => Vec<(vout, PartialEspoTrace)> } for quick attach later.
fn traces_for_block_indexed(
    block: u64,
    allow_txids: Option<&HashSet<String>>,
) -> Result<HashMap<String, Vec<(u32, PartialEspoTrace)>>> {
    let partials = traces_for_block_as_prost(block)
        .with_context(|| format!("failed traces_for_block_as_prost({block})"))?;

    let mut map: HashMap<String, Vec<(u32, PartialEspoTrace)>> = HashMap::new();
    for p in partials {
        if p.outpoint.len() < 36 {
            continue;
        }
        let (tx_bytes, vout_le) = p.outpoint.split_at(32);
        let vout = u32::from_le_bytes(vout_le[..4].try_into().expect("vout 4 bytes"));

        let txid_hex_be = hex::encode(tx_bytes);
        let mut tx_bytes_rev = tx_bytes.to_vec();
        tx_bytes_rev.reverse();
        let txid_hex_le = hex::encode(&tx_bytes_rev);

        let txid_hex = if let Some(allow) = allow_txids {
            if allow.contains(&txid_hex_be) {
                txid_hex_be
            } else if allow.contains(&txid_hex_le) {
                txid_hex_le
            } else {
                continue;
            }
        } else {
            txid_hex_le
        };

        map.entry(txid_hex).or_default().push((vout, p));
    }

    for v in map.values_mut() {
        v.sort_by_key(|(vout, _)| *vout);
    }
    Ok(map)
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
    let full_block = block_source
        .get_block_by_height(h32, tip32)
        .context("BlockSource: get_block_by_height")?;
    let total_txs = full_block.txdata.len();
    eprintln!("[TRACE::get_espo_block] got block at height={}, txs={}", h32, total_txs);

    // Header from block source
    let block_header: Header = full_block.header.clone();
    let host_function_values = {
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
                for protostone in protostones {
                    if protostone.protocol_tag != 1 {
                        continue;
                    }
                    let calldata: Vec<u8> =
                        protostone.message.iter().flat_map(|v| v.to_be_bytes()).collect();
                    if calldata.is_empty() {
                        continue;
                    }
                    let varint_list = match decode_varint_list(&mut Cursor::new(calldata)) {
                        Ok(list) => list,
                        Err(err) => {
                            if std::env::var_os("ESPO_LOG_DIESEL_MINTS").is_some() {
                                eprintln!(
                                    "[TRACE::get_espo_block] diesel mint decode failed: txid={} err={err:#}",
                                    tx.compute_txid()
                                );
                            }
                            continue;
                        }
                    };
                    if varint_list.len() < 2 {
                        continue;
                    }
                    if let Ok(cellpack) = TryInto::<Cellpack>::try_into(varint_list) {
                        if cellpack.target == AlkaneId::new(2, 0)
                            && !cellpack.inputs.is_empty()
                            && cellpack.inputs[0] == 77
                        {
                            diesel_mints = diesel_mints.saturating_add(1);
                            break;
                        }
                    }
                }
            }
        }
        let diesel_mints_bytes = diesel_mints.to_le_bytes().to_vec();

        (header_bytes, coinbase_bytes, diesel_mints_bytes, total_fees_bytes)
    };

    let (page_start, page_end) =
        opts.as_ref().map(|o| o.page_range(total_txs)).unwrap_or((0, total_txs));

    // Select only the requested page of transactions
    let mut selected: Vec<(Txid, Transaction)> =
        Vec::with_capacity(page_end.saturating_sub(page_start));
    for (idx, tx) in full_block.txdata.into_iter().enumerate() {
        if idx < page_start || idx >= page_end {
            continue;
        }
        let txid = tx.compute_txid();
        selected.push((txid, tx));
    }

    let allow_txids: HashSet<String> = selected.iter().map(|(txid, _)| txid.to_string()).collect();

    // Index traces only for the selected txids
    let traces_index = traces_for_block_indexed(block, Some(&allow_txids))?;
    eprintln!(
        "[TRACE::get_espo_block] built traces_index for block={} ({} txs with traces)",
        block,
        traces_index.len()
    );

    // Build transactions
    let mut txs: Vec<EspoAlkanesTransaction> = Vec::with_capacity(selected.len());
    for (txid, tx) in selected.into_iter() {
        let txid_hex = txid.to_string();

        let traces_opt: Option<Vec<EspoTrace>> =
            if let Some(vouts_partials) = traces_index.get(&txid_hex) {
                let mut traces_vec: Vec<EspoTrace> = Vec::with_capacity(vouts_partials.len());
                for (vout, partial) in vouts_partials.iter() {
                    let events_json_str = prettyify_protobuf_trace_json(&partial.protobuf_trace)?;
                    let events: Vec<EspoSandshrewLikeTraceEvent> =
                        serde_json::from_str(&events_json_str)
                            .context("deserialize sandshrew-like events")?;

                    let sandshrew_trace =
                        EspoSandshrewLikeTrace { outpoint: format!("{txid_hex}:{vout}"), events };

                    let storage_changes = extract_alkane_storage(&partial.protobuf_trace, &tx)?;
                    let outpoint = EspoOutpoint {
                        txid: txid.as_byte_array().to_vec(),
                        vout: *vout,
                        tx_spent: None,
                    };

                    traces_vec.push(EspoTrace {
                        sandshrew_trace,
                        protobuf_trace: partial.protobuf_trace.clone(),
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
        height: block
            .try_into()
            .context("block height does not fit into u32 for EspoBlock::height")?,
        is_latest: block == tip,
    })
}
