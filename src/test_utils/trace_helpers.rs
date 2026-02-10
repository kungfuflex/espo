use crate::alkanes::trace::{
    EspoAlkanesTransaction, EspoBlock, EspoSandshrewLikeTrace, EspoSandshrewLikeTraceEvent,
    EspoTrace, PartialEspoTrace, extract_alkane_storage, prettyify_protobuf_trace_json,
};
use crate::schemas::EspoOutpoint;
/// Helpers for extracting and working with traces in tests
///
/// This module provides utilities to bridge metashrew traces with ESPO structures
/// for testing purposes.
use anyhow::{Context, Result};
use bitcoin::consensus::Encodable;
use bitcoin::hashes::Hash;
use bitcoin::{Block, Transaction};
use std::collections::HashMap;

/// Build EspoBlock from a Bitcoin block and metashrew traces
///
/// This function takes:
/// - A Bitcoin block (with transactions)
/// - A vector of PartialEspoTrace from metashrew
///
/// And produces an EspoBlock that can be passed to ESPO modules for indexing.
///
/// # Example
///
/// ```no_run
/// use espo::test_utils::trace_helpers::build_espo_block;
/// use espo::alkanes::trace::PartialEspoTrace;
///
/// // After indexing block through metashrew:
/// let traces = extract_traces_for_block(height)?;
/// let espo_block = build_espo_block(height, &bitcoin_block, traces)?;
///
/// // Now you can pass to ammdata:
/// ammdata.index_block(espo_block)?;
/// ```
pub fn build_espo_block(
    height: u32,
    block: &Block,
    traces: Vec<PartialEspoTrace>,
) -> Result<EspoBlock> {
    // Build host function values (required by EspoBlock)
    let host_function_values = build_host_function_values(block)?;

    // Group traces by transaction ID
    let traces_by_txid = group_traces_by_txid(traces)?;

    // Build EspoAlkanesTransaction for each tx in the block
    let transactions = build_espo_transactions(&block.txdata, traces_by_txid)?;

    Ok(EspoBlock {
        is_latest: true,
        height,
        block_header: block.header,
        host_function_values,
        tx_count: block.txdata.len(),
        transactions,
    })
}

/// Build host function values from a block
fn build_host_function_values(block: &Block) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>)> {
    let mut header_bytes = Vec::new();
    block
        .header
        .consensus_encode(&mut header_bytes)
        .context("Failed to encode block header")?;

    let coinbase_tx = block.txdata.first().cloned().context("Block has no coinbase transaction")?;

    let mut coinbase_bytes = Vec::new();
    coinbase_tx
        .consensus_encode(&mut coinbase_bytes)
        .context("Failed to encode coinbase transaction")?;

    let total_fees: u128 = coinbase_tx.output.iter().map(|out| out.value.to_sat() as u128).sum();
    let total_fees_bytes = total_fees.to_le_bytes().to_vec();

    // TODO: Calculate actual diesel mints if needed
    let diesel_mints_bytes = 0u128.to_le_bytes().to_vec();

    Ok((header_bytes, coinbase_bytes, diesel_mints_bytes, total_fees_bytes))
}

/// Group traces by transaction ID
fn group_traces_by_txid(
    traces: Vec<PartialEspoTrace>,
) -> Result<HashMap<String, Vec<(u32, PartialEspoTrace)>>> {
    let mut traces_by_txid: HashMap<String, Vec<(u32, PartialEspoTrace)>> = HashMap::new();

    for partial in traces {
        if partial.outpoint.len() < 36 {
            continue;
        }

        let (tx_bytes, vout_le) = partial.outpoint.split_at(32);
        let vout = u32::from_le_bytes(
            vout_le[..4].try_into().context("Failed to parse vout from outpoint")?,
        );

        // Convert txid to hex (little-endian)
        let mut tx_bytes_rev = tx_bytes.to_vec();
        tx_bytes_rev.reverse();
        let txid_hex = hex::encode(&tx_bytes_rev);

        traces_by_txid.entry(txid_hex).or_default().push((vout, partial));
    }

    // Sort traces by vout within each transaction
    for traces in traces_by_txid.values_mut() {
        traces.sort_by_key(|(vout, _)| *vout);
    }

    Ok(traces_by_txid)
}

