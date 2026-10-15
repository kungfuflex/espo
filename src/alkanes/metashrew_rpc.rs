//! `metashrew_source = "rpc"`: everything the indexer used to read out of a
//! local rockshrew RocksDB (opened as a secondary), fetched over metashrew
//! JSON-RPC instead.
//!
//! What the local path reads per block, and what replaces it here:
//!
//! | local (secondary RocksDB)                         | RPC                                   |
//! |---------------------------------------------------|---------------------------------------|
//! | `__INTERNAL/height`                                | `metashrew_height`                    |
//! | `/__INTERNAL/height-to-hash/<h>`                   | `metashrew_getblockhash [h]`          |
//! | `/trace/height/<h>` list + `/trace/<outpoint>`     | `metashrew_view traceblock` at `h`    |
//! | `/alkanes/<id>` (wasm, gz, alias chain)            | `metashrew_view getbytecode` at `h`   |
//! | `/alkanes/<what>/balances/<who>` (latest)          | `metashrew_view getinventory` latest  |
//!
//! `traceblock` is the alkanes indexer's own view over the same two tables
//! (`TRACES_BY_HEIGHT` and `TRACES`), so it returns the same outpoints and the
//! same `AlkanesTrace` protobufs. Order is irrelevant downstream: espo regroups
//! traces by txid and sorts each group by vout (`partial_traces_indexed`).
//!
//! Height pinning: every per-block read is a `metashrew_view` at that block's
//! height, so it is answered from state as of that block and never from a later
//! tip. The block's hash is checked against bitcoind before the trace fetch and
//! again after it; a reorg between the two fails the block (it is retried by the
//! indexer loop like any other canonicality wait). Nothing here ever fills a gap
//! with a guess: a short, undecodable or missing answer is an error.

use crate::alkanes::trace::PartialEspoTrace;
use crate::schemas::SchemaAlkaneId;
use alkanes_support::proto::alkanes::{
    AlkaneId as PbAlkaneId, AlkaneInventoryRequest, AlkaneInventoryResponse,
    AlkanesBlockTraceEvent, BytecodeRequest, Uint128 as PbUint128,
};
use anyhow::{Context, Result, anyhow, bail};
use bitcoin::BlockHash;
use bitcoin::hashes::Hash;
use prost::Message;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Tunables for the RPC source. Defaults are deliberately gentle: the backing
/// rockshrew is shared infrastructure.
#[derive(Debug, Clone)]
pub struct MetashrewRpcConfig {
    pub url: String,
    /// Maximum in-flight requests to the backend (all kinds combined).
    pub concurrency: usize,
    /// How many blocks ahead of the indexer to prefetch traces for.
    pub prefetch: u32,
    pub timeout: Duration,
    pub max_retries: u32,
    /// Minimum spacing between request starts (a simple rate limit). 0 = off.
    pub min_interval: Duration,
    /// PD endpoints of the TiKV block-trace cache. When set, block traces come
    /// from the cache (see `trace_cache`) and never from a rockshrew; the RPC url
    /// is then only used for the light views (getbytecode / getinventory).
    pub tikv_pd: Vec<String>,
    pub tikv_prefix: String,
    /// Blocks within this many of bitcoind's tip are not in the cache yet.
    pub tikv_settle_depth: u32,
}

impl Default for MetashrewRpcConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            concurrency: 4,
            prefetch: 8,
            timeout: Duration::from_secs(180),
            max_retries: 6,
            min_interval: Duration::ZERO,
            tikv_pd: Vec::new(),
            tikv_prefix: crate::alkanes::trace_cache::DEFAULT_KEY_PREFIX.to_string(),
            tikv_settle_depth: 6,
        }
    }
}

/// Counting semaphore + start-spacing rate limiter shared by every call.
struct Limiter {
    permits: Mutex<usize>,
    cv: Condvar,
    min_interval: Duration,
    next_start: Mutex<Instant>,
}

impl Limiter {
    fn new(permits: usize, min_interval: Duration) -> Self {
        Self {
            permits: Mutex::new(permits.max(1)),
            cv: Condvar::new(),
            min_interval,
            next_start: Mutex::new(Instant::now()),
        }
    }

