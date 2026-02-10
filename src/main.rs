// Module declarations - these reference lib.rs modules indirectly
#[cfg(not(target_arch = "wasm32"))]
pub mod alkanes;
#[cfg(not(target_arch = "wasm32"))]
pub mod bitcoind_flexible;
#[cfg(not(target_arch = "wasm32"))]
pub mod config;
#[cfg(not(target_arch = "wasm32"))]
pub mod consts;
#[cfg(not(target_arch = "wasm32"))]
pub mod core;
#[cfg(not(target_arch = "wasm32"))]
pub mod debug;
#[cfg(not(target_arch = "wasm32"))]
pub mod explorer;
#[cfg(not(target_arch = "wasm32"))]
pub mod modules;
#[cfg(not(target_arch = "wasm32"))]
pub mod runtime;
#[cfg(not(target_arch = "wasm32"))]
pub mod schemas;
#[cfg(not(target_arch = "wasm32"))]
pub mod utils;

use std::net::SocketAddr;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

use crate::config::{DebugBackupConfig, init_block_source};
//modules
use crate::config::get_metashrew_sdb;
use crate::config::get_network;
use crate::modules::ammdata::main::AmmData;
use crate::modules::essentials::main::Essentials;
use crate::modules::essentials::storage::preload_block_summary_cache;
use crate::modules::oylapi::main::OylApi;
use crate::modules::pizzafun::main::Pizzafun;
use crate::modules::subfrost::main::Subfrost;
use crate::utils::{EtaTracker, fmt_duration};
use anyhow::{Context, Result};

use crate::explorer::run_explorer;
use crate::{
    alkanes::{trace::get_espo_block, utils::get_safe_tip},
    config::{
        get_aof_manager, get_bitcoind_rpc_client, get_config, get_espo_db, get_module_config,
        init_config, update_safe_tip,
    },
    consts::alkanes_genesis_block,
    modules::defs::ModuleRegistry,
    runtime::aof::AOF_REORG_DEPTH,
    runtime::mdb::Mdb,
    runtime::mempool::{
        purge_confirmed_from_chain, purge_confirmed_txids, reset_mempool_store, run_mempool_service,
    },
    runtime::rpc::run_rpc,
};
use bitcoin::Txid;
use bitcoincore_rpc::RpcApi;
pub use espo::{ESPO_HEIGHT, SAFE_TIP};
use tokio::runtime::Builder as TokioBuilder;

fn should_watch_for_reorg(next_height: u32, safe_tip: u32) -> bool {
    safe_tip.saturating_sub(next_height) <= AOF_REORG_DEPTH
}

fn run_debug_backup(db_path: &str, backup: &DebugBackupConfig, block: u32) -> std::io::Result<()> {
    let db_root = Path::new(db_path);
    let backup_root = Path::new(&backup.dir);
    if backup_root.starts_with(db_root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "debug_backup.dir may not be inside db_path",
        ));
    }
    std::fs::create_dir_all(backup_root)?;
    let dest_dir = backup_root.join(format!("bkp-{block}"));
    eprintln!("[debug_backup] starting copy: '{}' -> '{}'", db_root.display(), dest_dir.display());
    let status = Command::new("cp").arg("-r").arg(db_root).arg(&dest_dir).status()?;
    if !status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("cp exited with status {status}"),
        ));
    }
    eprintln!("[debug_backup] finished copy to '{}'", dest_dir.display());
    Ok(())
}

fn check_and_handle_reorg(next_height: u32, safe_tip: u32) -> Option<u32> {
    let Some(aof) = get_aof_manager() else { return None };
    if !should_watch_for_reorg(next_height, safe_tip) {
        return None;
    }

    let logs = match aof.recent_blocks(AOF_REORG_DEPTH as usize) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[aof] failed to load recent blocks: {e:?}");
            return None;
        }
    };
    if logs.is_empty() {
        return None;
    }

    let rpc = get_bitcoind_rpc_client();
    let mut mismatch = false;
    for log in &logs {
        match rpc.get_block_hash(log.height as u64) {
            Ok(h) => {
                if h.to_string() != log.block_hash {
                    mismatch = true;
                    break;
                }
            }
            Err(e) => {
                eprintln!("[aof] failed to fetch block hash for {}: {e:?}", log.height);
                return None;
            }
        }
    }

    if !mismatch {
        return None;
    }

    let revert_count = logs.len().min(AOF_REORG_DEPTH as usize);
    eprintln!(
        "[aof] detected reorg near height {} (safe tip {}); reverting {} blocks",
        next_height, safe_tip, revert_count
    );

    if let Err(e) = aof.revert_last_blocks(revert_count) {
        eprintln!("[aof] rollback failed: {e:?}");
        return None;
    }

    if let Err(e) = reset_mempool_store() {
        eprintln!("[mempool] failed to reset store after reorg: {e:?}");
    }

    Some(next_height.saturating_sub(revert_count as u32))
}

