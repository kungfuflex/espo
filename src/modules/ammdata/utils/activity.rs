use crate::modules::ammdata::consts::AMOUNT_SCALE;
use crate::modules::ammdata::schemas::{ActivityDirection, ActivityKind, SchemaActivityV1};
use crate::modules::ammdata::storage::{
    AmmDataProvider, GetListEntriesDescParams, GetRawValueParams,
};
use crate::modules::ammdata::utils::candles::PriceSide;
use crate::runtime::state_at::StateAt;
use crate::schemas::SchemaAlkaneId;
use anyhow::Result;
use borsh::{BorshDeserialize, to_vec};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

/* ----------------------- storage key helpers -----------------------
IMPORTANT: All keys here are RELATIVE to the DB-level namespace.
The DB layer (mdb) will prepend "ammdata:" exactly once.           */

fn activity_ns_prefix(pool: &SchemaAlkaneId) -> Vec<u8> {
    // namespace:  "activity:v1:<block>:<tx>:"
    format!("activity:v1:{}:{}:", pool.block, pool.tx).into_bytes()
}

/// Key = namespace + "<ts>:<seq>"
fn activity_key(pool: &SchemaAlkaneId, ts: u64, seq: u32) -> Vec<u8> {
    let mut k = activity_ns_prefix(pool);
    k.extend_from_slice(ts.to_string().as_bytes());
    k.push(b':');
    k.extend_from_slice(seq.to_string().as_bytes());
    k
}

#[derive(BorshDeserialize)]
struct SchemaActivityV1Legacy {
    pub timestamp: u64,
    pub txid: [u8; 32],
    pub kind: ActivityKind,
    pub direction: Option<ActivityDirection>,
    pub base_delta: i128,
    pub quote_delta: i128,
}

// simple encode/decode (borsh) for SchemaActivityV1
#[inline]
fn encode_activity_v1(activity: &SchemaActivityV1) -> Result<Vec<u8>> {
    Ok(to_vec(activity)?)
}

#[inline]
pub fn decode_activity_v1(v: &[u8]) -> Result<SchemaActivityV1> {
    if let Ok(parsed) = SchemaActivityV1::try_from_slice(v) {
        return Ok(parsed);
    }
    let legacy = SchemaActivityV1Legacy::try_from_slice(v)?;
    Ok(SchemaActivityV1 {
        timestamp: legacy.timestamp,
        txid: legacy.txid,
        kind: legacy.kind,
        direction: legacy.direction,
        base_delta: legacy.base_delta,
        quote_delta: legacy.quote_delta,
        address_spk: Vec::new(),
        success: true,
    })
}

/* ---------------- index helpers ---------------- */

#[inline]
fn be_u32(x: u32) -> [u8; 4] {
    x.to_be_bytes()
}
#[inline]
fn be_u64(x: u64) -> [u8; 8] {
    x.to_be_bytes()
}
#[inline]
fn be_u128(x: u128) -> [u8; 16] {
    x.to_be_bytes()
}
#[inline]
fn abs_i128(x: i128) -> u128 {
    if x < 0 { (-x) as u128 } else { x as u128 }
}

