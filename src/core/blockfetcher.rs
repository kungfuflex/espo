// blockfetcher.rs
use crate::bitcoind_flexible::FlexibleBitcoindClient as CoreClient;
use anyhow::{Context, Result, anyhow};
use bitcoincore_rpc::RpcApi;
use bitcoincore_rpc::bitcoin::hashes::Hash; // for to_byte_array()
use bitcoincore_rpc::bitcoin::{
    Block, BlockHash, CompactTarget, Network, Transaction, TxMerkleNode, block, consensus,
};
use borsh::{BorshDeserialize, BorshSerialize};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Mutex;
use std::time::Instant;

use crate::config::{get_bitcoind_rpc_client, get_espo_db};
use crate::consts::alkanes_genesis_block;
use crate::runtime::mdb::Mdb;
use crate::utils::fee_rates::{
    BLOCK_FEE_RPC_VERBOSITY, BlockFeeRateSummary, compute_fee_rate_summary,
    fee_rate_entry_from_weight_and_btc_fee,
};

/// === Tuning ==================================================================
/// Max expected payload size from blk header (sanity).
const MAX_BLOCK_PAYLOAD: u32 = 8_000_000;
/// If height is within this distance from tip, fetch via RPC (avoid file tail races).
const NEAR_TIP_RPC_THRESHOLD: u32 = 6_000;
/// ============================================================================

/// Public trait: source of blocks for a given height.
pub trait BlockSource {
    /// Returns the full block for `height`. `tip` is used to optionally route near-tip to RPC.
    fn get_block_by_height(&self, height: u32, tip: u32) -> Result<Block>;

    fn get_block_result_by_height(&self, height: u32, tip: u32) -> Result<BlockFetchResult> {
        Ok(BlockFetchResult { block: self.get_block_by_height(height, tip)?, fee_summary: None })
    }
}

pub struct BlockFetchResult {
    pub block: Block,
    pub fee_summary: Option<BlockFeeRateSummary>,
}

#[derive(Deserialize)]
struct VerboseRpcBlock {
    version: i32,
    previousblockhash: Option<String>,
    merkleroot: String,
    time: u32,
    bits: String,
    nonce: u32,
    tx: Vec<VerboseRpcTx>,
}

#[derive(Deserialize)]
struct VerboseRpcTx {
    hex: String,
    weight: u64,
    fee: Option<f64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockFetchMode {
    /// Existing behaviour: use blk files when indexed, fall back to RPC near tip or if missing.
    Auto,
    /// Always fetch via RPC (skip blk files entirely). Useful when local blk files are stale/reorged.
    RpcOnly,
    /// Only use blk files for block bodies; RPC is still used for headers/height lookups.
    BlkOnly,
}

/// Borsh-encoded value stored in Mdb for each block hash.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct BlockFileLocationDescriptor {
    /// blk file number (e.g. 5159 for blk05159.dat)
    pub file_no: u32,
    /// byte offset of the *payload* (right after the 8B [magic|len] record header)
    pub offset: u64,
    /// payload length in bytes from the record header
    pub len: u32,
    /// cached tx count (handy; not required to load the block)
    pub txs: u32,
}

/// In-memory decoded cache for exactly ONE blk file.
#[derive(Default)]
struct DecodedFileCache {
    file_no: Option<u32>,
    // All decoded blocks from that file (ACTIVE-CHAIN ONLY — verified via RPC):
    blocks: HashMap<BlockHash, Block>,
}

/// Main implementation: uses Mdb-backed index of blk files, falling back to Core RPC.
pub struct BlkOrRpcBlockSource {
    mdb: Mdb,
    blocks_dir: PathBuf,
    network: Network,
    rpc: &'static CoreClient, // borrow the global RPC client from config
    mode: BlockFetchMode,

    /// Stop indexing once this hash is known in the index (genesis of Alkanes range)
    genesis_stop_hash: Option<BlockHash>,

    /// Decoded-block cache for the most recent blk file we touched.
    decoded_cache: Mutex<DecodedFileCache>,

    /// Preloaded mapping of height -> block hash for everything we already have indexed.
    height_to_hash: Mutex<HashMap<u32, BlockHash>>,
}