    fn acquire(&self) -> LimiterGuard<'_> {
        let mut p = self.permits.lock().unwrap_or_else(|e| e.into_inner());
        while *p == 0 {
            p = self.cv.wait(p).unwrap_or_else(|e| e.into_inner());
        }
        *p -= 1;
        drop(p);
        if !self.min_interval.is_zero() {
            let wait = {
                let mut next = self.next_start.lock().unwrap_or_else(|e| e.into_inner());
                let now = Instant::now();
                let start = (*next).max(now);
                *next = start + self.min_interval;
                start.saturating_duration_since(now)
            };
            if !wait.is_zero() {
                std::thread::sleep(wait);
            }
        }
        LimiterGuard { l: self }
    }
}

struct LimiterGuard<'a> {
    l: &'a Limiter,
}

impl Drop for LimiterGuard<'_> {
    fn drop(&mut self) {
        let mut p = self.l.permits.lock().unwrap_or_else(|e| e.into_inner());
        *p += 1;
        self.l.cv.notify_one();
    }
}

/// Per-process request counters, logged by the indexer for load accounting.
#[derive(Default)]
pub struct RpcStats {
    pub requests: AtomicU64,
    pub retries: AtomicU64,
    pub failures: AtomicU64,
    pub bytes: AtomicU64,
    pub micros: AtomicU64,
}

pub struct MetashrewRpc {
    cfg: MetashrewRpcConfig,
    client: reqwest::blocking::Client,
    limiter: Limiter,
    pub stats: RpcStats,
    prefetch: PrefetchCache,
    #[cfg(feature = "tikv-traces")]
    tikv: Option<crate::alkanes::trace_cache::TikvTraceCache>,
    self_balance_cache: Mutex<(u32, HashMap<SchemaAlkaneId, Option<u128>>)>,
}

static RPC: OnceLock<Arc<MetashrewRpc>> = OnceLock::new();

/// The block the indexer is working on; keys per-block memoisation.
static CURRENT_BLOCK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