/// Side byte codes used in side-aware indexes.
/// 0=buy (v<0), 1=neutral (v==0), 2=sell (v>0)
#[inline]
pub fn side_code(v: i128) -> u8 {
    if v < 0 {
        0
    } else if v > 0 {
        2
    } else {
        1
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ActivityGroup {
    Trades,
    Events,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityFilter {
    All,
    Trades,
    Events,
}

fn group_for_kind(kind: ActivityKind) -> ActivityGroup {
    match kind {
        ActivityKind::TradeBuy | ActivityKind::TradeSell => ActivityGroup::Trades,
        ActivityKind::LiquidityAdd | ActivityKind::LiquidityRemove | ActivityKind::PoolCreate => {
            ActivityGroup::Events
        }
    }
}

fn group_tag(group: ActivityGroup) -> &'static str {
    match group {
        ActivityGroup::Trades => "trades",
        ActivityGroup::Events => "events",
    }
}

fn group_from_filter(filter: ActivityFilter) -> Option<ActivityGroup> {
    match filter {
        ActivityFilter::All => None,
        ActivityFilter::Trades => Some(ActivityGroup::Trades),
        ActivityFilter::Events => Some(ActivityGroup::Events),
    }
}

fn idx_ns_prefix(pool: &SchemaAlkaneId, group: Option<ActivityGroup>) -> String {
    if let Some(g) = group {
        format!("activity:idx:v1:{}:{}:{}:", pool.block, pool.tx, group_tag(g))
    } else {
        format!("activity:idx:v1:{}:{}:", pool.block, pool.tx)
    }
}

fn idx_prefix_ts(pool: &SchemaAlkaneId, group: Option<ActivityGroup>) -> Vec<u8> {
    format!("{}ts:", idx_ns_prefix(pool, group)).into_bytes()
}
fn idx_prefix_absb(pool: &SchemaAlkaneId, group: Option<ActivityGroup>) -> Vec<u8> {
    format!("{}absb:", idx_ns_prefix(pool, group)).into_bytes()
}
fn idx_prefix_absq(pool: &SchemaAlkaneId, group: Option<ActivityGroup>) -> Vec<u8> {
    format!("{}absq:", idx_ns_prefix(pool, group)).into_bytes()
}
fn idx_prefix_sb_absb(pool: &SchemaAlkaneId, group: Option<ActivityGroup>) -> Vec<u8> {
    format!("{}sb_absb:", idx_ns_prefix(pool, group)).into_bytes()
}
fn idx_prefix_sq_absq(pool: &SchemaAlkaneId, group: Option<ActivityGroup>) -> Vec<u8> {
    format!("{}sq_absq:", idx_ns_prefix(pool, group)).into_bytes()
}
fn idx_prefix_sb_ts(pool: &SchemaAlkaneId, group: Option<ActivityGroup>) -> Vec<u8> {
    format!("{}sb_ts:", idx_ns_prefix(pool, group)).into_bytes()
}
fn idx_prefix_sq_ts(pool: &SchemaAlkaneId, group: Option<ActivityGroup>) -> Vec<u8> {
    format!("{}sq_ts:", idx_ns_prefix(pool, group)).into_bytes()
}

/// Public so index updater can maintain O(1) totals per pool (optional).
pub fn idx_count_key(pool: &SchemaAlkaneId) -> Vec<u8> {
    format!("{}__count", idx_ns_prefix(pool, None)).into_bytes()
}

pub fn idx_count_key_group(pool: &SchemaAlkaneId, group: ActivityGroup) -> Vec<u8> {
    format!("{}__count", idx_ns_prefix(pool, Some(group))).into_bytes()
}

/// Public so writer/reader can encode/decode counts when used.
pub fn encode_u64_be(x: u64) -> [u8; 8] {
    x.to_be_bytes()
}
pub fn decode_u64_be(v: &[u8]) -> Option<u64> {
    if v.len() == 8 {
        let mut b = [0u8; 8];
        b.copy_from_slice(v);
        Some(u64::from_be_bytes(b))
    } else {
        None
    }
}

fn append_index_entries(
    writes: &mut Vec<(Vec<u8>, Vec<u8>)>,
    pool: &SchemaAlkaneId,
    ts: u64,
    seq: u32,
    a: &SchemaActivityV1,
    group: Option<ActivityGroup>,
) {
    // value payload to make reads robust and backward/future-proof
    let mut val = Vec::with_capacity(12);
    val.extend_from_slice(&be_u64(ts));
    val.extend_from_slice(&be_u32(seq));

    let absb = abs_i128(a.base_delta);
    let absq = abs_i128(a.quote_delta);
    let sb = side_code(a.base_delta);
    let sq = side_code(a.quote_delta);

    // ts:       ... ts(8) seq(4)
    {
        let mut k = idx_prefix_ts(pool, group);
        k.extend_from_slice(&be_u64(ts));
        k.extend_from_slice(&be_u32(seq));
        writes.push((k, val.clone()));
    }
    // absb:     ... absb(16) ts(8) seq(4)
    {
        let mut k = idx_prefix_absb(pool, group);
        k.extend_from_slice(&be_u128(absb));
        k.extend_from_slice(&be_u64(ts));
        k.extend_from_slice(&be_u32(seq));
        writes.push((k, val.clone()));
    }
    // absq:     ... absq(16) ts(8) seq(4)
    {
        let mut k = idx_prefix_absq(pool, group);
        k.extend_from_slice(&be_u128(absq));
        k.extend_from_slice(&be_u64(ts));
        k.extend_from_slice(&be_u32(seq));
        writes.push((k, val.clone()));
    }
    // sb_absb:  ... side(1) absb(16) ts(8) seq(4)
    {
        let mut k = idx_prefix_sb_absb(pool, group);
        k.push(sb);
        k.extend_from_slice(&be_u128(absb));
        k.extend_from_slice(&be_u64(ts));
        k.extend_from_slice(&be_u32(seq));
        writes.push((k, val.clone()));
    }
    // sq_absq:  ... side(1) absq(16) ts(8) seq(4)
    {
        let mut k = idx_prefix_sq_absq(pool, group);
        k.push(sq);
        k.extend_from_slice(&be_u128(absq));
        k.extend_from_slice(&be_u64(ts));
        k.extend_from_slice(&be_u32(seq));
        writes.push((k, val.clone()));
    }
    // sb_ts:    ... side(1) ts(8) seq(4)
    {
        let mut k = idx_prefix_sb_ts(pool, group);
        k.push(sb);
        k.extend_from_slice(&be_u64(ts));
        k.extend_from_slice(&be_u32(seq));
        writes.push((k, val.clone()));
    }
    // sq_ts:    ... side(1) ts(8) seq(4)
    {
        let mut k = idx_prefix_sq_ts(pool, group);
        k.push(sq);
        k.extend_from_slice(&be_u64(ts));
        k.extend_from_slice(&be_u32(seq));
        writes.push((k, val));
    }
}

/// Accumulator while walking a block (ensures unique keys per ts via seq).
/// Also emits secondary index keys (with values = [ts_be|seq_be]) for robust reads.
pub struct ActivityWriteAcc {
    seqs: BTreeMap<(SchemaAlkaneId, u64), u32>,
    writes: Vec<(Vec<u8>, Vec<u8>)>,
}

impl ActivityWriteAcc {
    pub fn new() -> Self {
        Self { seqs: BTreeMap::new(), writes: Vec::new() }
    }

    /// Push one activity value for a given pool+timestamp. Handles `seq`.
    /// Writes secondary indexes with value = ts_be(8) || seq_be(4)
    pub fn push(&mut self, pool: SchemaAlkaneId, ts: u64, a: SchemaActivityV1) -> Result<u32> {
        // primary
        let entry = self.seqs.entry((pool, ts)).or_insert(0);
        let seq = *entry;
        *entry = entry.wrapping_add(1);

        let k = activity_key(&pool, ts, seq);
        let v = encode_activity_v1(&a)?;
        self.writes.push((k, v));

        append_index_entries(&mut self.writes, &pool, ts, seq, &a, None);
        let group = group_for_kind(a.kind);
        append_index_entries(&mut self.writes, &pool, ts, seq, &a, Some(group));

        Ok(seq)
    }

    pub fn into_writes(self) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.writes
    }
}

impl Default for ActivityWriteAcc {
    fn default() -> Self {
        Self::new()
    }
}

/// Accumulator for secondary index writes and per-pool count deltas.
#[derive(Clone)]
pub struct ActivityIndexAcc {
    writes: Vec<(Vec<u8>, Vec<u8>)>,
    /// number of new events per pool in this block (all indexes share same count)
    per_pool_delta: HashMap<(u32, u64), u64>,
    per_pool_group_delta: HashMap<(u32, u64, ActivityGroup), u64>,
}

impl ActivityIndexAcc {
    pub fn new() -> Self {
        Self {
            writes: Vec::new(),
            per_pool_delta: HashMap::new(),
            per_pool_group_delta: HashMap::new(),
        }
    }

    /// Mirror the same index layout/value as ActivityWriteAcc::push (ts_be||seq_be).
    pub fn add(&mut self, pool: &SchemaAlkaneId, ts: u64, seq: u32, a: &SchemaActivityV1) {
        append_index_entries(&mut self.writes, pool, ts, seq, a, None);
        let group = group_for_kind(a.kind);
        append_index_entries(&mut self.writes, pool, ts, seq, a, Some(group));

        *self.per_pool_delta.entry((pool.block, pool.tx)).or_insert(0) += 1;
        *self.per_pool_group_delta.entry((pool.block, pool.tx, group)).or_insert(0) += 1;
    }

    pub fn into_writes(self) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.writes
    }
    pub fn per_pool_delta(self) -> HashMap<(u32, u64), u64> {
        self.per_pool_delta
    }
    pub fn per_pool_group_delta(self) -> HashMap<(u32, u64, ActivityGroup), u64> {
        self.per_pool_group_delta
    }
}

