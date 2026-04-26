use anyhow::{Context, Result};
use bitcoin::Transaction;
use clap::Parser;
use espo::alkanes::trace::PartialEspoTrace;
use espo::config::{
    get_block_source, get_espo_db, get_metashrew, get_metashrew_sdb, init_config_from_read_only,
    load_config_from_path,
};
use espo::core::blockfetcher::BlockSource;
use espo::modules::essentials::storage::{EssentialsProvider, GetIndexHeightParams};
use espo::runtime::mdb::Mdb;
use espo::runtime::state_at::StateAt;
use ordinals::{Artifact, Runestone};
use protorune_support::protostone::Protostone;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(
    name = "trace_mismatch_check",
    about = "Check recent metashrew trace/block txid consistency"
)]
struct Args {
    #[arg(long, default_value = "./config.json")]
    config_path: String,

    #[arg(long)]
    start_height: Option<u32>,

    #[arg(long)]
    end_height: Option<u32>,

    #[arg(long, default_value_t = 1000)]
    window: u32,

    #[arg(long, default_value_t = 50)]
    log_every: u32,

    #[arg(long, default_value_t = 8)]
    sample_limit: usize,
}

#[derive(Debug, Default)]
struct TraceIndex {
    display_txids: HashSet<String>,
    native_txids: HashSet<String>,
    display_by_native: HashMap<String, String>,
    bad_outpoints: usize,
}

#[derive(Debug, Default)]
struct BlockReport {
    height: u32,
    trace_count: usize,
    bad_outpoints: usize,
    recovered_missing_txids: Vec<String>,
    missing_candidate_txids: Vec<String>,
    unexpected_height_trace_txids: Vec<String>,
    native_endian_trace_txids: Vec<String>,
}

impl BlockReport {
    fn has_mismatch(&self) -> bool {
        self.bad_outpoints > 0
            || !self.recovered_missing_txids.is_empty()
            || !self.missing_candidate_txids.is_empty()
            || !self.unexpected_height_trace_txids.is_empty()
            || !self.native_endian_trace_txids.is_empty()
    }
}

fn tx_has_alkanes_protocol(tx: &Transaction) -> bool {
    let Some(Artifact::Runestone(ref runestone)) = Runestone::decipher(tx) else {
        return false;
    };
    let Ok(protostones) = Protostone::from_runestone(runestone) else {
        return false;
    };
    protostones
        .iter()
        .any(|protostone| protostone.protocol_tag == 1 && !protostone.message.is_empty())
}

fn trace_index(partials: &[PartialEspoTrace]) -> TraceIndex {
    let mut out = TraceIndex::default();
    for partial in partials {
        if partial.outpoint.len() < 36 {
            out.bad_outpoints = out.bad_outpoints.saturating_add(1);
            continue;
        }
        let txid_le = &partial.outpoint[..32];
        let native_hex = hex::encode(txid_le);
        let mut display_bytes = txid_le.to_vec();
        display_bytes.reverse();
        let display_hex = hex::encode(display_bytes);
        out.native_txids.insert(native_hex.clone());
        out.display_txids.insert(display_hex.clone());
        out.display_by_native.insert(native_hex, display_hex);
    }
    out
}

fn sort_dedup(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}

fn sample(values: &[String], limit: usize) -> String {
    if values.is_empty() {
        return "-".to_string();
    }
    let mut out = values.iter().take(limit).cloned().collect::<Vec<_>>();
    if values.len() > limit {
        out.push(format!("...+{}", values.len() - limit));
    }
    out.join(",")
}

fn read_index_tip() -> Result<u32> {
    let mdb = Arc::new(Mdb::from_db(get_espo_db(), b"essentials:"));
    let provider = EssentialsProvider::new(mdb);
    provider
        .get_index_height(GetIndexHeightParams { blockhash: StateAt::Latest })?
        .height
        .context("missing essentials:/index_height")
}