/// Build EspoAlkanesTransaction structures from Bitcoin transactions and traces
fn build_espo_transactions(
    txs: &[Transaction],
    traces_by_txid: HashMap<String, Vec<(u32, PartialEspoTrace)>>,
) -> Result<Vec<EspoAlkanesTransaction>> {
    let mut result = Vec::with_capacity(txs.len());

    for tx in txs {
        let txid = tx.compute_txid();
        let txid_hex = txid.to_string();

        let traces_opt = if let Some(vouts_partials) = traces_by_txid.get(&txid_hex) {
            Some(build_espo_traces(vouts_partials, tx, &txid)?)
        } else {
            None
        };

        result.push(EspoAlkanesTransaction { traces: traces_opt, transaction: tx.clone() });
    }

    Ok(result)
}

/// Build EspoTrace structures from partial traces
fn build_espo_traces(
    vouts_partials: &[(u32, PartialEspoTrace)],
    tx: &Transaction,
    txid: &bitcoin::Txid,
) -> Result<Vec<EspoTrace>> {
    let txid_hex = txid.to_string();
    let mut traces = Vec::with_capacity(vouts_partials.len());

    for (vout, partial) in vouts_partials {
        // Convert protobuf trace to sandshrew-like JSON trace
        let events_json_str = prettyify_protobuf_trace_json(&partial.protobuf_trace)
            .context("Failed to convert protobuf trace to JSON")?;

        let events: Vec<EspoSandshrewLikeTraceEvent> =
            serde_json::from_str(&events_json_str).context("Failed to parse trace events")?;

        let sandshrew_trace =
            EspoSandshrewLikeTrace { outpoint: format!("{}:{}", txid_hex, vout), events };

        // Extract storage changes from the trace
        let storage_changes = extract_alkane_storage(&partial.protobuf_trace, tx)
            .context("Failed to extract alkane storage")?;

        let outpoint =
            EspoOutpoint { txid: txid.as_byte_array().to_vec(), vout: *vout, tx_spent: None };

        traces.push(EspoTrace {
            sandshrew_trace,
            protobuf_trace: partial.protobuf_trace.clone(),
            storage_changes,
            outpoint,
        });
    }

    Ok(traces)
}

/// Mock trace builders for testing
///
/// Since alkanes.wasm computes traces on-demand via view functions (not stored in DB),
/// we manually construct traces for testing purposes.
use alkanes_cli_common::alkanes_pb::AlkanesTrace;
use alkanes_support::id::AlkaneId;

/// Helper to encode AlkaneId as 32-byte little-endian (for storage values)
pub fn encode_alkane_id_le(id: &AlkaneId) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(&id.block.to_le_bytes());
    bytes.extend_from_slice(&id.tx.to_le_bytes());
    bytes
}

/// Build a mock pool creation trace
///
/// Simulates what happens when a factory creates a pool:
/// - Trace events showing pool initialization
/// - Storage writes for /alkane/0, /alkane/1, /factory_id
///
/// Note: This is a simplified mock - real traces have more detail
pub fn build_pool_creation_trace(
    pool_id: AlkaneId,
    factory_id: AlkaneId,
    token0: AlkaneId,
    token1: AlkaneId,
    outpoint: Vec<u8>,
) -> PartialEspoTrace {
    // Create an empty trace for now - the key is having the outpoint
    // The ammdata module extracts pool info from the sandshrew_trace events
    let protobuf_trace = AlkanesTrace {
        events: vec![], // TODO: Add proper trace events
    };

    PartialEspoTrace { protobuf_trace, outpoint }
}

/// Build EspoBlock from Bitcoin block without traces
///
/// Convenience wrapper for blocks with no alkane activity
pub fn build_espo_block_from_traces(
    height: u32,
    block: &Block,
    traces: Vec<PartialEspoTrace>,
) -> Result<EspoBlock> {
    build_espo_block(height, block, traces)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_host_function_values() {
        let block = protorune::test_helpers::create_block_with_coinbase_tx(1);
        let result = build_host_function_values(&block);
        assert!(result.is_ok());

        let (header, coinbase, diesel, fees) = result.unwrap();
        assert!(!header.is_empty());
        assert!(!coinbase.is_empty());
        assert_eq!(diesel.len(), 16); // u128 bytes
        assert_eq!(fees.len(), 16); // u128 bytes
    }

    #[test]
    fn test_group_traces_by_txid_empty() {
        let result = group_traces_by_txid(vec![]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_build_espo_transactions_no_traces() {
        let block = protorune::test_helpers::create_block_with_coinbase_tx(1);
        let result = build_espo_transactions(&block.txdata, HashMap::new());
        assert!(result.is_ok());

        let txs = result.unwrap();
        assert_eq!(txs.len(), 1); // Just coinbase
        assert!(txs[0].traces.is_none());
    }
}