impl Default for ActivityIndexAcc {
    fn default() -> Self {
        Self::new()
    }
}

/* ---------------- reader: paginated & side-injected ---------------- */

#[derive(Serialize, Clone)]
pub struct ActivityRow {
    pub timestamp: u64,
    /// txid hex, big-endian (display order)
    pub txid: String,
    pub kind: String,
    pub direction: Option<String>,

    /// raw deltas as strings (exact i128 as stored)
    pub base_delta: String,
    pub quote_delta: String,

    /// derived from chosen side + delta sign
    pub side: ActivityUiSide,

    /// absolute volume on the chosen side (scaled by AMOUNT_SCALE)
    pub amount: f64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum ActivityUiSide {
    Buy,
    Sell,
    Neutral,
}

impl ActivityRow {
    #[inline]
    fn to_be(mut h: [u8; 32]) -> [u8; 32] {
        h.reverse();
        h
    }

    #[inline]
    fn scale_u128(x: u128) -> f64 {
        (x as f64) / (AMOUNT_SCALE as f64)
    }

    fn kind_str(kind: ActivityKind, side: &ActivityUiSide) -> &'static str {
        match kind {
            ActivityKind::TradeBuy | ActivityKind::TradeSell => match side {
                ActivityUiSide::Buy => "trade_buy",
                ActivityUiSide::Sell => "trade_sell",
                ActivityUiSide::Neutral => "trade",
            },
            ActivityKind::LiquidityAdd => "liquidity_add",
            ActivityKind::LiquidityRemove => "liquidity_remove",
            ActivityKind::PoolCreate => "pool_create",
        }
    }