thread_local! {
    static INDEXER_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Called once on the indexer thread: reads made from it may block-and-retry
/// (fail closed); reads from request handlers must not.
pub fn mark_indexer_thread() {
    INDEXER_THREAD.with(|f| f.set(true));
}

pub fn on_indexer_thread() -> bool {
    INDEXER_THREAD.with(|f| f.get())
}

pub fn set_current_block(h: u32) {
    CURRENT_BLOCK.store(h, Ordering::Relaxed);
}

pub fn current_block() -> u32 {
    CURRENT_BLOCK.load(Ordering::Relaxed)
}

pub fn init(cfg: MetashrewRpcConfig) -> Result<()> {
    if !cfg.tikv_pd.is_empty() && !cfg!(feature = "tikv-traces") {
        bail!("traces_tikv_pd is set but this espo was built without the tikv-traces feature");
    }
    if cfg.url.trim().is_empty() {
        bail!("metashrew_source=rpc requires a metashrew RPC url");
    }
    let rpc = Arc::new(MetashrewRpc::new(cfg)?);
    RPC.set(rpc).map_err(|_| anyhow!("metashrew rpc source already initialized"))
}

pub fn get() -> Option<Arc<MetashrewRpc>> {
    RPC.get().cloned()
}

pub fn require() -> Result<Arc<MetashrewRpc>> {
    get().ok_or_else(|| anyhow!("metashrew rpc source is not initialized"))
}

fn strip0x(s: &str) -> &str {
    s.strip_prefix("0x").unwrap_or(s)
}

pub fn pb_u128(v: u128) -> PbUint128 {
    PbUint128 { lo: v as u64, hi: (v >> 64) as u64 }
}

pub fn pb_alkane_id(id: &SchemaAlkaneId) -> PbAlkaneId {
    PbAlkaneId { block: Some(pb_u128(id.block as u128)), tx: Some(pb_u128(id.tx as u128)) }
}

fn from_pb_u128(v: &Option<PbUint128>) -> u128 {
    v.as_ref().map(|v| (v.lo as u128) | ((v.hi as u128) << 64)).unwrap_or(0)
}

/// Decode a `traceblock` answer into espo's partial traces.
///
/// Mirrors the local reader exactly where it matters: the outpoint key is the
/// consensus encoding (`txid` internal byte order ‖ `vout` LE), duplicates in the
/// height list collapse to their first occurrence, and an entry with no trace
/// payload is a hard error (the local reader logs it as an incomplete index).
pub fn decode_traceblock(raw: &[u8]) -> Result<Vec<PartialEspoTrace>> {
    let decoded =
        AlkanesBlockTraceEvent::decode(raw).context("decode AlkanesBlockTraceEvent from traceblock")?;
    let mut out = Vec::with_capacity(decoded.events.len());
    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    for (i, ev) in decoded.events.into_iter().enumerate() {
        let op = ev.outpoint.ok_or_else(|| anyhow!("traceblock event {i} has no outpoint"))?;
        if op.txid.len() != 32 {
            bail!("traceblock event {i}: txid is {} bytes, expected 32", op.txid.len());
        }
        let mut outpoint = Vec::with_capacity(36);
        outpoint.extend_from_slice(&op.txid);
        outpoint.extend_from_slice(&op.vout.to_le_bytes());
        if !seen.insert(outpoint.clone()) {
            continue;
        }
        let trace = ev.traces.ok_or_else(|| anyhow!("traceblock event {i} has no trace"))?;
        out.push(PartialEspoTrace { protobuf_trace: trace, outpoint });
    }
    Ok(out)
}

/// Parse a 32-byte hash as stored by metashrew, in whichever byte order matches
/// `expected` (rockshrew has stored both orders across versions).
pub fn hash_matches(raw_hex: &str, expected: &BlockHash) -> Result<bool> {
    let raw = hex::decode(strip0x(raw_hex.trim())).context("decode metashrew block hash hex")?;
    if raw.len() != 32 {
        bail!("metashrew block hash is {} bytes, expected 32", raw.len());
    }
    let mut a = [0u8; 32];
    a.copy_from_slice(&raw);
    let direct = BlockHash::from_byte_array(a);
    a.reverse();
    let reversed = BlockHash::from_byte_array(a);
    Ok(direct == *expected || reversed == *expected)
}

impl MetashrewRpc {
    pub fn new(cfg: MetashrewRpcConfig) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(cfg.timeout)
            .pool_max_idle_per_host(cfg.concurrency.max(1) * 2)
            .build()
            .context("build metashrew rpc http client")?;
        let limiter = Limiter::new(cfg.concurrency, cfg.min_interval);
        #[cfg(feature = "tikv-traces")]
        let tikv = if cfg.tikv_pd.is_empty() {
            None
        } else {
            eprintln!(
                "[metashrew-rpc] block traces from the TiKV trace cache (PD {:?}, prefix {:?}); no rockshrew is read for traces",
                cfg.tikv_pd, cfg.tikv_prefix
            );
            Some(crate::alkanes::trace_cache::TikvTraceCache::connect(
                cfg.tikv_pd.clone(),
                cfg.tikv_prefix.clone(),
                cfg.timeout,
            )?)
        };
        Ok(Self {
            #[cfg(feature = "tikv-traces")]
            tikv,
            cfg,
            client,
            limiter,
            stats: RpcStats::default(),
            prefetch: PrefetchCache::default(),
            self_balance_cache: Mutex::new((u32::MAX, HashMap::new())),
        })
    }

    pub fn config(&self) -> &MetashrewRpcConfig {
        &self.cfg
    }

    /// One JSON-RPC call with bounded retries. Transport errors and 5xx retry
    /// with backoff; a JSON-RPC `error` object is returned as an error at once
    /// (it is the backend's answer, not a transient fault).
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let mut attempt: u32 = 0;
        loop {
            let started = Instant::now();
            let res = {
                let _permit = self.limiter.acquire();
                self.stats.requests.fetch_add(1, Ordering::Relaxed);
                self.client.post(&self.cfg.url).json(&body).send().and_then(|r| {
                    let status = r.status();
                    r.bytes().map(|b| (status, b))
                })
            };
            self.stats.micros.fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
            let transient: anyhow::Error = match res {
                Ok((status, bytes)) => {
                    self.stats.bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                    if status.is_server_error() || status.as_u16() == 429 {
                        anyhow!("metashrew rpc {method}: HTTP {status}")
                    } else {
                        let v: Value = serde_json::from_slice(&bytes).with_context(|| {
                            format!("metashrew rpc {method}: invalid JSON (HTTP {status})")
                        })?;
                        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
                            self.stats.failures.fetch_add(1, Ordering::Relaxed);
                            bail!("metashrew rpc {method} error: {err}");
                        }
                        return v
                            .get("result")
                            .cloned()
                            .ok_or_else(|| anyhow!("metashrew rpc {method}: no result"));
                    }
                }
                Err(e) => anyhow!("metashrew rpc {method}: {e}"),
            };
            if attempt >= self.cfg.max_retries {
                self.stats.failures.fetch_add(1, Ordering::Relaxed);
                return Err(transient.context(format!("after {} attempts", attempt + 1)));
            }
            attempt += 1;
            self.stats.retries.fetch_add(1, Ordering::Relaxed);
            let backoff = Duration::from_millis(250u64.saturating_mul(1 << attempt.min(5)));
            eprintln!("[metashrew-rpc] {transient:#}; retry {attempt} in {backoff:?}");
            std::thread::sleep(backoff);
        }
    }

    /// `metashrew_view <fn> <input> <height>`: the answer as bytes. `height`
    /// pins the state the view runs against; `None` is the backend's latest.
    pub fn view(&self, func: &str, input: &[u8], height: Option<u32>) -> Result<Vec<u8>> {
        let at = height.map(|h| h.to_string()).unwrap_or_else(|| "latest".to_string());
        let v = self.call("metashrew_view", json!([func, format!("0x{}", hex::encode(input)), at]))?;
        let s = v.as_str().ok_or_else(|| anyhow!("metashrew_view {func}: result is not a string"))?;
        hex::decode(strip0x(s)).with_context(|| format!("metashrew_view {func}: result is not hex"))
    }

    /// True when block traces come from the TiKV cache.
    pub fn traces_from_tikv(&self) -> bool {
        !self.cfg.tikv_pd.is_empty()
    }

    /// The TiKV alias record for a created alkane (`asp1:alias:`), when the
    /// trace source is the cache. `Ok(None)` = no record (not decorated yet).
    pub fn tikv_alias(&self, id: &SchemaAlkaneId) -> Result<Option<SchemaAlkaneId>> {
        #[cfg(feature = "tikv-traces")]
        if let Some(t) = self.tikv.as_ref() {
            return t.alias(id);
        }
        let _ = id;
        Ok(None)
    }

    /// Highest height the trace source can serve. For the TiKV cache that is
    /// bitcoind's tip minus the settle depth (the cache never holds newer
    /// blocks); for traceblock it is metashrew's indexed height.
    pub fn trace_tip_height(&self) -> Result<u32> {
        if self.traces_from_tikv() {
            use bitcoincore_rpc::RpcApi as _;
            let core = crate::config::get_bitcoind_rpc_client()
                .get_block_count()
                .context("bitcoind getblockcount")?;
            let core = u32::try_from(core).map_err(|_| anyhow!("core tip does not fit u32"))?;
            return Ok(core.saturating_sub(self.cfg.tikv_settle_depth));
        }
        self.tip_height()
    }

    pub fn tip_height(&self) -> Result<u32> {
        let v = self.call("metashrew_height", json!([]))?;
        let h = match &v {
            Value::String(s) => s.trim().parse::<u64>().ok(),
            Value::Number(n) => n.as_u64(),
            _ => None,
        }
        .ok_or_else(|| anyhow!("metashrew_height: unexpected result {v}"))?;
        u32::try_from(h).map_err(|_| anyhow!("metashrew_height {h} does not fit in u32"))
    }

    /// The block hash metashrew indexed at `height`, as the raw hex it returns,
    /// or `None` when it has not indexed that height.
    pub fn indexed_block_hash_hex(&self, height: u32) -> Result<Option<String>> {
        match self.call("metashrew_getblockhash", json!([height])) {
            Ok(Value::String(s)) if !strip0x(&s).is_empty() => Ok(Some(s)),
            Ok(Value::Null) => Ok(None),
            Ok(Value::String(_)) => Ok(None),
            Ok(other) => bail!("metashrew_getblockhash({height}): unexpected result {other}"),
            Err(e) => Err(e),
        }
    }

    /// Same contract as the local `ensure_canonical_height_with_db`, including
    /// the error wording the indexer loop keys its canonicality waits on.
    pub fn ensure_canonical(&self, height: u32, chain_hash: &BlockHash) -> Result<()> {
        let tip = self.tip_height()?;
        if tip < height {
            bail!("metashrew tip {} is behind required height {}", tip, height);
        }
        let Some(raw) = self.indexed_block_hash_hex(height)? else {
            bail!("metashrew missing stored block hash at height {} (indexed_tip={})", height, tip);
        };
        if !hash_matches(&raw, chain_hash)? {
            bail!(
                "metashrew hash mismatch at height {}: core={} metashrew={} indexed_tip={}",
                height,
                chain_hash,
                strip0x(&raw),
                tip
            );
        }
        Ok(())
    }

    /// Traces for `height` from the configured source, pinned to `chain_hash`.
    fn fetch_block_traces(&self, height: u32, chain_hash: &BlockHash) -> Result<Vec<PartialEspoTrace>> {
        #[cfg(feature = "tikv-traces")]
        if let Some(t) = self.tikv.as_ref() {
            let started = Instant::now();
            self.stats.requests.fetch_add(1, Ordering::Relaxed);
            let r = t.traces(height, chain_hash);
            self.stats.micros.fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
            if r.is_err() {
                self.stats.failures.fetch_add(1, Ordering::Relaxed);
            }
            return r;
        }
        let _ = chain_hash;
        self.fetch_traceblock_raw(height)
    }

    fn fetch_traceblock_raw(&self, height: u32) -> Result<Vec<PartialEspoTrace>> {
        // traceblock reads its height from the u32 rockshrew prepends to the
        // (empty) input, exactly as alspo-traces calls it, and runs against
        // state AT that height.
        let raw = self.view("traceblock", &[], Some(height))?;
        decode_traceblock(&raw).with_context(|| format!("traceblock({height})"))
    }

    /// Traces for block `height`, pinned to the canonical block `chain_hash`.
    /// Checks canonicality before and after the fetch; uses a prefetched answer
    /// only if it was fetched against the same hash.
    pub fn traces_for_block(
        &self,
        height: u32,
        chain_hash: &BlockHash,
    ) -> Result<Vec<PartialEspoTrace>> {
        if self.traces_from_tikv() {
            // Keyed by the canonical hash and verified against the cached
            // header: the entry cannot belong to another block, and no
            // rockshrew is consulted.
            return match self.prefetch.take(height, chain_hash) {
                Some(t) => Ok(t),
                None => self.fetch_block_traces(height, chain_hash),
            };
        }
        self.ensure_canonical(height, chain_hash)?;
        let traces = match self.prefetch.take(height, chain_hash) {
            Some(t) => t,
            None => self.fetch_traceblock_raw(height)?,
        };
        // The fetch is pinned to `height`; this second check is what rules out a
        // reorg having replaced the block between the first check and the read.
        self.ensure_canonical(height, chain_hash)
            .with_context(|| format!("block {height} changed while its traces were fetched"))?;
        Ok(traces)
    }

    /// Decompressed wasm of `alkane`, with aliases followed, from state at
    /// `height` (or latest). `None` when the alkane has no binary.
    pub fn bytecode(&self, alkane: &SchemaAlkaneId, height: Option<u32>) -> Result<Option<Vec<u8>>> {
        let req = BytecodeRequest { id: Some(pb_alkane_id(alkane)) };
        // rockshrew prepends the 4-byte height itself; the view's own input is
        // just the request (a second height prefix would be read as protobuf).
        let input = req.encode_to_vec();
        match self.view("getbytecode", &input, height) {
            Ok(b) if b.is_empty() => Ok(None),
            Ok(b) => Ok(Some(b)),
            // getbytecode answers an error for "No bytecode found"
            Err(e) if e.to_string().contains("No bytecode") => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Balance of `what` held by alkane `who` (latest, or at `height`), from
    /// `getinventory`. `None` when `who` has never held `what`, matching the
    /// local reader returning `None` for an empty balance list.
    pub fn alkane_balance(
        &self,
        who: &SchemaAlkaneId,
        what: &SchemaAlkaneId,
        height: Option<u32>,
    ) -> Result<Option<u128>> {
        let req = AlkaneInventoryRequest { id: Some(pb_alkane_id(who)) };
        // rockshrew prepends the 4-byte height itself; the view's own input is
        // just the request (a second height prefix would be read as protobuf).
        let input = req.encode_to_vec();
        let raw = self.view("getinventory", &input, height)?;
        let resp = AlkaneInventoryResponse::decode(raw.as_slice())
            .context("decode AlkaneInventoryResponse")?;
        for t in resp.alkanes.iter() {
            let Some(id) = t.id.as_ref() else { continue };
            if from_pb_u128(&id.block) == what.block as u128
                && from_pb_u128(&id.tx) == what.tx as u128
            {
                return Ok(Some(from_pb_u128(&t.value)));
            }
        }
        Ok(None)
    }

    /// `alkane_balance(alk, alk, latest)` memoised for the duration of one
    /// indexed block (`block` keys the cache; a new block clears it). The local
    /// reader also reads this at the metashrew tip, so latest is the same
    /// semantics, and within one block the answer cannot be allowed to move.
    pub fn self_balance_for_block(&self, block: u32, alk: &SchemaAlkaneId) -> Result<Option<u128>> {
        self.self_balance_for_block_inner(block, alk)
    }

    /// `self_balance_for_block`, but never gives up: a backend that stays down
    /// stalls the indexer instead of letting a guessed `None` into the index.
    pub fn self_balance_blocking(&self, alk: &SchemaAlkaneId) -> Option<u128> {
        let mut attempt: u32 = 0;
        loop {
            match self.self_balance_for_block_inner(current_block(), alk) {
                Ok(v) => return v,
                Err(e) => {
                    attempt += 1;
                    let wait = Duration::from_secs(5u64.saturating_mul(attempt.min(12) as u64));
                    eprintln!(
                        "[metashrew-rpc] self-balance {}:{} failed ({e:#}); retrying in {wait:?} (attempt {attempt})",
                        alk.block, alk.tx
                    );
                    std::thread::sleep(wait);
                }
            }
        }
    }

    fn self_balance_for_block_inner(&self, block: u32, alk: &SchemaAlkaneId) -> Result<Option<u128>> {
        {
            let mut c = self.self_balance_cache.lock().unwrap_or_else(|e| e.into_inner());
            if c.0 != block {
                *c = (block, HashMap::new());
            }
            if let Some(v) = c.1.get(alk) {
                return Ok(*v);
            }
        }
        let v = self.alkane_balance(alk, alk, None)?;
        let mut c = self.self_balance_cache.lock().unwrap_or_else(|e| e.into_inner());
        if c.0 == block {
            c.1.insert(*alk, v);
        }
        Ok(v)
    }

    /// Fetch traces for `heights` ahead of the indexer, `concurrency` at a time,
    /// keeping only answers whose block was canonical (against `hash_of`) both
    /// before and after the fetch. Errors are dropped: the indexer refetches.
    pub fn prefetch(self: &Arc<Self>, heights: Vec<u32>, hash_of: Arc<dyn Fn(u32) -> Option<BlockHash> + Send + Sync>) {
        for h in heights {
            if !self.prefetch.claim(h) {
                continue;
            }
            let me = Arc::clone(self);
            let hash_of = Arc::clone(&hash_of);
            std::thread::spawn(move || {
                let res = (|| -> Result<Option<(BlockHash, Vec<PartialEspoTrace>)>> {
                    let Some(hash) = hash_of(h) else { return Ok(None) };
                    if me.traces_from_tikv() {
                        return Ok(Some((hash, me.fetch_block_traces(h, &hash)?)));
                    }
                    me.ensure_canonical(h, &hash)?;
                    let t = me.fetch_traceblock_raw(h)?;
                    me.ensure_canonical(h, &hash)?;
                    Ok(Some((hash, t)))
                })();
                match res {
                    Ok(Some((hash, t))) => me.prefetch.store(h, hash, t),
                    _ => me.prefetch.release(h),
                }
            });
        }
    }

    pub fn prefetch_window(&self) -> u32 {
        self.cfg.prefetch
    }

    pub fn prefetch_drop_below(&self, height: u32) {
        self.prefetch.drop_below(height);
    }
}

#[derive(Default)]
struct PrefetchCache {
    inner: Mutex<PrefetchInner>,
}

#[derive(Default)]
struct PrefetchInner {
    in_flight: HashSet<u32>,
    ready: BTreeMap<u32, (BlockHash, Vec<PartialEspoTrace>)>,
}

impl PrefetchCache {
    fn claim(&self, h: u32) -> bool {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.in_flight.contains(&h) || g.ready.contains_key(&h) {
            return false;
        }
        g.in_flight.insert(h)
    }

    fn release(&self, h: u32) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).in_flight.remove(&h);
    }

    fn store(&self, h: u32, hash: BlockHash, t: Vec<PartialEspoTrace>) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.in_flight.remove(&h);
        g.ready.insert(h, (hash, t));
    }

    fn take(&self, h: u32, hash: &BlockHash) -> Option<Vec<PartialEspoTrace>> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match g.ready.remove(&h) {
            Some((stored, t)) if stored == *hash => Some(t),
            _ => None,
        }
    }

    fn drop_below(&self, h: u32) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.ready = g.ready.split_off(&h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alkanes_support::proto::alkanes::{
        AlkanesBlockEvent, AlkanesCreate, AlkanesTrace, AlkanesTraceEvent, Outpoint,
        alkanes_trace_event,
    };

    fn trace_with_create(tx: u128) -> AlkanesTrace {
        AlkanesTrace {
            events: vec![AlkanesTraceEvent {
                event: Some(alkanes_trace_event::Event::CreateAlkane(AlkanesCreate {
                    new_alkane: Some(PbAlkaneId { block: Some(pb_u128(2)), tx: Some(pb_u128(tx)) }),
                })),
            }],
        }
    }

    #[test]
    fn traceblock_decodes_to_consensus_outpoints_and_dedups() {
        let txid = [7u8; 32];
        let ev = |vout: u32, tx: u128| AlkanesBlockEvent {
            traces: Some(trace_with_create(tx)),
            outpoint: Some(Outpoint { txid: txid.to_vec(), vout }),
            txindex: 3,
        };
        let block = AlkanesBlockTraceEvent { events: vec![ev(5, 1), ev(4, 2), ev(5, 9)] };
        let out = decode_traceblock(&block.encode_to_vec()).expect("decode");
        assert_eq!(out.len(), 2, "duplicate outpoint keeps its first occurrence");
        let mut expect = txid.to_vec();
        expect.extend_from_slice(&5u32.to_le_bytes());
        assert_eq!(out[0].outpoint, expect);
        assert_eq!(out[0].protobuf_trace, trace_with_create(1));
        assert_eq!(&out[1].outpoint[32..], &4u32.to_le_bytes());
    }

    #[test]
    fn traceblock_rejects_missing_trace_or_bad_txid() {
        let block = AlkanesBlockTraceEvent {
            events: vec![AlkanesBlockEvent {
                traces: None,
                outpoint: Some(Outpoint { txid: vec![1; 32], vout: 0 }),
                txindex: 0,
            }],
        };
        assert!(decode_traceblock(&block.encode_to_vec()).is_err());
        let block = AlkanesBlockTraceEvent {
            events: vec![AlkanesBlockEvent {
                traces: Some(trace_with_create(1)),
                outpoint: Some(Outpoint { txid: vec![1; 31], vout: 0 }),
                txindex: 0,
            }],
        };
        assert!(decode_traceblock(&block.encode_to_vec()).is_err());
    }

    #[test]
    fn block_hash_matches_either_byte_order() {
        let h: BlockHash =
            "00000000000000000001b1a3d1b6c2bd7e5a0a0b4dbd6d4e6c2c1c5d4b3a2f1e".parse().unwrap();
        let mut raw = h.to_byte_array();
        assert!(hash_matches(&hex::encode(raw), &h).unwrap());
        raw.reverse();
        assert!(hash_matches(&format!("0x{}", hex::encode(raw)), &h).unwrap());
        assert!(!hash_matches(&hex::encode([0u8; 32]), &h).unwrap());
        assert!(hash_matches("00", &h).is_err());
    }

    #[test]
    fn prefetch_cache_only_serves_the_hash_it_was_fetched_for() {
        let c = PrefetchCache::default();
        let a: BlockHash = BlockHash::from_byte_array([1; 32]);
        let b: BlockHash = BlockHash::from_byte_array([2; 32]);
        assert!(c.claim(10));
        assert!(!c.claim(10), "in flight");
        c.store(10, a, vec![]);
        assert!(c.take(10, &b).is_none(), "a reorged block is never served");
        assert!(c.take(10, &a).is_none(), "the mismatched take consumed it");
        assert!(c.claim(11));
        c.store(11, a, vec![]);
        assert!(c.take(11, &a).is_some());
        assert!(c.claim(12));
        c.store(12, a, vec![]);
        c.drop_below(13);
        assert!(c.take(12, &a).is_none());
    }

    #[test]
    fn limiter_bounds_concurrency() {
        let l = Arc::new(Limiter::new(2, Duration::ZERO));
        let live = Arc::new(AtomicU64::new(0));
        let peak = Arc::new(AtomicU64::new(0));
        let hs: Vec<_> = (0..8)
            .map(|_| {
                let (l, live, peak) = (l.clone(), live.clone(), peak.clone());
                std::thread::spawn(move || {
                    let _g = l.acquire();
                    let n = live.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(n, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    live.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert!(peak.load(Ordering::SeqCst) <= 2);
    }
}
