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

fn has_method(inspection: &StoredInspectionResult, name: &str, opcode: u128) -> bool {
    inspection
        .metadata
        .as_ref()
        .map(|meta| meta.methods.iter().any(|m| m.name == name && m.opcode == opcode))
        .unwrap_or(false)
}

fn decode_u128_bytes(bytes: &[u8]) -> Option<u128> {
    if bytes.is_empty() {
        return None;
    }
    let mut buf = [0u8; 16];
    if bytes.len() >= 16 {
        buf.copy_from_slice(&bytes[..16]);
    } else {
        buf[..bytes.len()].copy_from_slice(bytes);
    }
    Some(u128::from_le_bytes(buf))
}

pub fn get_cap(
    height: u32,
    alkane: &SchemaAlkaneId,
    inspection: Option<&StoredInspectionResult>,
) -> Option<u128> {
    let Some(inspection) = inspection else { return None };
    if !has_method(inspection, "get_cap", 102) {
        return None;
    }
    match simulate_get_u128(alkane, height, 102) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "[ESSENTIALS] get_cap simulate failed for {}:{}: {e}",
                alkane.block, alkane.tx
            );
            None
        }
    }
}

pub fn get_value_per_mint(
    height: u32,
    alkane: &SchemaAlkaneId,
    inspection: Option<&StoredInspectionResult>,
) -> Option<u128> {
    let Some(inspection) = inspection else { return None };
    if !has_method(inspection, "get_value_per_mint", 104) {
        return None;
    }
    match simulate_get_u128(alkane, height, 104) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "[ESSENTIALS] get_value_per_mint simulate failed for {}:{}: {e}",
                alkane.block, alkane.tx
            );
            None
        }
    }
}

fn simulate_get_u128(alkane: &SchemaAlkaneId, height: u32, opcode: u128) -> Result<Option<u128>> {
    block_on_result(simulate_get_u128_async(alkane, height, opcode))
}

async fn simulate_get_u128_async(
    alkane: &SchemaAlkaneId,
    height: u32,
    opcode: u128,
) -> Result<Option<u128>> {
    let cellpack = Cellpack {
        target: AlkaneId { block: alkane.block as u128, tx: alkane.tx as u128 },
        inputs: vec![opcode],
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
        return Ok(decode_u128_bytes(&exec.data));
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