fn main() -> Result<()> {
    let args = Args::parse();
    let cfg = load_config_from_path(&args.config_path, true)?;
    init_config_from_read_only(cfg)?;

    let tip = read_index_tip()?;
    let end_height = args.end_height.unwrap_or(tip).min(tip);
    let start_height = args
        .start_height
        .unwrap_or_else(|| end_height.saturating_sub(args.window.saturating_sub(1)));
    if start_height > end_height {
        anyhow::bail!("empty range: start_height={start_height} end_height={end_height}");
    }

    let block_source = get_block_source();
    let metashrew = get_metashrew();
    let metashrew_sdb = get_metashrew_sdb();
    let total = end_height.saturating_sub(start_height).saturating_add(1);
    let started = Instant::now();
    let mut mismatches: Vec<BlockReport> = Vec::new();

    eprintln!(
        "[trace-mismatch-check] tip={} range={}..={} total={} sample_limit={}",
        tip, start_height, end_height, total, args.sample_limit
    );

    for height in start_height..=end_height {
        let block = block_source
            .get_block_result_by_height(height, tip)
            .with_context(|| format!("fetch block {height}"))?
            .block;

        let mut canonical_txids: HashSet<String> = HashSet::with_capacity(block.txdata.len());
        let mut candidate_txids: Vec<String> = Vec::new();
        for tx in &block.txdata {
            let txid = tx.compute_txid().to_string();
            canonical_txids.insert(txid.clone());
            if tx_has_alkanes_protocol(tx) {
                candidate_txids.push(txid);
            }
        }
        sort_dedup(&mut candidate_txids);

        let height_partials = metashrew
            .traces_for_block_as_prost_with_db(metashrew_sdb.as_ref(), height as u64)
            .with_context(|| format!("fetch metashrew traces for block {height}"))?;
        let height_index = trace_index(&height_partials);

        let mut unexpected_height_trace_txids = Vec::new();
        let mut native_endian_trace_txids = Vec::new();
        for display_txid in &height_index.display_txids {
            if !canonical_txids.contains(display_txid) {
                unexpected_height_trace_txids.push(display_txid.clone());
            }
        }
        for native_txid in &height_index.native_txids {
            if canonical_txids.contains(native_txid) {
                native_endian_trace_txids.push(native_txid.clone());
            }
        }
        sort_dedup(&mut unexpected_height_trace_txids);
        sort_dedup(&mut native_endian_trace_txids);

        let mut recovered_missing_txids = Vec::new();
        let mut missing_candidate_txids = Vec::new();
        for txid in &candidate_txids {
            if height_index.display_txids.contains(txid) || height_index.native_txids.contains(txid)
            {
                continue;
            }
            let parsed = txid.parse().with_context(|| format!("parse txid {txid}"))?;
            let fallback_partials = metashrew
                .traces_for_tx_with_db(metashrew_sdb.as_ref(), &parsed)
                .with_context(|| format!("fetch metashrew traces for tx {txid}"))?;
            let fallback_index = trace_index(&fallback_partials);
            if fallback_index.display_txids.contains(txid)
                || fallback_index.native_txids.contains(txid)
            {
                recovered_missing_txids.push(txid.clone());
            }
        }
        sort_dedup(&mut recovered_missing_txids);
        sort_dedup(&mut missing_candidate_txids);

        let report = BlockReport {
            height,
            trace_count: height_partials.len(),
            bad_outpoints: height_index.bad_outpoints,
            recovered_missing_txids,
            missing_candidate_txids,
            unexpected_height_trace_txids,
            native_endian_trace_txids,
        };

        if report.has_mismatch() {
            eprintln!(
                "[trace-mismatch-check] mismatch height={} traces={} bad_outpoints={} recovered={} missing={} unexpected={} native_endian={}",
                report.height,
                report.trace_count,
                report.bad_outpoints,
                sample(&report.recovered_missing_txids, args.sample_limit),
                sample(&report.missing_candidate_txids, args.sample_limit),
                sample(&report.unexpected_height_trace_txids, args.sample_limit),
                sample(&report.native_endian_trace_txids, args.sample_limit)
            );
            mismatches.push(report);
        } else if args.log_every > 0
            && (height == start_height
                || height == end_height
                || (height - start_height) % args.log_every == 0)
        {
            eprintln!(
                "[trace-mismatch-check] ok height={} traces={} elapsed={:.1}s",
                height,
                height_partials.len(),
                started.elapsed().as_secs_f64()
            );
        }
    }

    if !mismatches.is_empty() {
        anyhow::bail!(
            "trace mismatch check failed: mismatched_blocks={} range={}..={}",
            mismatches.len(),
            start_height,
            end_height
        );
    }

    println!(
        "trace mismatch check passed: range={}..={} blocks={} elapsed={:.1}s",
        start_height,
        end_height,
        total,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}