    fn direction_str(dir: ActivityDirection) -> &'static str {
        match dir {
            ActivityDirection::BaseIn => "base_in",
            ActivityDirection::QuoteIn => "quote_in",
        }
    }

    pub fn from_storage(a: &SchemaActivityV1, chosen: PriceSide) -> Self {
        let (amt_i128, mut side) = match chosen {
            PriceSide::Base => {
                let v = a.base_delta;
                let s = if v < 0 {
                    ActivityUiSide::Buy
                } else if v > 0 {
                    ActivityUiSide::Sell
                } else {
                    ActivityUiSide::Neutral
                };
                (v, s)
            }
            PriceSide::Quote => {
                let v = a.quote_delta;
                let s = if v < 0 {
                    ActivityUiSide::Buy
                } else if v > 0 {
                    ActivityUiSide::Sell
                } else {
                    ActivityUiSide::Neutral
                };
                (v, s)
            }
        };

        if !matches!(a.kind, ActivityKind::TradeBuy | ActivityKind::TradeSell) {
            side = ActivityUiSide::Neutral;
        }

        let amt_abs_u128 = if amt_i128 < 0 { (-amt_i128) as u128 } else { amt_i128 as u128 };

        ActivityRow {
            timestamp: a.timestamp,
            txid: hex::encode(Self::to_be(a.txid)),
            kind: Self::kind_str(a.kind, &side).to_string(),
            direction: a.direction.map(Self::direction_str).map(|s| s.to_string()),
            base_delta: a.base_delta.to_string(),
            quote_delta: a.quote_delta.to_string(),
            side,
            amount: Self::scale_u128(amt_abs_u128),
        }
    }
}