impl BlkOrRpcBlockSource {
    /// Namespace prefix inside ESPO DB for this index. (Literal; includes the trailing slash.)
    pub const MDB_PREFIX: &'static str = "block_core_index/";

    #[inline]
    fn uses_blk_index(mode: BlockFetchMode) -> bool {
        mode != BlockFetchMode::RpcOnly
    }

    pub fn new(
        blocks_dir: impl AsRef<Path>,
        network: Network,
        rpc: &'static CoreClient,
        mode: BlockFetchMode,
    ) -> Result<Self> {
        let db = get_espo_db(); // Arc<DB> from config
        let mdb = Mdb::from_db(db, Self::MDB_PREFIX);

        // Precompute the “stop at genesis” hash for this network (if > 0)
        let genesis_stop_hash = if Self::uses_blk_index(mode) {
            let genesis_height = alkanes_genesis_block(network);
            if genesis_height > 0 {
                match rpc.get_block_hash(genesis_height as u64) {
                    Ok(h) => Some(h),
                    Err(e) => {
                        eprintln!(
                            "[BLOCKFETCHER] warn: failed to fetch genesis stop hash at height {}: {:?}",
                            genesis_height, e
                        );
                        None
                    }
                }
            } else {
                None
            }
        } else {
            eprintln!("[BLOCKFETCHER] rpc-only mode: skipping blk index initialization");
            None
        };

        // Preload height->hash only when blk files may be used during this run.
        let height_map = if Self::uses_blk_index(mode) {
            let height_map = Self::build_height_map(&mdb, rpc)?;
            eprintln!(
                "[BLOCKFETCHER] preloaded {} height→hash entries from index (~{} KB)",
                height_map.len(),
                approx_height_map_kb(height_map.len())
            );
            height_map
        } else {
            HashMap::new()
        };

        Ok(Self {
            mdb,
            blocks_dir: blocks_dir.as_ref().to_path_buf(),
            network,
            rpc,
            mode,
            genesis_stop_hash,
            decoded_cache: Mutex::new(DecodedFileCache::default()),
            height_to_hash: Mutex::new(height_map),
        })
    }

    /// Convenience constructor that uses the Core RPC client from config directly.
    pub fn new_with_config(
        blocks_dir: impl AsRef<Path>,
        network: Network,
        mode: BlockFetchMode,
    ) -> Result<Self> {
        let rpc: &'static CoreClient = get_bitcoind_rpc_client();
        Self::new(blocks_dir, network, rpc, mode)
    }

    /// Utility: rough size estimate for logging (bytes per entry ~ (u32 height + 32B hash) = 36B).
    #[inline]
    fn log_height_map_stats(wherefrom: &str, entries: usize) {
        eprintln!(
            "[BLOCKFETCHER] {} height→hash: {} entries (~{} KB)",
            wherefrom,
            entries,
            approx_height_map_kb(entries)
        );
    }

    /// Scan our namespace and build a map {height -> hash} for every hash we’ve indexed.
    /// We filter out file-markers (5B keys). For each 32B key, query header info once to get height.
    fn build_height_map(mdb: &Mdb, rpc: &CoreClient) -> Result<HashMap<u32, BlockHash>> {
        let mut out: HashMap<u32, BlockHash> = HashMap::new();
        eprintln!("[BLOCKFETCHER] Loading height map from DB (first run may take a bit)...");
        for res in mdb.iter_from(b"") {
            let (k_full, _v) = res.context("iter_from for block_core_index/")?;
            let rel = &k_full[mdb.prefix().len()..];
            if rel.len() != 32 {
                continue; // skip 'F' markers or anything unexpected
            }
            let hash = match BlockHash::from_slice(&rel) {
                Ok(h) => h,
                Err(_) => continue,
            };
            match rpc.get_block_header_info(&hash) {
                Ok(hdr) => {
                    // Only map ACTIVE chain blocks (confirmations > 0). Skip stale/orphans here.
                    if hdr.confirmations > 0 {
                        out.insert(hdr.height as u32, hash);
                    }
                }
                Err(e) => {
                    // Best-effort: if pruned or unknown, just skip
                    eprintln!("[BLOCKFETCHER] build_height_map: header({hash}) err: {:?}", e);
                }
            }
        }

        Ok(out)
    }

