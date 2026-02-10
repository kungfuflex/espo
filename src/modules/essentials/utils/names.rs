use crate::alkanes::trace::{
    EspoBlock, EspoSandshrewLikeTraceEvent, EspoSandshrewLikeTraceShortId,
};
use crate::config::get_metashrew_rpc_url;
use crate::modules::essentials::utils::inspections::StoredInspectionResult;
use crate::schemas::SchemaAlkaneId;
use alkanes_support::cellpack::Cellpack;
use alkanes_support::id::AlkaneId;
use alkanes_support::proto::alkanes::MessageContextParcel;
use alkanes_support::proto::alkanes::SimulateResponse as SimulateProto;
use anyhow::{Context, Result};
use bitcoin::consensus::Encodable;
use bitcoin::locktime::absolute::LockTime;
use bitcoin::transaction::Version;
use bitcoin::{Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut};
use ordinals::Runestone;
use prost::Message;
use protorune_support::protostone::{Protostone, Protostones};
use reqwest::Client;
use serde_json::json;
use std::future::Future;
use tokio::runtime::{Handle, Runtime};
use tokio::task::block_in_place;

pub fn normalize_alkane_name(name: &str) -> Option<String> {
    let trimmed = name.trim_matches('\0').trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

fn parse_short_id(id: &EspoSandshrewLikeTraceShortId) -> Option<SchemaAlkaneId> {
    fn parse_u32_or_hex(s: &str) -> Option<u32> {
        if let Some(hex) = s.strip_prefix("0x") {
            return u32::from_str_radix(hex, 16).ok();
        }
        s.parse::<u32>().ok()
    }
    fn parse_u64_or_hex(s: &str) -> Option<u64> {
        if let Some(hex) = s.strip_prefix("0x") {
            return u64::from_str_radix(hex, 16).ok();
        }
        s.parse::<u64>().ok()
    }

    let block = parse_u32_or_hex(&id.block)?;
    let tx = parse_u64_or_hex(&id.tx)?;
    Some(SchemaAlkaneId { block, tx })
}

fn decode_name_bytes(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let text = String::from_utf8(bytes.to_vec()).ok()?;
    let trimmed = text.trim_matches('\0').trim().to_string();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) {
        return None;
    }
    Some(trimmed)
}

fn name_from_creation_trace(block: &EspoBlock, alkane: &SchemaAlkaneId) -> Option<String> {
    for tx in block.transactions.iter() {
        let Some(traces) = tx.traces.as_ref() else { continue };
        for trace in traces.iter() {
            let created = trace.sandshrew_trace.events.iter().any(|ev| {
                matches!(ev, EspoSandshrewLikeTraceEvent::Create(create) if parse_short_id(create).as_ref() == Some(alkane))
            });
            if !created {
                continue;
            }
            let Some(kvs) = trace.storage_changes.get(alkane) else { continue };
            let Some((_txid, value)) = kvs.get(b"/name".as_slice()) else { continue };
            if let Some(name) = decode_name_bytes(value) {
                return Some(name);
            }
        }
    }
    None
}

fn has_get_name_method(inspection: &StoredInspectionResult) -> bool {
    inspection
        .metadata
        .as_ref()
        .map(|meta| meta.methods.iter().any(|m| m.name == "get_name" && m.opcode == 99))
        .unwrap_or(false)
}

pub fn get_name(
    block: &EspoBlock,
    alkane: &SchemaAlkaneId,
    inspection: Option<&StoredInspectionResult>,
) -> Option<String> {
    if let Some(name) = name_from_creation_trace(block, alkane) {
        return Some(name);
    }
    let Some(inspection) = inspection else { return None };
    if !has_get_name_method(inspection) {
        return None;
    }
    match simulate_get_name(alkane, block.height) {
        Ok(name) => name,
        Err(e) => {
            eprintln!(
                "[ESSENTIALS] get_name simulate failed for {}:{}: {e}",
                alkane.block, alkane.tx
            );
            None
        }
    }
}

fn simulate_get_name(alkane: &SchemaAlkaneId, height: u32) -> Result<Option<String>> {
    block_on_result(simulate_get_name_async(alkane, height))
}

async fn simulate_get_name_async(alkane: &SchemaAlkaneId, height: u32) -> Result<Option<String>> {
    let cellpack = Cellpack {
        target: AlkaneId { block: alkane.block as u128, tx: alkane.tx as u128 },
        inputs: vec![99],
    };
    let calldata = cellpack.encipher();
    let protostone = Protostone {
        burn: None,
        message: calldata.clone(),
        edicts: Vec::new(),
        refund: None,
        pointer: Some(0),
        from: None,
        protocol_tag: 1,
    };
    let protocol_values = vec![protostone].encipher().context("protostone encode failed")?;
    let runestone =
        Runestone { protocol: Some(protocol_values), pointer: Some(0), ..Default::default() };
    let runestone_script = runestone.encipher();

    let dummy_tx = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![
            TxOut { value: Amount::from_sat(0), script_pubkey: ScriptBuf::new() },
            TxOut { value: Amount::from_sat(0), script_pubkey: runestone_script },
        ],
    };

    let mut tx_bytes = Vec::new();
    dummy_tx.consensus_encode(&mut tx_bytes).context("tx encode failed")?;

    let parcel = MessageContextParcel {
        alkanes: vec![],
        transaction: tx_bytes,
        block: vec![],
        height: height as u64,
        txindex: 0,
        calldata,
        vout: 0,
        pointer: 0,
        refund_pointer: 0,
    };

    let mut parcel_bytes = Vec::new();
    parcel.encode(&mut parcel_bytes).context("parcel encode failed")?;

    let body = json!({
        "jsonrpc": "2.0",
        "id": format!("simulate:{}:{}", alkane.block, alkane.tx),
        "method": "metashrew_view",
        "params": [
            "simulate",
            format!("0x{}", hex::encode(parcel_bytes)),
            height as u64,
        ],
    });

    let client = Client::new();
    let resp_json: serde_json::Value = client
        .post(get_metashrew_rpc_url())
        .json(&body)
        .send()
        .await
        .context("metashrew request failed")?
        .error_for_status()
        .context("metashrew http error")?
        .json()
        .await
        .context("metashrew response decode failed")?;

    let result_hex = resp_json.get("result").and_then(|v| v.as_str()).unwrap_or("");
    if result_hex.is_empty() {
        return Ok(None);
    }
    let result_hex = result_hex.strip_prefix("0x").unwrap_or(result_hex);
    let bytes = hex::decode(result_hex).context("simulate result decode failed")?;
    let sim = SimulateProto::decode(bytes.as_slice()).context("simulate response decode failed")?;
    if !sim.error.is_empty() {
        return Ok(None);
    }
    if let Some(exec) = sim.execution {
        return Ok(decode_name_bytes(&exec.data));
    }
    Ok(None)
}

fn block_on_result<F, T>(fut: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    match Handle::try_current() {
        Ok(handle) => block_in_place(|| handle.block_on(fut)),
        Err(_) => {
            let rt = Runtime::new().context("failed to build ad-hoc Tokio runtime")?;
            rt.block_on(fut)
        }
    }
}
