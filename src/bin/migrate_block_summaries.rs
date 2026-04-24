use anyhow::{Context, Result};
use bitcoin::consensus::encode::deserialize;
use bitcoin::hashes::Hash;
use bitcoincore_rpc::RpcApi;
use clap::Parser;
use espo::config::{get_bitcoind_rpc_client, get_espo_db, init_config_from, load_config_from_path};
use espo::modules::essentials::storage::{
    BlockSummary, EssentialsProvider, EssentialsTable, compute_block_fee_rate_summary,
};
use espo::runtime::mdb::Mdb;
use std::sync::Arc;
use std::time::Instant;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long, default_value = "./config.json")]
    config_path: String,

    #[arg(long)]
    start_height: Option<u32>,

    #[arg(long)]
    end_height: Option<u32>,

    #[arg(long, default_value_t = 100)]
    log_every: u32,

    #[arg(long)]
    force: bool,
}

fn decode_u32_le(bytes: &[u8]) -> Option<u32> {
    if bytes.len() != 4 {
        return None;
    }
    let mut arr = [0u8; 4];
    arr.copy_from_slice(bytes);
    Some(u32::from_le_bytes(arr))
}

fn main() -> Result<()> {
    let args = Args::parse();
    let cfg = load_config_from_path(&args.config_path, true)?;
    init_config_from(cfg)?;

    let started = Instant::now();
    let mdb = Arc::new(Mdb::from_db(get_espo_db(), b"essentials:"));
    let provider = EssentialsProvider::new(mdb.clone());
    let table = EssentialsTable::new(mdb.as_ref());
    let rpc = get_bitcoind_rpc_client();

    let indexed_tip = mdb
        .get(table.INDEX_HEIGHT.key())
        .context("read /index_height")?
        .as_deref()
        .and_then(decode_u32_le)
        .context("missing or invalid /index_height")?;
    let start_height = args.start_height.unwrap_or(0);
    let end_height = args.end_height.unwrap_or(indexed_tip).min(indexed_tip);
    if start_height > end_height {
        eprintln!(
            "[block-summary-migration] empty range start={} end={} indexed_tip={}",
            start_height, end_height, indexed_tip
        );
        return Ok(());
    }

    let total = (end_height - start_height + 1) as u64;
    eprintln!(
        "[block-summary-migration] start config={} indexed_tip={} range={}..={} total={} force={} log_every={}",
        args.config_path, indexed_tip, start_height, end_height, total, args.force, args.log_every
    );

    let mut migrated = 0u64;
    let mut skipped_existing = 0u64;
    let mut skipped_missing_old = 0u64;
    let mut failed = 0u64;

    for height in start_height..=end_height {
        let pos = (height - start_height + 1) as u64;
        let blockhash = match rpc.get_block_hash(height as u64) {
            Ok(hash) => hash,
            Err(err) => {
                failed += 1;
                eprintln!(
                    "[block-summary-migration] height={} pos={}/{} failed getblockhash: {err}",
                    height, pos, total
                );
                continue;
            }
        };
        let summary_key = table.block_summary_by_hash_key(&blockhash);
        let existing_raw = provider
            .get_blob_raw_value(espo::modules::essentials::storage::GetRawValueParams {
                blockhash: espo::runtime::state_at::StateAt::Latest,
                key: summary_key,
            })?
            .value;
        if !args.force && existing_raw.is_some() {
            skipped_existing += 1;
            if args.log_every > 0 && (pos == 1 || pos % args.log_every as u64 == 0 || pos == total)
            {
                log_progress(
                    "skip-existing",
                    height,
                    pos,
                    total,
                    migrated,
                    skipped_existing,
                    skipped_missing_old,
                    failed,
                    started,
                );
            }
            continue;
        }

        let source_summary =
            existing_raw.as_ref().and_then(|raw| BlockSummary::decode(raw)).or_else(|| {
                mdb.get(&table.block_summary_key(height))
                    .ok()
                    .flatten()
                    .and_then(|raw| BlockSummary::decode(&raw))
            });
        let Some(old_summary) = source_summary else {
            skipped_missing_old += 1;
            eprintln!(
                "[block-summary-migration] height={} pos={}/{} missing/undecodable old or hash-keyed summary",
                height, pos, total
            );
            continue;
        };

        let fee_summary = match compute_block_fee_rate_summary(&blockhash) {
            Ok(stats) => stats,
            Err(err) => {
                failed += 1;
                eprintln!(
                    "[block-summary-migration] height={} hash={} pos={}/{} failed fee summary: {err:#}",
                    height, blockhash, pos, total
                );
                continue;
            }
        };

        let tx_count = if old_summary.tx_count > 0 {
            old_summary.tx_count
        } else {
            rpc.get_block_header_info(&blockhash)
                .ok()
                .map(|hdr| hdr.n_tx as u32)
                .unwrap_or(0)
        };
        let header = if old_summary.header.is_empty() {
            rpc.get_block_header(&blockhash)
                .map(|hdr| {
                    let mut bytes = Vec::new();
                    let _ = bitcoin::consensus::Encodable::consensus_encode(&hdr, &mut bytes);
                    bytes
                })
                .unwrap_or_default()
        } else {
            old_summary.header
        };
        if deserialize::<bitcoin::blockdata::block::Header>(&header).is_err() {
            eprintln!(
                "[block-summary-migration] height={} hash={} warning: stored header failed decode",
                height, blockhash
            );
        }

        let new_summary = BlockSummary {
            height,
            blockhash: blockhash.to_byte_array(),
            trace_count: old_summary.trace_count,
            tx_count,
            header,
            fee_avg: fee_summary.avg,
            fee_median: fee_summary.median,
            fee_range: fee_summary.range.to_vec(),
        };

        if let Err(err) = provider.put_block_summary_indexes(&new_summary) {
            failed += 1;
            eprintln!(
                "[block-summary-migration] height={} hash={} pos={}/{} failed write: {err:#}",
                height, blockhash, pos, total
            );
            continue;
        }
        migrated += 1;

        if args.log_every > 0 && (pos == 1 || pos % args.log_every as u64 == 0 || pos == total) {
            eprintln!(
                "[block-summary-migration] indexed height={} hash={} txs={} traces={} avg={:.4} median={:.4} range=[{:.4}, {:.4}, {:.4}, {:.4}, {:.4}, {:.4}, {:.4}]",
                height,
                blockhash,
                new_summary.tx_count,
                new_summary.trace_count,
                new_summary.fee_avg,
                new_summary.fee_median,
                new_summary.fee_range.get(0).copied().unwrap_or(0.0),
                new_summary.fee_range.get(1).copied().unwrap_or(0.0),
                new_summary.fee_range.get(2).copied().unwrap_or(0.0),
                new_summary.fee_range.get(3).copied().unwrap_or(0.0),
                new_summary.fee_range.get(4).copied().unwrap_or(0.0),
                new_summary.fee_range.get(5).copied().unwrap_or(0.0),
                new_summary.fee_range.get(6).copied().unwrap_or(0.0),
            );
            log_progress(
                "progress",
                height,
                pos,
                total,
                migrated,
                skipped_existing,
                skipped_missing_old,
                failed,
                started,
            );
        }
    }

    log_progress(
        "done",
        end_height,
        total,
        total,
        migrated,
        skipped_existing,
        skipped_missing_old,
        failed,
        started,
    );
    Ok(())
}

fn log_progress(
    label: &str,
    height: u32,
    pos: u64,
    total: u64,
    migrated: u64,
    skipped_existing: u64,
    skipped_missing_old: u64,
    failed: u64,
    started: Instant,
) {
    let elapsed = started.elapsed().as_secs_f64();
    let rate = if elapsed > 0.0 { pos as f64 / elapsed } else { 0.0 };
    let remaining = total.saturating_sub(pos);
    let eta = if rate > 0.0 { remaining as f64 / rate } else { 0.0 };
    eprintln!(
        "[block-summary-migration] {} height={} pos={}/{} migrated={} skipped_existing={} skipped_missing_old={} failed={} elapsed={:.1}s rate={:.2}/s eta={:.1}s",
        label,
        height,
        pos,
        total,
        migrated,
        skipped_existing,
        skipped_missing_old,
        failed,
        elapsed,
        rate,
        eta
    );
}