pub struct ActivityPage {
    pub activity: Vec<ActivityRow>,
    pub total: usize,
}

/* -------- legacy reader: newest→oldest by timestamp (primary) ------ */

#[inline]
fn parse_ts_from_key_tail(k: &[u8]) -> Option<u64> {
    // key ends with "...:<ts>:<seq>"
    let mut parts = k.rsplit(|&b| b == b':');
    let _seq_b = parts.next();
    if let Some(ts_b) = parts.next() {
        if let Ok(ts_s) = std::str::from_utf8(ts_b) {
            if let Ok(ts) = ts_s.parse::<u64>() {
                return Some(ts);
            }
        }
    }
    None
}

/// Read activity newest→oldest with pagination.
pub fn read_activity_for_pool(
    provider: &AmmDataProvider,
    pool: SchemaAlkaneId,
    page: usize,
    limit: usize,
    side: PriceSide,
    activity_type: ActivityFilter,
) -> Result<ActivityPage> {
    // Collect newest → oldest
    let mut all: Vec<(u64, SchemaActivityV1)> = Vec::new();
    let prefix = activity_ns_prefix(&pool);
    for (k, v) in provider
        .get_list_entries_desc(GetListEntriesDescParams { blockhash: StateAt::Latest, prefix })?
        .entries
    {
        let ts = parse_ts_from_key_tail(&k).unwrap_or_default();
        let a = decode_activity_v1(&v)?;
        if let Some(group) = group_from_filter(activity_type) {
            if group_for_kind(a.kind) != group {
                continue;
            }
        }
        all.push((ts, a));
    }

    let total = all.len();
    if limit == 0 {
        return Ok(ActivityPage { activity: vec![], total });
    }

    // paging: page is 1-based; newest-first already
    let start = page.saturating_sub(1).saturating_mul(limit);
    let end = (start + limit).min(total);
    let slice: &[(u64, SchemaActivityV1)] = if start >= end { &[] } else { &all[start..end] };

    let activity: Vec<ActivityRow> =
        slice.iter().map(|(_ts, a)| ActivityRow::from_storage(a, side)).collect();

    Ok(ActivityPage { activity, total })
}

/* --------- new: sorted reads over secondary index prefixes ---------- */

#[derive(Clone, Copy, Debug)]
pub enum ActivitySortKey {
    Timestamp,       // "ts"
    AmountBaseAbs,   // "absb"
    AmountQuoteAbs,  // "absq"
    SideBaseTs,      // "sb_ts"
    SideQuoteTs,     // "sq_ts"
    SideBaseAmount,  // "sb_absb"
    SideQuoteAmount, // "sq_absq"
}

