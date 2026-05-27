use crate::config::get_bitcoind_rpc_client;
use anyhow::{Result, anyhow};
use bitcoin::BlockHash;
use bitcoincore_rpc::RpcApi;
use serde::Deserialize;
use serde_json::json;

#[derive(Clone, Copy, Debug)]
pub struct BlockFeeRateSummary {
    pub avg: f64,
    pub median: f64,
    pub range: [f64; 7],
}

#[derive(Clone, Copy, Debug)]
pub struct FeeRateEntry {
    pub weight: f64,
    pub rate: f64,
}

/// Verbosity 2 includes every tx with hex, weight, and fee. Verbosity 3 adds
/// prevouts, which are much larger and are not used for fee ranges.
pub const BLOCK_FEE_RPC_VERBOSITY: u8 = 2;

#[derive(Deserialize)]
struct VerboseBlockTxsForFees {
    tx: Vec<VerboseBlockTxForFees>,
}

#[derive(Deserialize)]
struct VerboseBlockTxForFees {
    weight: u64,
    fee: Option<f64>,
}

pub fn fee_rate_entry_from_weight_and_btc_fee(
    weight: u64,
    fee_btc: Option<f64>,
) -> Option<FeeRateEntry> {
    let weight = weight as f64;
    if weight <= 0.0 {
        return None;
    }
    let fee_sat = fee_btc? * 100_000_000.0;
    let rate = fee_sat / (weight / 4.0);
    Some(FeeRateEntry { weight, rate })
}

pub fn compute_block_fee_rate_summary(blockhash: &BlockHash) -> Result<BlockFeeRateSummary> {
    let rpc = get_bitcoind_rpc_client();
    let block: VerboseBlockTxsForFees = rpc
        .call("getblock", &[json!(blockhash.to_string()), json!(BLOCK_FEE_RPC_VERBOSITY)])
        .map_err(|e| {
            anyhow!("bitcoind getblock({blockhash}, {BLOCK_FEE_RPC_VERBOSITY}) failed: {e}")
        })?;
    Ok(compute_fee_rate_summary(
        block
            .tx
            .into_iter()
            .filter_map(|tx| fee_rate_entry_from_weight_and_btc_fee(tx.weight, tx.fee))
            .collect(),
    ))
}

pub fn compute_fee_rate_summary(txs: Vec<FeeRateEntry>) -> BlockFeeRateSummary {
    if txs.is_empty() {
        return BlockFeeRateSummary { avg: 0.0, median: 0.0, range: [0.0; 7] };
    }

    let total_weight: f64 = txs.iter().map(|tx| tx.weight).sum();
    let total_fee_sat: f64 = txs.iter().map(|tx| tx.rate * (tx.weight / 4.0)).sum();
    let avg = if total_weight > 0.0 { total_fee_sat / (total_weight / 4.0) } else { 0.0 };

    let mut sorted_txs = txs.clone();
    sorted_txs.sort_by(|a, b| a.rate.total_cmp(&b.rate));
    let percentile_rate = |n: usize| -> f64 {
        if sorted_txs.is_empty() {
            return 0.0;
        }
        let idx = (((sorted_txs.len() - 1) as f64) * (n as f64 / 100.0)).floor() as usize;
        sorted_txs[idx].rate
    };

    let p10 = percentile_rate(10);
    let p25 = percentile_rate(25);
    let p50 = percentile_rate(50);
    let p75 = percentile_rate(75);
    let p90 = percentile_rate(90);
    let p99 = percentile_rate(99);

    let min_fee = txs.last().map(|tx| tx.rate).unwrap_or(0.0);

    let head_len = txs.len() / 50;
    let max_fee = txs.iter().take(head_len).map(|tx| tx.rate).fold(p99, f64::max);

    let half_width = (total_weight / 800.0).max(1.0);
    let left_bound = (total_weight / 2.0 - half_width).floor().max(0.0);
    let right_bound = (total_weight / 2.0 + half_width).ceil().min(total_weight);
    let mut weight_count = 0.0;
    let mut median_fee = 0.0;
    let mut median_weight = 0.0;
    for tx in &sorted_txs {
        if weight_count >= right_bound {
            break;
        }
        let left = weight_count;
        let right = weight_count + tx.weight;
        if right > left_bound {
            let overlap = right.min(right_bound) - left.max(left_bound);
            if overlap > 0.0 {
                median_fee += tx.rate * (overlap / 4.0);
                median_weight += overlap;
            }
        }
        weight_count += tx.weight;
    }
    let median = if median_weight > 0.0 { median_fee / (median_weight / 4.0) } else { p50 };

    BlockFeeRateSummary { avg, median, range: [min_fee, p10, p25, p50, p75, p90, max_fee] }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(rate: f64) -> FeeRateEntry {
        FeeRateEntry { weight: 400.0, rate }
    }

    #[test]
    fn fee_rate_entry_keeps_zero_fee_transactions() {
        let entry = fee_rate_entry_from_weight_and_btc_fee(617, Some(0.0))
            .expect("zero-fee non-coinbase transaction should be included");

        assert_eq!(entry.rate, 0.0);
    }

    #[test]
    fn fee_rate_range_uses_last_transaction_as_lower_bound() {
        let mut txs: Vec<FeeRateEntry> =
            (0..3_400).map(|idx| entry(10.0 - (idx as f64 / 400.0))).collect();
        txs[1_778] = entry(0.102_376_6);
        txs[2_000] = entry(0.883_720_93);
        txs.push(entry(1.006_681_51));

        let summary = compute_fee_rate_summary(txs);

        assert_eq!(summary.range[0], 1.006_681_51);
    }

    #[test]
    fn fee_rate_median_uses_actual_transaction_weight() {
        let txs = vec![entry(1.0), entry(2.0), entry(3.0)];

        let summary = compute_fee_rate_summary(txs);

        assert_eq!(summary.median, 2.0);
    }
}