    /// Rebuild and replace the in-memory height→hash map from RocksDB.
    fn refresh_height_map_from_db(&self) -> Result<()> {
        let new_map = Self::build_height_map(&self.mdb, self.rpc)?;
        Self::log_height_map_stats("refreshed", new_map.len());
        *self.height_to_hash.lock().unwrap() = new_map;
        Ok(())
    }

    #[inline]
    fn network_magic(&self) -> u32 {
        match self.network {
            Network::Bitcoin => 0xD9B4BEF9,
            Network::Testnet => 0x0709_110B,
            Network::Signet => 0x0A03_CF40,
            Network::Regtest => 0xDAB5_BFFA,
            _ => 0xD9B4BEF9,
        }
    }

    /// Metadata key (under the same Mdb prefix) marking a blk file as "already indexed".
    #[inline]
    fn meta_key_file_indexed(file_no: u32) -> [u8; 5] {
        let mut k = [0u8; 5];
        k[0] = b'F';
        k[1..5].copy_from_slice(&file_no.to_le_bytes());
        k
    }

    /// Extract file number from "blk05159.dat" => 5159.
    fn parse_file_no(p: &Path) -> Result<u32> {
        let name = p
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("bad blk filename: {}", p.display()))?;
        let stem = name.trim_start_matches("blk").trim_end_matches(".dat");
        Ok(stem.parse::<u32>()?)
    }

    /// List blk files newest → oldest by filename.
    fn list_blk_files_desc(&self) -> Result<Vec<PathBuf>> {
        let mut v: Vec<PathBuf> = fs::read_dir(&self.blocks_dir)
            .with_context(|| format!("read_dir {}", self.blocks_dir.display()))?
            .filter_map(|e| {
                let p = e.ok()?.path();
                let name = p.file_name()?.to_string_lossy().to_string();
                if p.extension().map(|e| e == "dat").unwrap_or(false) && name.starts_with("blk") {
                    Some(p)
                } else {
                    None
                }
            })
            .collect();
        v.sort_by(|a, b| b.file_name().cmp(&a.file_name())); // newest first
        Ok(v)
    }

    /// Fetch a location from the index (None if not present).
    #[inline]
    fn index_get(&self, hash: &BlockHash) -> Result<Option<BlockFileLocationDescriptor>> {
        let key = hash.to_byte_array(); // local [u8;32] buffer keeps lifetime simple
        if let Some(val) = self.mdb.get(&key)? {
            let loc =
                BlockFileLocationDescriptor::try_from_slice(&val).context("borsh decode loc")?;
            Ok(Some(loc))
        } else {
            Ok(None)
        }
    }

    /// Check whether a file_no has already been indexed.
    #[inline]
    fn is_file_indexed(&self, file_no: u32) -> Result<bool> {
        let key = Self::meta_key_file_indexed(file_no);
        Ok(self.mdb.get(&key)?.is_some())
    }

    /// Verify a decoded block against Core:
    /// - It must be in the active chain (confirmations > 0).
    /// - Header lookup failure is not accepted as "probably active"; callers can fall back to
    ///   fetching the already-resolved canonical hash by RPC.
    fn verify_block_active_via_rpc(&self, h: &BlockHash, blk: &Block) -> Result<Option<Block>> {
        match self.rpc.get_block_header_info(h) {
            Ok(info) => {
                if info.confirmations <= 0 {
                    eprintln!(
                        "[BLOCKFETCHER] skip cache: {} is not in active chain (confs={})",
                        h, info.confirmations
                    );
                    return Ok(None);
                }
                Ok(Some(blk.clone()))
            }
            Err(e) => Err(anyhow!("active-chain header verification failed for {h}: {e}")),
        }
    }

    /// Fully decode **all blocks** in the given blk file into the single-file cache.
    /// Only ACTIVE-CHAIN blocks (confirmations > 0) are inserted into the cache.
    fn ensure_decoded_file_cached(&self, file_no: u32) -> Result<()> {
        let mut cache = self.decoded_cache.lock().unwrap();
        if cache.file_no == Some(file_no) {
            return Ok(());
        }

        let path = self.blocks_dir.join(format!("blk{:05}.dat", file_no));
        eprintln!(
            "[BLOCKFETCHER] warming decoded cache for file {}",
            path.file_name().unwrap().to_string_lossy()
        );

        // Reset cache for new file
        cache.blocks.clear();
        cache.file_no = Some(file_no);

        let expected_magic = self.network_magic();
        let f = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        let mut r = BufReader::with_capacity(16 << 20, f);

        loop {
            let mut header = [0u8; 8];
            if let Err(e) = r.read_exact(&mut header) {
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    break;
                } else {
                    eprintln!("[BLOCKFETCHER] cache warm read header {}: {:?}", path.display(), e);
                    break;
                }
            }
            if header.iter().all(|&b| b == 0) {
                break; // zero padding at tail
            }

            let magic = u32::from_le_bytes(header[0..4].try_into().unwrap());
            let len = u32::from_le_bytes(header[4..8].try_into().unwrap());
            if magic != expected_magic || len == 0 || len > MAX_BLOCK_PAYLOAD {
                eprintln!(
                    "[BLOCKFETCHER] cache warm: bad record (magic={:#X}, len={}) in {}",
                    magic,
                    len,
                    path.display()
                );
                break;
            }

            let mut payload = vec![0u8; len as usize];
            if let Err(e) = r.read_exact(&mut payload) {
                eprintln!("[BLOCKFETCHER] cache warm payload {}: {:?}", path.display(), e);
                break;
            }

            let blk_from_file: Block = match consensus::encode::deserialize(&payload) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("[BLOCKFETCHER] cache warm decode {}: {:?}", path.display(), e);
                    break;
                }
            };
            let h = blk_from_file.block_hash();

            // === NEW: verify against RPC and only cache ACTIVE-CHAIN blocks ===
            match self.verify_block_active_via_rpc(&h, &blk_from_file)? {
                Some(verified) => {
                    cache.blocks.insert(h, verified);
                }
                None => {
                    // Skip inserting; either not in active chain or RPC failed.
                }
            }
        }

        Ok(())
    }

    /// Index a single blk file: read each record and store (hash → Borsh(loc)) in ONE batch.
    /// Returns (#blocks_indexed, last_block_height_opt) where last_block_height_opt is the height
    /// of the **last** block in this file (via RPC), used for the progress estimate.
    fn index_file(&self, path: &Path, file_no: u32) -> Result<(usize, Option<u32>)> {
        let t0 = Instant::now();
        eprintln!(
            "[BLOCKFETCHER] indexing file {} (no={})",
            path.file_name().unwrap().to_string_lossy(),
            file_no
        );

        let expected_magic = self.network_magic();

        // Open read-only; if missing (pruned), just skip.
        let f = match File::open(path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("[BLOCKFETCHER] skip missing {}: {:?}", path.display(), e);
                return Ok((0, None));
            }
        };
        let mut r = BufReader::with_capacity(16 << 20, f);

        let mut file_pos: u64 = 0u64;
        let mut blocks = 0usize;
        let mut last_hash: Option<BlockHash> = None;

        self.mdb.bulk_write(|wb| {
            loop {
                let mut header = [0u8; 8];
                if let Err(e) = r.read_exact(&mut header) {
                    if e.kind() == std::io::ErrorKind::UnexpectedEof {
                        break;
                    } else {
                        eprintln!("[BLOCKFETCHER] read header error {}: {:?}", path.display(), e);
                        break;
                    }
                }
                if header.iter().all(|&b| b == 0) {
                    break; // zero padding at tail
                }

                let magic = u32::from_le_bytes(header[0..4].try_into().unwrap());
                let len = u32::from_le_bytes(header[4..8].try_into().unwrap());
                if magic != expected_magic {
                    eprintln!(
                        "[BLOCKFETCHER] bad magic in {} at pos {} (exp={:#X} got={:#X})",
                        path.display(),
                        file_pos,
                        expected_magic,
                        magic
                    );
                    break;
                }
                if len == 0 || len > MAX_BLOCK_PAYLOAD {
                    eprintln!(
                        "[BLOCKFETCHER] suspicious len={} in {} at pos {}; abort file",
                        len,
                        path.display(),
                        file_pos
                    );
                    break;
                }

                // Read payload and decode (full decode acceptable)
                let mut payload = vec![0u8; len as usize];
                if let Err(e) = r.read_exact(&mut payload) {
                    eprintln!("[BLOCKFETCHER] payload read error {}: {:?}", path.display(), e);
                    break;
                }

                let blk: Block = match consensus::encode::deserialize(&payload) {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("[BLOCKFETCHER] decode error {}: {:?}", path.display(), e);
                        break;
                    }
                };
                let hash = blk.block_hash();
                last_hash = Some(hash);
                let txs = blk.txdata.len() as u32;

                // We still index *every* block hash → location (including stale),
                // because get_block_by_height always resolves a canonical hash via RPC first.
                let loc = BlockFileLocationDescriptor { file_no, offset: file_pos + 8, len, txs };
                let key = hash.to_byte_array();
                let val = borsh::to_vec(&loc).expect("borsh encode loc");
                wb.put(&key, &val);

                blocks += 1;
                file_pos += 8 + len as u64;
            }

            // mark file as indexed in same batch
            let mark_key = Self::meta_key_file_indexed(file_no);
            wb.put(&mark_key, &[1u8]);
        })?;

        // Get the height of the last block in this file for progress estimation
        let last_height = match last_hash {
            Some(h) => match self.rpc.get_block_header_info(&h) {
                Ok(info) => Some(info.height as u32),
                Err(e) => {
                    eprintln!("[BLOCKFETCHER] warn: get_block_header_info({h}) failed: {:?}", e);
                    None
                }
            },
            None => None,
        };

        eprintln!(
            "[BLOCKFETCHER] indexed {}: {} blocks in {:.2?}",
            path.file_name().unwrap().to_string_lossy(),
            blocks,
            t0.elapsed()
        );
        Ok((blocks, last_height))
    }

    /// Ensure the index contains `hash`; if not, lazily index blk files (newest → older),
    /// skipping those marked as already indexed. We also **stop** once the Alkanes genesis
    /// stop-hash is present in the index, to avoid indexing pre-genesis files.
    /// Progress is: remaining ≈ last_height_in_file - alkanes_genesis_block(network).
    fn ensure_index_contains(&self, hash: &BlockHash, _target_height: u32) -> Result<bool> {
        if self.mode == BlockFetchMode::RpcOnly {
            return Ok(false);
        }
        if self.index_get(hash)?.is_some() {
            return Ok(true);
        }

        if let Some(stop_hash) = self.genesis_stop_hash {
            if self.index_get(&stop_hash)?.is_some() {
                eprintln!(
                    "[BLOCKFETCHER] stop-hash already indexed; target not found → stop scanning"
                );
                return Ok(false);
            }
        }

        let files = self.list_blk_files_desc()?;
        eprintln!(
            "[BLOCKFETCHER] ensure_index_contains: scanning {} files (newest→older) to find {}",
            files.len(),
            hash
        );

        let genesis_h = alkanes_genesis_block(self.network);
        let mut indexed_any = false;

        for (i, p) in files.iter().enumerate() {
            // Stop early if target already present.
            if self.index_get(hash)?.is_some() {
                eprintln!("[BLOCKFETCHER] found {} after scanning {} files", hash, i);
                break;
            }

            // Stop once stop-hash present in index (no pre-genesis scanning).
            if let Some(stop_hash) = self.genesis_stop_hash {
                if self.index_get(&stop_hash)?.is_some() {
                    eprintln!(
                        "[BLOCKFETCHER] stop-hash {} present after {} files; ending scan",
                        stop_hash, i
                    );
                    break;
                }
            }

            let file_no = match Self::parse_file_no(p) {
                Ok(n) => n,
                Err(_) => continue,
            };

            if self.is_file_indexed(file_no)? {
                eprintln!(
                    "[BLOCKFETCHER] skip already-indexed {} [{}/{}]",
                    p.file_name().unwrap().to_string_lossy(),
                    i + 1,
                    files.len(),
                );
                continue;
            }

            eprintln!(
                "[BLOCKFETCHER] → indexing {} [{}/{}]",
                p.file_name().unwrap().to_string_lossy(),
                i + 1,
                files.len()
            );

            match self.index_file(p, file_no) {
                Ok((delta, last_h_opt)) => {
                    indexed_any = true;
                    let remaining = last_h_opt.map(|h| h.saturating_sub(genesis_h)).unwrap_or(0);
                    eprintln!(
                        "   → file done: ~{} blocks indexed; approx ~{} to genesis (based on last block in file)",
                        delta, remaining
                    );
                }
                Err(e) => {
                    eprintln!("[BLOCKFETCHER] index_file failed {}: {:?}", p.display(), e);
                    // continue; RPC fallback still possible
                }
            }
        }

        // If we indexed anything this pass, rebuild (and log) the height→hash map now.
        if indexed_any {
            if let Err(e) = self.refresh_height_map_from_db() {
                eprintln!("[BLOCKFETCHER] warn: failed to refresh height map after scan: {:?}", e);
            }
        }

        Ok(self.index_get(hash)?.is_some())
    }

    /// Read a block directly from a known file location (with **single-file decoded cache**).
    /// Blocks added to the cache are verified against Core (active chain) in ensure_decoded_file_cached.
    fn read_block_from_loc(
        &self,
        hash: &BlockHash,
        loc: &BlockFileLocationDescriptor,
    ) -> Result<Block> {
        // Warm/flip the decoded cache to this file if needed (does active-chain verification).
        self.ensure_decoded_file_cached(loc.file_no)?;

        // Now serve from the in-memory map (O(1)) if present
        if let Some(b) = self.decoded_cache.lock().unwrap().blocks.get(hash).cloned() {
            return Ok(b);
        }

        // Fallback (rare): read just this one from disk, then verify before returning/caching.
        let path = self.blocks_dir.join(format!("blk{:05}.dat", loc.file_no));
        let mut f = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        f.seek(SeekFrom::Start(loc.offset))
            .with_context(|| format!("seek {}", path.display()))?;
        let mut payload = vec![0u8; loc.len as usize];
        f.read_exact(&mut payload)
            .with_context(|| format!("read {} bytes {}", payload.len(), path.display()))?;
        let blk_from_file: Block =
            consensus::encode::deserialize(&payload).context("consensus decode block payload")?;
        let h = blk_from_file.block_hash();

        if &h != hash {
            eprintln!(
                "[BLOCKFETCHER] payload hash {} != expected {}; rejecting local body",
                h, hash
            );
            return Err(anyhow!("blk payload hash mismatch: expected {hash} got {h}"));
        }

        // Verify via RPC (active chain). If accepted, also insert into cache for future calls.
        if let Some(verified) = self.verify_block_active_via_rpc(&h, &blk_from_file)? {
            self.decoded_cache.lock().unwrap().blocks.insert(h, verified.clone());
            return Ok(verified);
        }

        Err(anyhow!("block {hash} failed active-chain verification"))
    }

    fn get_block_result_from_rpc(&self, hash: &BlockHash) -> Result<BlockFetchResult> {
        let verbose: VerboseRpcBlock = match self
            .rpc
            .call("getblock", &[json!(hash.to_string()), json!(BLOCK_FEE_RPC_VERBOSITY)])
        {
            Ok(block) => block,
            Err(err) => {
                eprintln!(
                    "[BLOCKFETCHER] verbose getblock({hash}, {BLOCK_FEE_RPC_VERBOSITY}) failed; falling back to raw block without fee range: {err:?}"
                );
                let block = self
                    .rpc
                    .get_block(hash)
                    .with_context(|| format!("bitcoind: getblock({hash})"))?;
                if block.block_hash() != *hash {
                    return Err(anyhow!(
                        "raw getblock hash mismatch: expected {} got {}",
                        hash,
                        block.block_hash()
                    ));
                }
                return Ok(BlockFetchResult { block, fee_summary: None });
            }
        };

        let previousblockhash = verbose
            .previousblockhash
            .as_deref()
            .map(BlockHash::from_str)
            .transpose()
            .context("parse previousblockhash")?
            .unwrap_or_else(BlockHash::all_zeros);
        let merkle_root = TxMerkleNode::from_str(&verbose.merkleroot)
            .context("parse verbose block merkleroot")?;
        let bits = u32::from_str_radix(verbose.bits.trim_start_matches("0x"), 16)
            .context("parse verbose block bits")?;

        let mut transactions = Vec::with_capacity(verbose.tx.len());
        let mut fee_entries = Vec::new();
        for tx in verbose.tx {
            let raw = hex::decode(&tx.hex).context("decode verbose tx hex")?;
            let transaction: Transaction =
                consensus::encode::deserialize(&raw).context("decode verbose tx")?;
            if let Some(entry) = fee_rate_entry_from_weight_and_btc_fee(tx.weight, tx.fee) {
                fee_entries.push(entry);
            }
            transactions.push(transaction);
        }

        let block = Block {
            header: block::Header {
                version: block::Version::from_consensus(verbose.version),
                prev_blockhash: previousblockhash,
                merkle_root,
                time: verbose.time,
                bits: CompactTarget::from_consensus(bits),
                nonce: verbose.nonce,
            },
            txdata: transactions,
        };

        if block.block_hash() != *hash {
            return Err(anyhow!(
                "verbose getblock reconstructed hash mismatch: expected {} got {}",
                hash,
                block.block_hash()
            ));
        }

        Ok(BlockFetchResult { block, fee_summary: Some(compute_fee_rate_summary(fee_entries)) })
    }
}