fn run_safe_tip_hook(script: &str, next_height: u32, tip: u32) {
    let script = script.trim();
    if script.is_empty() {
        return;
    }
    let script = script.to_string();
    std::thread::spawn(move || {
        eprintln!("[safe_tip_hook] running (next_height={}, tip={}): {}", next_height, tip, script);
        match Command::new("sh").arg("-c").arg(&script).status() {
            Ok(status) => eprintln!("[safe_tip_hook] finished: {}", status),
            Err(e) => eprintln!("[safe_tip_hook] failed: {e:?}"),
        }
    });
}

async fn run_indexer_loop(
    mods: ModuleRegistry,
    start_height: u32,
    mut next_height: u32,
    network: bitcoin::Network,
    metashrew_sdb: std::sync::Arc<crate::runtime::sdb::SDB>,
    cfg: crate::config::AppConfig,
) {
    const POLL_INTERVAL: Duration = Duration::from_secs(5);
    let mut last_tip: Option<u32> = None;
    let mut mempool_started = false;
    let mut logged_start = false;
    let mut safe_tip_hook_ran = false;
    if cfg.reset_mempool_on_startup {
        if let Err(e) = reset_mempool_store() {
            eprintln!("[mempool] failed to reset store on startup: {e:?}");
        }
    }
    if let Err(e) = purge_confirmed_from_chain() {
        eprintln!("[mempool] failed to purge confirmed txs on startup: {e:?}");
    }

    // ETA tracker
    let mut eta = EtaTracker::new(3.0); // EMA smoothing factor (tweak if you want faster/slower adaptation)
    let mut debug_backup_remaining: std::collections::HashSet<u32> = cfg
        .debug_backup
        .as_ref()
        .map(|backup| backup.blocks.iter().copied().collect())
        .unwrap_or_default();

    loop {
        if let Err(e) = metashrew_sdb.catch_up_now() {
            eprintln!("[indexer] metashrew catch_up before tip fetch: {e:?}");
        }

        let tip = match get_safe_tip() {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[indexer] failed to fetch safe tip: {e:?}");
                tokio::time::sleep(POLL_INTERVAL).await;
                continue;
            }
        };
        update_safe_tip(tip);
        if let Some(prev_tip) = last_tip {
            if tip > prev_tip {
                if let Err(e) = metashrew_sdb.catch_up_now() {
                    eprintln!(
                        "[indexer] metashrew catch_up after new tip {} (prev {}) detected: {e:?}",
                        tip, prev_tip
                    );
                }
            }
        }
        last_tip = Some(tip);

        if next_height == start_height && !logged_start {
            let remaining = tip.saturating_sub(next_height) + 1;
            let eta_str = fmt_duration(eta.eta(remaining));
            eprintln!(
                "[indexer] starting at {}, safe tip {}, {} blocks behind, ETA ~ {}",
                next_height, tip, remaining, eta_str
            );
            logged_start = true;
        }

        if next_height <= tip {
            // Compute a fresh ETA before starting the block
            let remaining = tip.saturating_sub(next_height) + 1;
            let eta_str = fmt_duration(eta.eta(remaining));

            eprintln!(
                "[indexer] indexing block #{} ({} left → ETA ~ {})",
                next_height, remaining, eta_str
            );

            eta.start_block();

            if let Err(e) = metashrew_sdb.catch_up_now() {
                eprintln!(
                    "[indexer] metashrew catch_up before indexing block {}: {e:?}",
                    next_height
                );
            }

            match get_espo_block(next_height.into(), tip.into())
                .with_context(|| format!("failed to load espo block {next_height}"))
            {
                Ok(espo_block) => {
                    // (Optional) include hash or tx count here as you like
                    let block_txids: Vec<Txid> = espo_block
                        .transactions
                        .iter()
                        .map(|t| t.transaction.compute_txid())
                        .collect();

                    let block_hash = espo_block.block_header.block_hash();

                    // Only capture AOF changes when we're near the safe tip; skip during deep catch-up.
                    let aof_for_block =
                        get_aof_manager().filter(|_| should_watch_for_reorg(next_height, tip));
                    if let Some(aof) = &aof_for_block {
                        aof.start_block(next_height, &block_hash);
                    }

                    for m in mods.modules() {
                        if next_height >= m.get_genesis_block(network) {
                            if let Err(e) = m.index_block(espo_block.clone()) {
                                eprintln!(
                                    "[module:{}] height {}: {e:?}",
                                    m.get_name(),
                                    next_height
                                );
                            }
                        }
                    }

                    match purge_confirmed_txids(&block_txids) {
                        Ok(removed) => {
                            if removed > 0 {
                                eprintln!(
                                    "[mempool] removed {} confirmed txs at height {}",
                                    removed, next_height
                                );
                            }
                        }
                        Err(e) => eprintln!(
                            "[mempool] failed to purge confirmed txs at height {}: {e:?}",
                            next_height
                        ),
                    }

                    if let Some(aof) = &aof_for_block {
                        if let Err(e) = aof.finish_block() {
                            eprintln!(
                                "[aof] failed to persist block {} changes: {e:?}",
                                next_height
                            );
                        }
                    }

                    if let Some(backup) = cfg.debug_backup.as_ref() {
                        if debug_backup_remaining.remove(&next_height) {
                            eprintln!(
                                "[debug_backup] reached block {}, copying db dir '{}' to '{}/bkp-{}'",
                                next_height, cfg.db_path, backup.dir, next_height
                            );
                            match run_debug_backup(&cfg.db_path, backup, next_height) {
                                Ok(_) => eprintln!("[debug_backup] backup complete"),
                                Err(e) => eprintln!("[debug_backup] backup failed: {e}"),
                            }
                        }
                    }

                    eta.finish_block();
                    next_height = next_height.saturating_add(1);
                    if let Some(h) = ESPO_HEIGHT.get() {
                        h.store(next_height, std::sync::atomic::Ordering::Relaxed);
                    }
                    if cfg.indexer_block_delay_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(cfg.indexer_block_delay_ms)).await;
                    }
                }
                Err(e) => {
                    eprintln!("[indexer] error at height {}: {e:?}", next_height);
                    // Don’t update EMA on failure; just wait and retry
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            }
        } else {
            if !safe_tip_hook_ran {
                if let Some(script) = cfg.safe_tip_hook_script.as_deref() {
                    safe_tip_hook_ran = true;
                    run_safe_tip_hook(script, next_height, tip);
                }
            }
            if let Some(new_next) = check_and_handle_reorg(next_height, tip) {
                if new_next < next_height {
                    next_height = new_next;
                    if let Some(h) = ESPO_HEIGHT.get() {
                        h.store(next_height, std::sync::atomic::Ordering::Relaxed);
                    }
                    eprintln!("[aof] rollback complete, restarting from height {}", next_height);
                }
            }
            // Caught up; chill then poll again
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        if !mempool_started && next_height >= tip.saturating_sub(1) {
            mempool_started = true;
            let network_for_task = network;
            std::thread::spawn(move || {
                let rt = TokioBuilder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build mempool runtime");
                if let Err(e) = rt.block_on(run_mempool_service(network_for_task)) {
                    eprintln!("[mempool] service error: {e:?}");
                }
            });
            eprintln!(
                "[mempool] service started near safe tip (next_height={}, tip={})",
                next_height, tip
            );
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() -> Result<()> {
    init_config()?;
    let cfg = get_config().clone();
    let network = get_network();
    let view_only = cfg.view_only;
    init_block_source()?;

    if view_only {
        eprintln!(
            "[mode] view-only enabled: indexer and mempool are disabled; serving existing data only"
        );
    }
    let metashrew_sdb = get_metashrew_sdb();

    if cfg.simulate_reorg {
        if view_only {
            eprintln!("[aof] simulate-reorg ignored in view-only mode");
        } else {
            match get_aof_manager() {
                Some(aof) => match aof.revert_all_blocks() {
                    Ok(Some(h)) => {
                        eprintln!(
                            "[aof] simulate-reorg: reverted through height {}, will reindex",
                            h
                        );
                        if let Err(e) = reset_mempool_store() {
                            eprintln!(
                                "[mempool] failed to reset store after simulated reorg: {e:?}"
                            );
                        }
                    }
                    Ok(None) => eprintln!("[aof] simulate-reorg set but no AOF logs to revert"),
                    Err(e) => eprintln!("[aof] simulate-reorg failed: {e:?}"),
                },
                None => {
                    eprintln!("[aof] simulate-reorg set but AOF is disabled; nothing to revert")
                }
            }
        }
    }

    // Build module registry with the global ESPO DB
    let mut mods = ModuleRegistry::with_db_and_aof(get_espo_db(), get_aof_manager());
    // Essentials must run before any optional modules.
    mods.register_module(Essentials::new());
    mods.register_module(Pizzafun::new());
    if get_module_config("ammdata").is_some() {
        mods.register_module(AmmData::new());
    } else {
        eprintln!("[modules] ammdata disabled (missing config)");
    }
    if get_module_config("subfrost").is_some() {
        mods.register_module(Subfrost::new());
    } else {
        eprintln!("[modules] subfrost disabled (missing config)");
    }
    if get_module_config("oylapi").is_some() {
        mods.register_module(OylApi::new());
    } else {
        eprintln!("[modules] oylapi disabled (missing config)");
    }
    // mods.register_module(TracesData::new());

    let essentials_mdb = Mdb::from_db(get_espo_db(), b"essentials:");
    let loaded = preload_block_summary_cache(&essentials_mdb);
    if loaded > 0 {
        eprintln!("[cache] preloaded {} block summaries", loaded);
    }

    // Start RPC server
    let addr: SocketAddr = SocketAddr::from(([0, 0, 0, 0], cfg.port));
    let rpc_router = mods.router.clone();
    tokio::spawn(async move {
        if let Err(e) = run_rpc(rpc_router, addr).await {
            eprintln!("[rpc] server error: {e:?}");
        }
    });
    eprintln!("[rpc] listening on {}", addr);

    // Optional SSR explorer server
    if let Some(explorer_addr) = cfg.explorer_host {
        let explorer_handle = tokio::spawn(async move {
            if let Err(e) = run_explorer(explorer_addr).await {
                eprintln!("[explorer] server error: {e:?}");
            }
        });
        tokio::spawn(async move {
            if let Err(err) = explorer_handle.await {
                eprintln!("[explorer] task panicked: {err:?}");
                std::process::abort();
            }
        });
        eprintln!("[explorer] listening on {}", explorer_addr);
    }

    let global_genesis = alkanes_genesis_block(network);

    // Decide initial start height (resume at last+1 per module)
    let start_height = mods
        .modules()
        .iter()
        .map(|m| {
            let g = m.get_genesis_block(network);
            match m.get_index_height() {
                Some(h) => h.saturating_add(1).max(g),
                None => g,
            }
        })
        .min()
        .unwrap_or(global_genesis)
        .max(global_genesis);

    let height_cell = Arc::new(AtomicU32::new(start_height));

    ESPO_HEIGHT
        .set(height_cell.clone())
        .map_err(|_| anyhow::anyhow!("espo height client already initialized"))?;
    let next_height: u32 = start_height;

    if view_only {
        let indexed_height = start_height.saturating_sub(1);
        update_safe_tip(indexed_height);
        eprintln!(
            "[mode] view-only: explorer/RPC running; indexed height {}, next height {}",
            indexed_height, start_height
        );
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    }

    let indexer_handle = std::thread::spawn(move || {
        let rt = TokioBuilder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build indexer runtime");
        rt.block_on(run_indexer_loop(mods, start_height, next_height, network, metashrew_sdb, cfg));
    });
    std::thread::spawn(move || {
        if let Err(err) = indexer_handle.join() {
            eprintln!("[indexer] thread panicked: {err:?}");
            std::process::abort();
        }
    });

    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

// Dummy main for WASM builds (should never be called)
#[cfg(target_arch = "wasm32")]
fn main() {
    panic!("ESPO binary cannot be compiled for WASM");
}