#[derive(Clone, Copy, Debug)]
pub enum SortDir {
    Asc,
    Desc,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ActivitySideFilter {
    All,
    Buy,
    Sell,
}

fn idx_prefix_for(
    pool: &SchemaAlkaneId,
    k: ActivitySortKey,
    group: Option<ActivityGroup>,
) -> Vec<u8> {
    match k {
        ActivitySortKey::Timestamp => idx_prefix_ts(pool, group),
        ActivitySortKey::AmountBaseAbs => idx_prefix_absb(pool, group),
        ActivitySortKey::AmountQuoteAbs => idx_prefix_absq(pool, group),
        ActivitySortKey::SideBaseAmount => idx_prefix_sb_absb(pool, group),
        ActivitySortKey::SideQuoteAmount => idx_prefix_sq_absq(pool, group),
        ActivitySortKey::SideBaseTs => idx_prefix_sb_ts(pool, group),
        ActivitySortKey::SideQuoteTs => idx_prefix_sq_ts(pool, group),
    }
}

/// Returns (effective_sort_key, fixed_side_byte_or_none) when a side filter is applied.
fn adjust_for_side_filter(
    requested_sort: ActivitySortKey,
    chosen_side: PriceSide,
    filter: ActivitySideFilter,
) -> (ActivitySortKey, Option<u8>) {
    let fixed = match filter {
        ActivitySideFilter::All => None,
        ActivitySideFilter::Buy => Some(0u8),
        ActivitySideFilter::Sell => Some(2u8),
    };

    if fixed.is_none() {
        return (requested_sort, None);
    }

    // ensure we use a side-aware index and fix the side-byte in prefix
    let eff = match requested_sort {
        ActivitySortKey::Timestamp => match chosen_side {
            PriceSide::Base => ActivitySortKey::SideBaseTs,
            PriceSide::Quote => ActivitySortKey::SideQuoteTs,
        },
        ActivitySortKey::AmountBaseAbs => ActivitySortKey::SideBaseAmount,
        ActivitySortKey::AmountQuoteAbs => ActivitySortKey::SideQuoteAmount,
        ActivitySortKey::SideBaseAmount
        | ActivitySortKey::SideQuoteAmount
        | ActivitySortKey::SideBaseTs
        | ActivitySortKey::SideQuoteTs => requested_sort,
    };

    (eff, fixed)
}

/// Try to get the O(1) count; fallback to reverse scanning when a fixed-side subset is used.
fn total_for_pool_index(
    provider: &AmmDataProvider,
    prefix: &[u8],
    count_key: Option<Vec<u8>>,
) -> Result<usize> {
    if let Some(count_k) = count_key {
        if let Some(v) = provider
            .get_raw_value(GetRawValueParams { blockhash: StateAt::Latest, key: count_k })?
            .value
        {
            if let Some(n) = decode_u64_be(&v) {
                return Ok(n as usize);
            }
        }
    }
    let entries = provider
        .get_list_entries_desc(GetListEntriesDescParams {
            blockhash: StateAt::Latest,
            prefix: prefix.to_vec(),
        })?
        .entries;
    Ok(entries.len())
}

/// Decode (ts, seq) from index value if present, else from key tail (last 12 bytes).
fn decode_ts_seq_from_entry(key: &[u8], val: &[u8]) -> Option<(u64, u32)> {
    if val.len() == 12 {
        let mut bts = [0u8; 8];
        let mut bsq = [0u8; 4];
        bts.copy_from_slice(&val[0..8]);
        bsq.copy_from_slice(&val[8..12]);
        return Some((u64::from_be_bytes(bts), u32::from_be_bytes(bsq)));
    }
    if key.len() >= 12 {
        let mut bts = [0u8; 8];
        let mut bsq = [0u8; 4];
        bts.copy_from_slice(&key[key.len() - 12..key.len() - 4]);
        bsq.copy_from_slice(&key[key.len() - 4..]);
        return Some((u64::from_be_bytes(bts), u32::from_be_bytes(bsq)));
    }
    None
}

/// Read (page, limit) according to the chosen secondary index (sort), direction, and optional side filter.
pub fn read_activity_for_pool_sorted(
    provider: &AmmDataProvider,
    pool: SchemaAlkaneId,
    page: usize,
    limit: usize,
    chosen_side: PriceSide,
    sort: ActivitySortKey,
    dir: SortDir,
    filter: ActivitySideFilter,
    activity_type: ActivityFilter,
) -> Result<ActivityPage> {
    let group = group_from_filter(activity_type);
    if matches!(group, Some(ActivityGroup::Events)) && !matches!(filter, ActivitySideFilter::All) {
        return Ok(ActivityPage { activity: vec![], total: 0 });
    }

    // adjust sort & get side byte (if any) when filtering
    let (eff_sort, fixed_side) = adjust_for_side_filter(sort, chosen_side, filter);

    // base index prefix (relative) for reverse iteration
    let mut iprefix = idx_prefix_for(&pool, eff_sort, group);

    // if we have a fixed side, narrow the prefix one more byte
    if let Some(sb) = fixed_side {
        iprefix.push(sb);
    }

    // count
    let allow_count_key = fixed_side.is_none()
        && matches!(
            eff_sort,
            ActivitySortKey::Timestamp
                | ActivitySortKey::AmountBaseAbs
                | ActivitySortKey::AmountQuoteAbs
        );
    let count_key = if allow_count_key {
        match group {
            Some(g) => Some(idx_count_key_group(&pool, g)),
            None => Some(idx_count_key(&pool)),
        }
    } else {
        None
    };
    let total = total_for_pool_index(provider, &iprefix, count_key)?;
    if limit == 0 {
        return Ok(ActivityPage { activity: vec![], total });
    }

    let skip = limit.saturating_mul(page.saturating_sub(1));

    // scan index keys and decode (ts, seq) from value (preferred) or key tail
    let read_pairs_from_prefix = |prefix: &[u8]| -> Result<Vec<(u64, u32)>> {
        let mut entries = provider
            .get_list_entries_desc(GetListEntriesDescParams {
                blockhash: StateAt::Latest,
                prefix: prefix.to_vec(),
            })?
            .entries;
        if matches!(dir, SortDir::Asc) {
            entries.reverse();
        }
        let mut out: Vec<(u64, u32)> = Vec::with_capacity(limit);
        for (i, (k, v)) in entries.into_iter().enumerate() {
            if i < skip {
                continue;
            }
            if out.len() >= limit {
                break;
            }
            if let Some(pair) = decode_ts_seq_from_entry(&k, &v) {
                out.push(pair);
            }
        }
        Ok(out)
    };

    // use the requested (possibly side-aware) index
    let pairs = read_pairs_from_prefix(&iprefix)?;

    // materialize primaries → ActivityRow
    let mut rows = Vec::with_capacity(pairs.len());
    for (ts, seq) in pairs {
        // RELATIVE primary key: "activity:v1:<pool>:<ts>:<seq>"
        let mut pk = activity_ns_prefix(&pool);
        pk.extend_from_slice(ts.to_string().as_bytes());
        pk.push(b':');
        pk.extend_from_slice(seq.to_string().as_bytes());

        if let Some(v) = provider
            .get_raw_value(GetRawValueParams { blockhash: StateAt::Latest, key: pk })?
            .value
        {
            let a = decode_activity_v1(&v)?;
            if let Some(g) = group {
                if group_for_kind(a.kind) != g {
                    continue;
                }
            }
            // sanity: if filtering, enforce it (robust even if very old index existed)
            if let Some(sb) = fixed_side {
                if !matches!(a.kind, ActivityKind::TradeBuy | ActivityKind::TradeSell) {
                    continue;
                }
                let actual = match chosen_side {
                    PriceSide::Base => side_code(a.base_delta),
                    PriceSide::Quote => side_code(a.quote_delta),
                };
                if actual != sb {
                    continue;
                }
            }
            rows.push(ActivityRow::from_storage(&a, chosen_side));
        }
    }

    Ok(ActivityPage { activity: rows, total })
}