impl BlockSource for BlkOrRpcBlockSource {
    fn get_block_by_height(&self, height: u32, tip: u32) -> Result<Block> {
        Ok(self.get_block_result_by_height(height, tip)?.block)
    }

    fn get_block_result_by_height(&self, height: u32, tip: u32) -> Result<BlockFetchResult> {
        let t0 = Instant::now();
        eprintln!(
            "[BLOCKFETCHER] request height={} (tip={}, Δ={}) mode={:?}",
            height,
            tip,
            tip.saturating_sub(height),
            self.mode
        );

        // Fast-path: RPC only mode skips any blk file lookups.
        if self.mode == BlockFetchMode::RpcOnly {
            let hash: BlockHash = self
                .rpc
                .get_block_hash(height as u64)
                .with_context(|| format!("bitcoind: getblockhash({height})"))?;
            let result = self.get_block_result_from_rpc(&hash)?;
            eprintln!("[BLOCKFETCHER] height={} RPC-only ok in {:.2?}", height, t0.elapsed());
            return Ok(result);
        }

        // 1) height → hash via RPC. The in-memory height map is only a hint; Core is the
        // canonical source before we serve any block body.
        let hash: BlockHash = self
            .rpc
            .get_block_hash(height as u64)
            .with_context(|| format!("bitcoind: getblockhash({height})"))?;
        {
            let mut height_map = self.height_to_hash.lock().unwrap();
            if height_map.get(&height).copied().is_some_and(|cached| cached != hash) {
                eprintln!(
                    "[BLOCKFETCHER] canonical hash changed at height {}: cached={} core={}; clearing decoded cache",
                    height,
                    height_map.get(&height).copied().unwrap(),
                    hash
                );
                let mut decoded = self.decoded_cache.lock().unwrap();
                decoded.blocks.clear();
                decoded.file_no = None;
            }
            height_map.insert(height, hash);
        }

        // Near-tip guard: direct RPC (avoid tail races on a file being appended)
        if self.mode != BlockFetchMode::BlkOnly
            && tip.saturating_sub(height) <= NEAR_TIP_RPC_THRESHOLD
        {
            eprintln!("[BLOCKFETCHER] height={} using RPC (near tip)", height);
            let result = self.get_block_result_from_rpc(&hash)?;
            eprintln!("[BLOCKFETCHER] height={} RPC ok in {:.2?}", height, t0.elapsed());
            return Ok(result);
        }

        // 2) Try local index → blk file
        if let Some(loc) = self.index_get(&hash)? {
            eprintln!(
                "[BLOCKFETCHER] height={} hash={} using BLK (file={}, off={}, len={})",
                height, hash, loc.file_no, loc.offset, loc.len
            );
            let blk = match self.read_block_from_loc(&hash, &loc) {
                Ok(blk) => blk,
                Err(e) if self.mode != BlockFetchMode::BlkOnly => {
                    eprintln!(
                        "[BLOCKFETCHER] height={} BLK read failed for canonical hash {}; falling back to RPC: {e:?}",
                        height, hash
                    );
                    let result = self.get_block_result_from_rpc(&hash)?;
                    eprintln!("[BLOCKFETCHER] height={} RPC ok in {:.2?}", height, t0.elapsed());
                    return Ok(result);
                }
                Err(e) => return Err(e),
            };
            if blk.block_hash() != hash {
                return Err(anyhow!(
                    "block fetch hash mismatch at height {}: expected {} got {}",
                    height,
                    hash,
                    blk.block_hash()
                ));
            }
            eprintln!("[BLOCKFETCHER] height={} BLK ok in {:.2?}", height, t0.elapsed());
            return Ok(BlockFetchResult { block: blk, fee_summary: None });
        }

        // 3) Lazily index files until found (but stop once stop-hash is present)
        eprintln!("[BLOCKFETCHER] height={} hash={} not in index → lazy index", height, hash);
        if self.ensure_index_contains(&hash, height)? {
            if let Some(loc) = self.index_get(&hash)? {
                eprintln!(
                    "[BLOCKFETCHER] height={} found after indexing → BLK (file={}, off={}, len={})",
                    height, loc.file_no, loc.offset, loc.len
                );
                let blk = match self.read_block_from_loc(&hash, &loc) {
                    Ok(blk) => blk,
                    Err(e) if self.mode != BlockFetchMode::BlkOnly => {
                        eprintln!(
                            "[BLOCKFETCHER] height={} BLK read failed for canonical hash {}; falling back to RPC: {e:?}",
                            height, hash
                        );
                        let result = self.get_block_result_from_rpc(&hash)?;
                        eprintln!(
                            "[BLOCKFETCHER] height={} RPC ok in {:.2?}",
                            height,
                            t0.elapsed()
                        );
                        return Ok(result);
                    }
                    Err(e) => return Err(e),
                };
                if blk.block_hash() != hash {
                    return Err(anyhow!(
                        "block fetch hash mismatch at height {}: expected {} got {}",
                        height,
                        hash,
                        blk.block_hash()
                    ));
                }
                eprintln!("[BLOCKFETCHER] height={} BLK ok in {:.2?}", height, t0.elapsed());
                return Ok(BlockFetchResult { block: blk, fee_summary: None });
            }
        }

        // 4) Fallback to RPC (e.g., pruned file or not in local blk files)
        if self.mode == BlockFetchMode::BlkOnly {
            return Err(anyhow!(
                "block height {} not found in blk files (RPC fallback disabled by block_source_mode=blk-only)",
                height
            ));
        }

        eprintln!("[BLOCKFETCHER] height={} fallback to RPC (not in local blk files)", height);
        let result = self.get_block_result_from_rpc(&hash)?;
        eprintln!("[BLOCKFETCHER] height={} RPC ok in {:.2?}", height, t0.elapsed());
        Ok(result)
    }
}

/// Helper for logging approximate memory use of the height map.
#[inline]
fn approx_height_map_kb(entries: usize) -> usize {
    // ~36 bytes per entry (u32 + 32B hash) — HashMap overhead not included.
    ((entries * 36) + 1023) / 1024
}

#[cfg(test)]
mod tests {
    use super::{BlkOrRpcBlockSource, BlockFetchMode};

    #[test]
    fn rpc_only_mode_skips_blk_index_usage() {
        assert!(!BlkOrRpcBlockSource::uses_blk_index(BlockFetchMode::RpcOnly));
    }

    #[test]
    fn auto_and_blk_only_modes_use_blk_index() {
        assert!(BlkOrRpcBlockSource::uses_blk_index(BlockFetchMode::Auto));
        assert!(BlkOrRpcBlockSource::uses_blk_index(BlockFetchMode::BlkOnly));
    }
}
