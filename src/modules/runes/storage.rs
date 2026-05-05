use crate::config::get_address_index_chunk_size;
use crate::runtime::mdb::Mdb;
use anyhow::{Result, anyhow};
use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, ScriptBuf, Txid};
use borsh::{BorshDeserialize, BorshSerialize};
use ordinals::{Rune, RuneId, SpacedRune, Terms};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use super::inscriptions::RuneIcon;

const INDEX_HEIGHT_KEY: &[u8] = b"/index_height";
const INDEX_BLOCK_HASH_KEY: &[u8] = b"/index_block_hash";
const UNDO_PREFIX: &[u8] = b"/undo/";
const UNCOMMON_GOODS_AVG_PRICE_USD_BY_HEIGHT_PREFIX: &[u8] =
    b"/block_price/v2/uncommon_goods_avg_usd_by_height/";
const TX_INDEX_INLINE_CAP: usize = 8;

// Raw Runes writes bypass the versioned B+tree for speed, so every block stores
// the pre-image of each touched key. Reorg handling replays these records backward.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
struct RunesBlockUndo {
    height: u32,
    block_hash: [u8; 32],
    prev_index_height: Option<u32>,
    prev_index_block_hash: Option<[u8; 32]>,
    changes: Vec<RuneUndoChange>,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
struct RuneUndoChange {
    key: Vec<u8>,
    previous: Option<Vec<u8>>,
}

#[derive(
    BorshSerialize,
    BorshDeserialize,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
)]
pub struct SchemaRuneId {
    pub block: u64,
    pub tx: u32,
}

impl From<RuneId> for SchemaRuneId {
    fn from(value: RuneId) -> Self {
        Self { block: value.block, tx: value.tx }
    }
}

impl From<SchemaRuneId> for RuneId {
    fn from(value: SchemaRuneId) -> Self {
        Self { block: value.block, tx: value.tx }
    }
}

impl std::fmt::Display for SchemaRuneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.block, self.tx)
    }
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct SchemaRuneTerms {
    pub amount: Option<u128>,
    pub cap: Option<u128>,
    pub height_start: Option<u64>,
    pub height_end: Option<u64>,
    pub offset_start: Option<u64>,
    pub offset_end: Option<u64>,
}

impl From<Terms> for SchemaRuneTerms {
    fn from(value: Terms) -> Self {
        Self {
            amount: value.amount,
            cap: value.cap,
            height_start: value.height.0,
            height_end: value.height.1,
            offset_start: value.offset.0,
            offset_end: value.offset.1,
        }
    }
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct RuneEntry {
    pub id: SchemaRuneId,
    pub rune: u128,
    pub name: String,
    pub spaced_name: String,
    pub spacers: u32,
    pub symbol: Option<String>,
    pub divisibility: u8,
    pub premine: u128,
    pub burned: u128,
    pub mints: u128,
    pub terms: Option<SchemaRuneTerms>,
    pub etching_txid: [u8; 32],
    pub number: u64,
    pub timestamp: u64,
    pub turbo: bool,
}

impl RuneEntry {
    pub fn mintable(&self, height: u64, tx_index: u32) -> Option<u128> {
        let terms = self.terms.as_ref()?;
        let amount = terms.amount?;
        let creation_height =
            if self.id == (SchemaRuneId { block: 1, tx: 0 }) { 840_000 } else { self.id.block };
        if height == creation_height && tx_index == self.id.tx {
            return None;
        }
        if let Some(start) = self.start_height() {
            if height < start {
                return None;
            }
        }
        if let Some(end) = self.end_height() {
            if height > end {
                return None;
            }
        }
        let cap = terms.cap.unwrap_or_default();
        if self.mints >= cap {
            return None;
        }
        Some(amount)
    }

    pub fn supply(&self) -> u128 {
        self.premine
            + self.mints * self.terms.as_ref().and_then(|terms| terms.amount).unwrap_or_default()
    }

    fn start_height(&self) -> Option<u64> {
        let terms = self.terms.as_ref()?;
        let relative = terms.offset_start.map(|offset| self.id.block.saturating_add(offset));
        let absolute = terms.height_start;
        relative.zip(absolute).map(|(a, b)| a.max(b)).or(relative).or(absolute)
    }

    fn end_height(&self) -> Option<u64> {
        let terms = self.terms.as_ref()?;
        let relative = terms.offset_end.map(|offset| self.id.block.saturating_add(offset));
        let absolute = terms.height_end;
        relative.zip(absolute).map(|(a, b)| a.min(b)).or(relative).or(absolute)
    }
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct RuneBalance {
    pub id: SchemaRuneId,
    pub amount: u128,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct OutpointRuneBalances {
    pub address: Option<String>,
    pub script_pubkey: Vec<u8>,
    pub balances: Vec<RuneBalance>,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct RuneMintActivity {
    pub id: SchemaRuneId,
    pub txid: [u8; 32],
    pub chain_txids: Vec<[u8; 32]>,
    pub cpfp: bool,
    pub height: u32,
    pub tx_index: u32,
    pub timestamp: u64,
    pub amount: u128,
    pub fee_paid_sats: u128,
    pub mint_price_paid_sats: [u8; 32],
    pub mint_price_paid_usd: [u8; 32],
    pub destination: Option<String>,
    pub success: bool,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuneActivityKind {
    Etch,
    Mint,
}

impl RuneActivityKind {
    pub fn key(self) -> &'static str {
        match self {
            Self::Etch => "etch",
            Self::Mint => "mint",
        }
    }
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct RuneActivity {
    pub id: SchemaRuneId,
    pub txid: [u8; 32],
    pub chain_txids: Vec<[u8; 32]>,
    pub cpfp: bool,
    pub height: u32,
    pub tx_index: u32,
    pub timestamp: u64,
    pub kind: RuneActivityKind,
    pub amount: u128,
    pub fee_paid_sats: u128,
    pub mint_price_paid_sats: [u8; 32],
    pub mint_price_paid_usd: [u8; 32],
    pub destination: Option<String>,
    pub success: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuneActivityScope {
    All,
    Market,
    Mint,
    Etch,
}

impl RuneActivityScope {
    fn segment(self) -> &'static [u8] {
        match self {
            Self::All => b"all",
            Self::Market => b"market",
            Self::Mint => b"mint",
            Self::Etch => b"etch",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuneActivitySortField {
    Timestamp,
    Amount,
}

impl RuneActivitySortField {
    fn segment(self) -> &'static [u8] {
        match self {
            Self::Timestamp => b"timestamp",
            Self::Amount => b"amount",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDir {
    Desc,
    Asc,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GetRuneActivityPageParams {
    pub id: SchemaRuneId,
    pub address: Option<String>,
    pub offset: usize,
    pub limit: usize,
    pub kind: Option<RuneActivityKind>,
    pub scope: RuneActivityScope,
    pub sort_by: RuneActivitySortField,
    pub dir: SortDir,
    pub start_time: Option<u64>,
    pub end_time: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GetRuneAddressActivityPageParams {
    pub address: String,
    pub id: Option<SchemaRuneId>,
    pub offset: usize,
    pub limit: usize,
    pub kind: Option<RuneActivityKind>,
    pub scope: RuneActivityScope,
    pub sort_by: RuneActivitySortField,
    pub dir: SortDir,
    pub start_time: Option<u64>,
    pub end_time: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuneActivityPage {
    pub total: usize,
    pub entries: Vec<RuneActivity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuneBalanceHistoryPoint {
    pub height: u32,
    pub amount: u128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuneAddressAmountEntry {
    pub address: String,
    pub amount: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RuneVolumeKind {
    TransferVolume,
    TotalReceived,
}

impl RuneVolumeKind {
    fn segment(self) -> &'static [u8] {
        match self {
            Self::TransferVolume => b"transfer",
            Self::TotalReceived => b"received",
        }
    }
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct TxRuneIo {
    pub inputs: BTreeMap<u32, Vec<RuneBalance>>,
    pub outputs: BTreeMap<u32, Vec<RuneBalance>>,
    pub burned: Vec<RuneBalance>,
    pub minted: Vec<RuneBalance>,
    pub etched: Option<SchemaRuneId>,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct RuneTxPointerBlob {
    pub txid: [u8; 32],
    pub height: u32,
    pub tx_index: u32,
    pub io: TxRuneIo,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
enum RuneTxIndexList {
    Inline { items: Vec<u64> },
    External { chunk_ids: Vec<u64>, len: u64, chunk_size: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuneTxIndexKind {
    Block,
    Address,
    ActionBlock,
    ActionAddress,
}

impl RuneTxIndexKind {
    fn segment(self) -> &'static [u8] {
        match self {
            Self::Block => b"block",
            Self::Address => b"address",
            Self::ActionBlock => b"action_block",
            Self::ActionAddress => b"action_address",
        }
    }
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct ActionTxPointerBlob {
    pub txid: [u8; 32],
    pub height: u32,
    pub tx_index: u32,
    pub has_alkane: bool,
    pub has_rune: bool,
}

#[derive(Clone)]
pub struct RunesProvider {
    mdb: Arc<Mdb>,
}

impl RunesProvider {
    pub fn new(mdb: Arc<Mdb>) -> Self {
        Self { mdb }
    }

    pub fn mdb(&self) -> &Mdb {
        self.mdb.as_ref()
    }

    pub fn get_index_height(&self) -> Result<Option<u32>> {
        Ok(self.mdb.get(INDEX_HEIGHT_KEY)?.and_then(|bytes| {
            (bytes.len() == 4).then(|| {
                let mut arr = [0u8; 4];
                arr.copy_from_slice(&bytes);
                u32::from_le_bytes(arr)
            })
        }))
    }

    pub fn get_index_block_hash(&self) -> Result<Option<BlockHash>> {
        Ok(self.mdb.get(INDEX_BLOCK_HASH_KEY)?.and_then(|bytes| {
            (bytes.len() == 32).then(|| {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                BlockHash::from_byte_array(arr)
            })
        }))
    }

    pub fn get_uncommon_goods_avg_price_paid_usd_by_height(
        &self,
        height: u32,
    ) -> Result<Option<[u8; 32]>> {
        let Some(bytes) = self.mdb.get(&uncommon_goods_avg_price_usd_by_height_key(height))? else {
            return Ok(None);
        };
        if bytes.len() != 32 {
            return Err(anyhow!("invalid uncommon goods avg usd price length {}", bytes.len()));
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        Ok(Some(out))
    }

    pub fn get_uncommon_goods_avg_price_paid_usd_points_through_height(
        &self,
        max_height: u32,
    ) -> Result<Vec<(u32, [u8; 32])>> {
        let prefix = uncommon_goods_avg_price_usd_by_height_prefix();
        let entries = self.mdb.scan_prefix_entries(&prefix)?;
        let mut out = Vec::new();
        for (key, value) in entries {
            let Some(height) = key.strip_prefix(prefix.as_slice()).and_then(decode_height_tail)
            else {
                continue;
            };
            if height > max_height {
                continue;
            }
            if value.len() != 32 {
                continue;
            }
            let mut price = [0u8; 32];
            price.copy_from_slice(&value);
            out.push((height, price));
        }
        out.sort_by_key(|(height, _)| *height);
        Ok(out)
    }

    pub fn set_index_height(&self, height: u32) -> Result<()> {
        self.mdb.put(INDEX_HEIGHT_KEY, &height.to_le_bytes())?;
        Ok(())
    }

    pub fn set_batch(&self, puts: Vec<(Vec<u8>, Vec<u8>)>, deletes: Vec<Vec<u8>>) -> Result<()> {
        self.mdb.bulk_write(|wb| {
            for key in deletes {
                wb.delete(&key);
            }
            for (key, value) in puts {
                wb.put(&key, &value);
            }
        })?;
        Ok(())
    }

    pub fn set_block_batch(
        &self,
        puts: Vec<(Vec<u8>, Vec<u8>)>,
        deletes: Vec<Vec<u8>>,
        index_height: u32,
        block_hash: &BlockHash,
    ) -> Result<()> {
        let t0 = Instant::now();
        let put_count = puts.len();
        let delete_count = deletes.len();
        let put_bytes: usize = puts.iter().map(|(key, value)| key.len() + value.len()).sum();
        let delete_bytes: usize = deletes.iter().map(|key| key.len()).sum();
        let mut read_keys = HashSet::new();
        let mut append_only_put_keys = Vec::new();
        for key in &deletes {
            read_keys.insert(key.clone());
        }
        for (key, _) in &puts {
            if runes_put_key_requires_preimage(key) {
                read_keys.insert(key.clone());
            } else {
                append_only_put_keys.push(key.clone());
            }
        }
        append_only_put_keys.sort();
        append_only_put_keys.dedup();
        let mut read_keys: Vec<Vec<u8>> = read_keys.into_iter().collect();
        read_keys.sort();
        let read_key_count = read_keys.len();

        let t_read = Instant::now();
        let previous_values = self.mdb.multi_get(&read_keys)?;
        let read_elapsed = t_read.elapsed();
        let mut changes = read_keys
            .into_iter()
            .zip(previous_values)
            .map(|(key, previous)| RuneUndoChange { key, previous })
            .collect::<Vec<_>>();
        changes.extend(
            append_only_put_keys
                .into_iter()
                .map(|key| RuneUndoChange { key, previous: None }),
        );
        let touched_count = changes.len();
        let prev_index_height = self.get_index_height()?;
        let prev_index_block_hash = self.get_index_block_hash()?.map(|hash| hash.to_byte_array());
        let block_hash_bytes = block_hash.to_byte_array();
        let undo = RunesBlockUndo {
            height: index_height,
            block_hash: block_hash_bytes,
            prev_index_height,
            prev_index_block_hash,
            changes,
        };
        let t_encode = Instant::now();
        let undo_value = encode(&undo)?;
        let encode_elapsed = t_encode.elapsed();
        let undo_bytes = undo_value.len();

        let t_write = Instant::now();
        self.mdb.bulk_write(|wb| {
            for key in deletes {
                wb.delete(&key);
            }
            for (key, value) in puts {
                wb.put(&key, &value);
            }
            wb.put(INDEX_HEIGHT_KEY, &index_height.to_le_bytes());
            wb.put(INDEX_BLOCK_HASH_KEY, &block_hash_bytes);
            wb.put(&undo_key(index_height), &undo_value);
        })?;
        if crate::config::debug_enabled() {
            eprintln!(
                "[RUNES][storage] height={} puts={} deletes={} touched={} preimage_keys={} put_bytes={} delete_key_bytes={} undo_bytes={} preimage_read={:?} undo_encode={:?} write={:?} total={:?}",
                index_height,
                put_count,
                delete_count,
                touched_count,
                read_key_count,
                put_bytes,
                delete_bytes,
                undo_bytes,
                read_elapsed,
                encode_elapsed,
                t_write.elapsed(),
                t0.elapsed()
            );
        }
        Ok(())
    }

    pub fn has_undo_for_height(&self, height: u32) -> Result<bool> {
        Ok(self.mdb.get(&undo_key(height))?.is_some())
    }

    pub fn rollback_before_height(&self, next_height: u32) -> Result<()> {
        self.rollback_to_height(next_height.checked_sub(1))
    }

    fn rollback_to_height(&self, target_height: Option<u32>) -> Result<()> {
        loop {
            let Some(current_height) = self.get_index_height()? else {
                return Ok(());
            };
            if let Some(target_height) = target_height {
                if current_height <= target_height {
                    return Ok(());
                }
            }

            let key = undo_key(current_height);
            let Some(undo_bytes) = self.mdb.get(&key)? else {
                return Err(anyhow!(
                    "runes rollback missing undo journal for height {current_height}; full runes reindex required"
                ));
            };
            let undo = RunesBlockUndo::try_from_slice(&undo_bytes).map_err(|e| {
                anyhow!("failed to decode runes undo journal at {current_height}: {e}")
            })?;
            if undo.height != current_height {
                return Err(anyhow!(
                    "runes rollback journal height mismatch: key height {current_height}, record height {}",
                    undo.height
                ));
            }
            if let Some(current_hash) = self.get_index_block_hash()? {
                if current_hash.to_byte_array() != undo.block_hash {
                    return Err(anyhow!(
                        "runes rollback journal hash mismatch at height {current_height}: index hash {}, journal hash {}",
                        current_hash,
                        BlockHash::from_byte_array(undo.block_hash)
                    ));
                }
            }

            self.mdb.bulk_write(|wb| {
                for change in undo.changes {
                    match change.previous {
                        Some(value) => wb.put(&change.key, &value),
                        None => wb.delete(&change.key),
                    }
                }
                match undo.prev_index_height {
                    Some(height) => wb.put(INDEX_HEIGHT_KEY, &height.to_le_bytes()),
                    None => wb.delete(INDEX_HEIGHT_KEY),
                }
                match undo.prev_index_block_hash {
                    Some(hash) => wb.put(INDEX_BLOCK_HASH_KEY, &hash),
                    None => wb.delete(INDEX_BLOCK_HASH_KEY),
                }
                wb.delete(&key);
            })?;
        }
    }

    pub fn clear_namespace(&self) -> Result<usize> {
        let keys = self.mdb.scan_prefix_keys(b"")?;
        let count = keys.len();
        self.mdb.bulk_write(|wb| {
            for key in keys {
                wb.delete(&key);
            }
        })?;
        Ok(count)
    }

    pub fn get_rune(&self, id: SchemaRuneId) -> Result<Option<RuneEntry>> {
        self.get_entry(&entry_key(id))
    }

    pub fn get_rune_by_query(&self, query: &str) -> Result<Option<RuneEntry>> {
        if let Some(id) = parse_rune_id(query) {
            return self.get_rune(id);
        }
        let key = id_by_name_key(&normalize_name(query));
        let Some(raw) = self.mdb.get(&key)? else {
            return Ok(None);
        };
        let id = SchemaRuneId::try_from_slice(&raw)?;
        self.get_rune(id)
    }

    pub fn get_runes_by_name_prefix(&self, query: &str, limit: usize) -> Result<Vec<RuneEntry>> {
        let normalized = normalize_name(query);
        if normalized.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let prefix = id_by_name_key(&normalized);
        let mut seen = HashSet::new();
        let mut rows = Vec::new();
        for (key, raw) in self.mdb.scan_prefix_entries(&prefix)? {
            if key.len() <= prefix.len() {
                continue;
            }
            let id = SchemaRuneId::try_from_slice(&raw)?;
            if !seen.insert(id) {
                continue;
            }
            if let Some(entry) = self.get_rune(id)? {
                rows.push(entry);
                if rows.len() >= limit {
                    break;
                }
            }
        }
        Ok(rows)
    }

    pub fn get_outpoint_balances(
        &self,
        txid: &Txid,
        vout: u32,
    ) -> Result<Option<OutpointRuneBalances>> {
        self.get_entry(&outpoint_key(txid, vout))
    }

    pub fn get_address_outpoints(
        &self,
        address: &str,
    ) -> Result<Vec<(Txid, u32, OutpointRuneBalances)>> {
        let mut rows = Vec::new();
        for key in self.mdb.scan_prefix_keys(&address_outpoint_prefix(address))? {
            let Some((txid, vout)) = decode_address_outpoint_key(address, &key) else {
                continue;
            };
            let Some(balances) = self.get_outpoint_balances(&txid, vout)? else {
                continue;
            };
            rows.push((txid, vout, balances));
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        Ok(rows)
    }

    pub fn get_live_outpoints(&self) -> Result<HashSet<(Txid, u32)>> {
        let mut out = HashSet::new();
        for key in self.mdb.scan_prefix_keys(b"/outpoint/")? {
            if let Some(outpoint) = decode_outpoint_key(&key) {
                out.insert(outpoint);
            }
        }
        Ok(out)
    }

    pub fn get_holders(
        &self,
        id: SchemaRuneId,
        page: usize,
        limit: usize,
    ) -> Result<Vec<(String, u128)>> {
        let prefix = holder_prefix(id);
        let mut rows: Vec<(String, u128)> = Vec::new();
        for item in self.mdb.scan_prefix_entries(&prefix)? {
            let (key, value) = item;
            let address = String::from_utf8_lossy(&key[prefix.len()..]).to_string();
            let amount = decode_u128(&value).unwrap_or(0);
            if amount > 0 {
                rows.push((address, amount));
            }
        }
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let start = limit.saturating_mul(page.saturating_sub(1));
        Ok(rows.into_iter().skip(start).take(limit).collect())
    }

    pub fn get_top_runes(&self, page: usize, limit: usize) -> Result<Vec<(RuneEntry, u64)>> {
        self.get_runes_by_holders(page, limit, true)
    }

    pub fn get_runes_by_holders(
        &self,
        page: usize,
        limit: usize,
        desc: bool,
    ) -> Result<Vec<(RuneEntry, u64)>> {
        let mut rows = Vec::new();
        for item in self.mdb.scan_prefix_entries(b"/rune/by_id/")? {
            let (_key, value) = item;
            let entry = RuneEntry::try_from_slice(&value)?;
            let holders = self.get_holders_count(entry.id)?;
            rows.push((entry, holders));
        }
        rows.sort_by(|a, b| {
            let ord = a.1.cmp(&b.1).then_with(|| b.0.number.cmp(&a.0.number));
            if desc { ord.reverse() } else { ord }
        });
        let start = limit.saturating_mul(page.saturating_sub(1));
        Ok(rows.into_iter().skip(start).take(limit).collect())
    }

    pub fn get_runes_by_age(
        &self,
        page: usize,
        limit: usize,
        desc: bool,
    ) -> Result<Vec<(RuneEntry, u64)>> {
        let mut rows = Vec::new();
        for item in self.mdb.scan_prefix_entries(b"/rune/by_id/")? {
            let (_key, value) = item;
            let entry = RuneEntry::try_from_slice(&value)?;
            let holders = self.get_holders_count(entry.id)?;
            rows.push((entry, holders));
        }
        rows.sort_by(|a, b| {
            let ord =
                a.0.id
                    .block
                    .cmp(&b.0.id.block)
                    .then_with(|| a.0.id.tx.cmp(&b.0.id.tx))
                    .then_with(|| a.0.number.cmp(&b.0.number));
            if desc { ord.reverse() } else { ord }
        });
        let start = limit.saturating_mul(page.saturating_sub(1));
        Ok(rows.into_iter().skip(start).take(limit).collect())
    }

    pub fn get_holders_count(&self, id: SchemaRuneId) -> Result<u64> {
        Ok(self.mdb.get(&holders_count_key(id))?.and_then(|v| decode_u64(&v)).unwrap_or(0))
    }

    pub fn get_rune_count(&self) -> Result<u64> {
        Ok(self.mdb.scan_prefix_keys(b"/rune/by_id/")?.len() as u64)
    }

    pub fn get_address_balances(&self, address: &str) -> Result<Vec<(SchemaRuneId, u128)>> {
        let prefix = address_balance_prefix(address);
        let mut out = Vec::new();
        for item in self.mdb.scan_prefix_entries(&prefix)? {
            let (key, value) = item;
            let Some(id) = decode_id_key_tail(&key[prefix.len()..]) else {
                continue;
            };
            let amount = decode_u128(&value).unwrap_or(0);
            if amount > 0 {
                out.push((id, amount));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    pub fn get_address_balance_history_points(
        &self,
        address: &str,
        id: SchemaRuneId,
        range_min: u32,
        range_max: u32,
        interval: u32,
    ) -> Result<Vec<RuneBalanceHistoryPoint>> {
        if range_min > range_max {
            return Ok(Vec::new());
        }
        let len = self
            .mdb
            .get(&address_balance_history_list_len_key(address, id))?
            .and_then(|bytes| decode_u32(&bytes))
            .unwrap_or(0);
        if len == 0 {
            return Ok(Vec::new());
        }

        let height_keys: Vec<Vec<u8>> = (0..len)
            .map(|idx| address_balance_history_list_idx_key(address, id, idx))
            .collect();
        let height_values = self.mdb.multi_get(&height_keys)?;
        let mut heights = Vec::new();
        for raw in height_values.into_iter().flatten() {
            if raw.len() == 4 {
                heights.push(u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]));
            }
        }
        heights.retain(|height| *height <= range_max);
        heights.sort_unstable();
        heights.dedup();
        if heights.is_empty() {
            return Ok(Vec::new());
        }

        let value_keys: Vec<Vec<u8>> = heights
            .iter()
            .map(|height| address_balance_history_key(address, id, *height))
            .collect();
        let value_rows = self.mdb.multi_get(&value_keys)?;
        let mut changes = Vec::new();
        for (height, raw) in heights.into_iter().zip(value_rows.into_iter()) {
            let Some(bytes) = raw else { continue };
            let amount = decode_u128(&bytes).unwrap_or(0);
            changes.push((height, amount));
        }
        changes.sort_by_key(|(height, _)| *height);
        changes.dedup_by_key(|(height, _)| *height);
        if changes.is_empty() {
            return Ok(Vec::new());
        }

        let mut idx = 0usize;
        let mut current = 0u128;
        while idx < changes.len() && changes[idx].0 <= range_min {
            current = changes[idx].1;
            idx += 1;
        }

        let step = interval.max(1);
        let mut height = range_min;
        let mut points = Vec::new();
        loop {
            while idx < changes.len() && changes[idx].0 <= height {
                current = changes[idx].1;
                idx += 1;
            }
            points.push(RuneBalanceHistoryPoint { height, amount: current });
            if height == range_max {
                break;
            }
            height = height.saturating_add(step).min(range_max);
        }
        Ok(points)
    }

    pub fn get_mint_activity(
        &self,
        id: SchemaRuneId,
        page: usize,
        limit: usize,
    ) -> Result<Vec<RuneMintActivity>> {
        let prefix = mint_activity_prefix(id);
        let end = prefix_end_exclusive(&prefix);
        let offset = limit.saturating_mul(page.saturating_sub(1));
        let mut rows = Vec::new();
        for item in
            self.mdb.scan_range_entries_page(&prefix, end.as_deref(), offset, limit, true)?
        {
            let (_key, value) = item;
            rows.push(RuneMintActivity::try_from_slice(&value)?);
        }
        Ok(rows)
    }

    pub fn get_volume(
        &self,
        id: SchemaRuneId,
        kind: RuneVolumeKind,
        page: usize,
        limit: usize,
    ) -> Result<(usize, Vec<RuneAddressAmountEntry>)> {
        let len = self
            .mdb
            .get(&rune_volume_list_len_key(kind, id))?
            .and_then(|bytes| decode_u32(&bytes))
            .unwrap_or(0);
        let mut rows = Vec::new();
        if len > 0 {
            for idx in 0..len {
                let Some(raw_addr) = self.mdb.get(&rune_volume_list_idx_key(kind, id, idx))? else {
                    continue;
                };
                let Ok(address) = std::str::from_utf8(&raw_addr).map(|s| s.to_string()) else {
                    continue;
                };
                let amount = self
                    .mdb
                    .get(&rune_volume_entry_key(kind, id, &address))?
                    .and_then(|bytes| decode_u128(&bytes))
                    .unwrap_or(0);
                if amount > 0 {
                    rows.push(RuneAddressAmountEntry { address, amount });
                }
            }
        }
        rows.sort_by(|a, b| b.amount.cmp(&a.amount).then_with(|| a.address.cmp(&b.address)));
        let total = rows.len();
        let start = limit.saturating_mul(page.saturating_sub(1));
        Ok((total, rows.into_iter().skip(start).take(limit).collect()))
    }

    pub fn get_rune_activity_page(
        &self,
        params: GetRuneActivityPageParams,
    ) -> Result<RuneActivityPage> {
        let prefix = match params.address.as_ref() {
            Some(address) => rune_address_token_activity_index_prefix(
                address,
                params.id,
                params.scope,
                params.sort_by,
            ),
            None => rune_activity_index_prefix(params.id, params.scope, params.sort_by),
        };
        let total_hint = if params.address.is_none() {
            self.rune_activity_total_hint(params.id, params.scope)?
        } else {
            None
        };
        self.get_rune_activity_page_by_prefix(
            prefix,
            params.offset,
            params.limit,
            params.dir,
            total_hint,
            params.kind,
            params.start_time,
            params.end_time,
        )
    }

    pub fn get_rune_address_activity_page(
        &self,
        params: GetRuneAddressActivityPageParams,
    ) -> Result<RuneActivityPage> {
        let prefix = match params.id {
            Some(id) => rune_address_token_activity_index_prefix(
                &params.address,
                id,
                params.scope,
                params.sort_by,
            ),
            None => {
                rune_address_activity_index_prefix(&params.address, params.scope, params.sort_by)
            }
        };
        self.get_rune_activity_page_by_prefix(
            prefix,
            params.offset,
            params.limit,
            params.dir,
            None,
            params.kind,
            params.start_time,
            params.end_time,
        )
    }

    fn get_rune_activity_page_by_prefix(
        &self,
        prefix: Vec<u8>,
        offset: usize,
        limit: usize,
        dir: SortDir,
        total_hint: Option<usize>,
        kind: Option<RuneActivityKind>,
        start_time: Option<u64>,
        end_time: Option<u64>,
    ) -> Result<RuneActivityPage> {
        let end = prefix_end_exclusive(&prefix);
        if kind.is_some() || start_time.is_some() || end_time.is_some() {
            let mut entries = Vec::new();
            for (_key, raw) in self.mdb.scan_prefix_entries(&prefix)? {
                let activity = RuneActivity::try_from_slice(&raw)?;
                if kind.map(|needle| activity.kind == needle).unwrap_or(true)
                    && start_time.map(|start| activity.timestamp >= start).unwrap_or(true)
                    && end_time.map(|end| activity.timestamp <= end).unwrap_or(true)
                {
                    entries.push(activity);
                }
            }
            if matches!(dir, SortDir::Desc) {
                entries.reverse();
            }
            let total = entries.len();
            let entries = entries.into_iter().skip(offset).take(limit).collect();
            return Ok(RuneActivityPage { total, entries });
        }
        let selected = self.mdb.scan_range_entries_page(
            &prefix,
            end.as_deref(),
            offset,
            limit,
            matches!(dir, SortDir::Desc),
        )?;
        let mut entries = Vec::new();
        for (_key, raw) in selected {
            entries.push(RuneActivity::try_from_slice(&raw)?);
        }
        let total = total_hint.unwrap_or_else(|| {
            self.mdb
                .scan_prefix_keys(&prefix)
                .map(|keys| keys.len())
                .unwrap_or(offset + entries.len())
        });
        Ok(RuneActivityPage { total, entries })
    }

    fn rune_activity_total_hint(
        &self,
        id: SchemaRuneId,
        scope: RuneActivityScope,
    ) -> Result<Option<usize>> {
        let Some(entry) = self.get_rune(id)? else {
            return Ok(Some(0));
        };
        let total = match scope {
            RuneActivityScope::All => entry.mints.saturating_add(1),
            RuneActivityScope::Mint => entry.mints,
            RuneActivityScope::Etch => 1,
            RuneActivityScope::Market => 0,
        };
        Ok(Some(total.min(usize::MAX as u128) as usize))
    }

    pub fn get_tx_io(&self, txid: &Txid) -> Result<Option<TxRuneIo>> {
        self.get_entry(&tx_io_key(txid))
    }

    pub fn get_block_tx_count(&self, height: u64) -> Result<u64> {
        self.get_tx_index_len(&rune_tx_block_list_key(height))
    }

    pub fn get_block_tx_range(
        &self,
        height: u64,
        start: u64,
        end: u64,
    ) -> Result<Vec<RuneTxPointerBlob>> {
        self.get_tx_index_pointer_range(
            RuneTxIndexKind::Block,
            &rune_tx_block_list_key(height),
            start,
            end,
        )
    }

    pub fn get_address_tx_count(&self, address: &str) -> Result<u64> {
        self.get_tx_index_len(&rune_tx_address_list_key(address))
    }

    pub fn get_address_tx_range(
        &self,
        address: &str,
        start: u64,
        end: u64,
    ) -> Result<Vec<RuneTxPointerBlob>> {
        self.get_tx_index_pointer_range(
            RuneTxIndexKind::Address,
            &rune_tx_address_list_key(address),
            start,
            end,
        )
    }

    pub fn get_action_block_tx_count(&self, height: u64) -> Result<u64> {
        self.get_tx_index_len(&action_tx_block_list_key(height))
    }

    pub fn get_action_block_tx_range(
        &self,
        height: u64,
        start: u64,
        end: u64,
    ) -> Result<Vec<ActionTxPointerBlob>> {
        self.get_action_tx_index_pointer_range(
            RuneTxIndexKind::ActionBlock,
            &action_tx_block_list_key(height),
            start,
            end,
        )
    }

    pub fn get_action_address_tx_count(&self, address: &str) -> Result<u64> {
        self.get_tx_index_len(&action_tx_address_list_key(address))
    }

    pub fn get_action_address_tx_range(
        &self,
        address: &str,
        start: u64,
        end: u64,
    ) -> Result<Vec<ActionTxPointerBlob>> {
        self.get_action_tx_index_pointer_range(
            RuneTxIndexKind::ActionAddress,
            &action_tx_address_list_key(address),
            start,
            end,
        )
    }

    pub fn get_rune_icon(&self, id: SchemaRuneId) -> Result<Option<RuneIcon>> {
        self.get_entry(&rune_icon_key(id))
    }

    fn get_tx_index_len(&self, key: &[u8]) -> Result<u64> {
        let Some(raw) = self.mdb.get(key)? else {
            return Ok(0);
        };
        let state = RuneTxIndexList::try_from_slice(&raw)?;
        Ok(rune_tx_index_total(&state))
    }

    fn get_tx_index_pointer_range(
        &self,
        kind: RuneTxIndexKind,
        list_key: &[u8],
        start: u64,
        end: u64,
    ) -> Result<Vec<RuneTxPointerBlob>> {
        let pointer_ids = self.get_tx_index_range(kind, list_key, start, end)?;
        let mut out = Vec::with_capacity(pointer_ids.len());
        for id in pointer_ids {
            if let Some(blob) = self.get_entry::<RuneTxPointerBlob>(&rune_tx_pointer_key(id))? {
                out.push(blob);
            }
        }
        Ok(out)
    }

    fn get_action_tx_index_pointer_range(
        &self,
        kind: RuneTxIndexKind,
        list_key: &[u8],
        start: u64,
        end: u64,
    ) -> Result<Vec<ActionTxPointerBlob>> {
        let pointer_ids = self.get_tx_index_range(kind, list_key, start, end)?;
        let mut out = Vec::with_capacity(pointer_ids.len());
        for id in pointer_ids {
            if let Some(blob) = self.get_entry::<ActionTxPointerBlob>(&action_tx_pointer_key(id))? {
                out.push(blob);
            }
        }
        Ok(out)
    }

    fn get_tx_index_range(
        &self,
        kind: RuneTxIndexKind,
        list_key: &[u8],
        start: u64,
        end: u64,
    ) -> Result<Vec<u64>> {
        if end <= start {
            return Ok(Vec::new());
        }
        let Some(raw) = self.mdb.get(list_key)? else {
            return Ok(Vec::new());
        };
        let state = RuneTxIndexList::try_from_slice(&raw)?;
        let total = rune_tx_index_total(&state);
        let start = start.min(total);
        let end = end.min(total);
        if end <= start {
            return Ok(Vec::new());
        }

        match state {
            RuneTxIndexList::Inline { items } => {
                let from = usize::try_from(start).unwrap_or(usize::MAX).min(items.len());
                let to = usize::try_from(end).unwrap_or(usize::MAX).min(items.len());
                Ok(items[from..to].to_vec())
            }
            RuneTxIndexList::External { chunk_ids, chunk_size, .. } => {
                let chunk_size_u64 = u64::from(chunk_size.max(1));
                let first_chunk = usize::try_from(start / chunk_size_u64).unwrap_or(usize::MAX);
                let mut last_chunk_excl =
                    usize::try_from((end + chunk_size_u64 - 1) / chunk_size_u64)
                        .unwrap_or(usize::MAX);
                if first_chunk >= chunk_ids.len() {
                    return Ok(Vec::new());
                }
                last_chunk_excl = last_chunk_excl.min(chunk_ids.len());

                let mut out =
                    Vec::with_capacity(usize::try_from(end.saturating_sub(start)).unwrap_or(0));
                for (offset, id) in chunk_ids[first_chunk..last_chunk_excl].iter().enumerate() {
                    let Some(raw_chunk) = self.mdb.get(&rune_tx_chunk_key(kind, *id))? else {
                        continue;
                    };
                    let items = decode_u64_chunk(&raw_chunk);
                    let global_chunk_idx = first_chunk.saturating_add(offset);
                    let chunk_start = (global_chunk_idx as u64).saturating_mul(chunk_size_u64);
                    let from = usize::try_from(start.saturating_sub(chunk_start))
                        .unwrap_or(usize::MAX)
                        .min(items.len());
                    let to = usize::try_from(end.saturating_sub(chunk_start))
                        .unwrap_or(usize::MAX)
                        .min(items.len());
                    if to > from {
                        out.extend_from_slice(&items[from..to]);
                    }
                }
                Ok(out)
            }
        }
    }

    fn get_entry<T: BorshDeserialize>(&self, key: &[u8]) -> Result<Option<T>> {
        self.mdb
            .get(key)?
            .map(|bytes| T::try_from_slice(&bytes).map_err(|e| anyhow!("runes decode failed: {e}")))
            .transpose()
    }
}

pub fn rune_tx_pointer_count_key() -> Vec<u8> {
    b"/tx_index/pointer/count".to_vec()
}

pub fn action_tx_pointer_count_key() -> Vec<u8> {
    b"/tx_index/actions/pointer/count".to_vec()
}

pub fn rune_tx_pointer_key(id: u64) -> Vec<u8> {
    let mut key = b"/tx_index/pointer/".to_vec();
    key.extend_from_slice(&id.to_be_bytes());
    key
}

pub fn action_tx_pointer_key(id: u64) -> Vec<u8> {
    let mut key = b"/tx_index/actions/pointer/".to_vec();
    key.extend_from_slice(&id.to_be_bytes());
    key
}

pub fn rune_tx_block_list_key(height: u64) -> Vec<u8> {
    let mut key = b"/tx_index/block/".to_vec();
    key.extend_from_slice(&height.to_be_bytes());
    key
}

pub fn action_tx_block_list_key(height: u64) -> Vec<u8> {
    let mut key = b"/tx_index/actions/block/".to_vec();
    key.extend_from_slice(&height.to_be_bytes());
    key
}

pub fn rune_tx_address_list_key(address: &str) -> Vec<u8> {
    let mut key = b"/tx_index/address/".to_vec();
    key.extend_from_slice(address.as_bytes());
    key
}

pub fn action_tx_address_list_key(address: &str) -> Vec<u8> {
    let mut key = b"/tx_index/actions/address/".to_vec();
    key.extend_from_slice(address.as_bytes());
    key
}

pub fn rune_tx_chunk_counter_key(kind: RuneTxIndexKind) -> Vec<u8> {
    let mut key = b"/tx_index/chunk_count/".to_vec();
    key.extend_from_slice(kind.segment());
    key
}

fn rune_tx_chunk_key(kind: RuneTxIndexKind, id: u64) -> Vec<u8> {
    let mut key = b"/tx_index/chunk/".to_vec();
    key.extend_from_slice(kind.segment());
    key.push(b'/');
    key.extend_from_slice(&id.to_be_bytes());
    key
}

pub fn encode_rune_tx_pointer_blob(
    txid: &Txid,
    height: u32,
    tx_index: u32,
    io: &TxRuneIo,
) -> Result<Vec<u8>> {
    encode(&RuneTxPointerBlob { txid: txid.to_byte_array(), height, tx_index, io: io.clone() })
}

pub fn encode_action_tx_pointer_blob(
    txid: &Txid,
    height: u32,
    tx_index: u32,
    has_alkane: bool,
    has_rune: bool,
) -> Result<Vec<u8>> {
    encode(&ActionTxPointerBlob {
        txid: txid.to_byte_array(),
        height,
        tx_index,
        has_alkane,
        has_rune,
    })
}

pub fn append_rune_tx_index_values(
    provider: &RunesProvider,
    kind: RuneTxIndexKind,
    list_key: Vec<u8>,
    values: &[u64],
    next_chunk_id: &mut u64,
    puts: &mut Vec<(Vec<u8>, Vec<u8>)>,
) -> Result<u64> {
    if values.is_empty() {
        return Ok(0);
    }
    let current = provider
        .mdb()
        .get(&list_key)?
        .map(|raw| RuneTxIndexList::try_from_slice(&raw))
        .transpose()?
        .unwrap_or_else(|| RuneTxIndexList::Inline { items: Vec::new() });

    let next_state = match current {
        RuneTxIndexList::Inline { mut items } => {
            if items.len().saturating_add(values.len()) <= TX_INDEX_INLINE_CAP {
                items.extend_from_slice(values);
                RuneTxIndexList::Inline { items }
            } else {
                let chunk_size = get_address_index_chunk_size().max(1);
                let mut merged = Vec::with_capacity(items.len().saturating_add(values.len()));
                merged.append(&mut items);
                merged.extend_from_slice(values);

                let mut chunk_ids = Vec::new();
                for chunk in merged.chunks(chunk_size) {
                    let id = *next_chunk_id;
                    *next_chunk_id = next_chunk_id.saturating_add(1);
                    chunk_ids.push(id);
                    puts.push((rune_tx_chunk_key(kind, id), encode_u64_chunk(chunk)?));
                }
                RuneTxIndexList::External {
                    chunk_ids,
                    len: merged.len() as u64,
                    chunk_size: chunk_size as u32,
                }
            }
        }
        RuneTxIndexList::External { mut chunk_ids, len, chunk_size } => {
            let chunk_size_usize = usize::try_from(chunk_size).unwrap_or(0).max(1);
            let chunk_size_u64 = chunk_size_usize as u64;
            let mut pending = values;

            if !chunk_ids.is_empty() && !pending.is_empty() {
                let rem = usize::try_from(len % chunk_size_u64).unwrap_or(0);
                if rem > 0 {
                    let last_chunk_id = *chunk_ids.last().unwrap_or(&0);
                    let last_key = rune_tx_chunk_key(kind, last_chunk_id);
                    let mut last_items = provider
                        .mdb()
                        .get(&last_key)?
                        .map(|raw| decode_u64_chunk(&raw))
                        .unwrap_or_default();
                    if last_items.len() > chunk_size_usize {
                        last_items.truncate(chunk_size_usize);
                    }
                    let free = chunk_size_usize.saturating_sub(last_items.len());
                    let take = free.min(pending.len());
                    if take > 0 {
                        last_items.extend_from_slice(&pending[..take]);
                        puts.push((last_key, encode_u64_chunk(&last_items)?));
                        pending = &pending[take..];
                    }
                }
            }

            while !pending.is_empty() {
                let take = chunk_size_usize.min(pending.len());
                let id = *next_chunk_id;
                *next_chunk_id = next_chunk_id.saturating_add(1);
                chunk_ids.push(id);
                puts.push((rune_tx_chunk_key(kind, id), encode_u64_chunk(&pending[..take])?));
                pending = &pending[take..];
            }

            RuneTxIndexList::External {
                chunk_ids,
                len: len.saturating_add(values.len() as u64),
                chunk_size: chunk_size_usize as u32,
            }
        }
    };

    let len = rune_tx_index_total(&next_state);
    puts.push((list_key, encode(&next_state)?));
    Ok(len)
}

fn rune_tx_index_total(state: &RuneTxIndexList) -> u64 {
    match state {
        RuneTxIndexList::Inline { items } => items.len() as u64,
        RuneTxIndexList::External { len, .. } => *len,
    }
}

fn encode_u64_chunk(values: &[u64]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(values.len().saturating_mul(8));
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    Ok(out)
}

fn decode_u64_chunk(raw: &[u8]) -> Vec<u64> {
    raw.chunks_exact(8)
        .map(|chunk| {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(chunk);
            u64::from_le_bytes(arr)
        })
        .collect()
}

pub fn rune_entry_to_json(entry: &RuneEntry, holders: u64) -> Value {
    json!({
        "id": entry.id.to_string(),
        "rune": entry.name,
        "spaced_rune": entry.spaced_name,
        "number": entry.number,
        "symbol": entry.symbol,
        "divisibility": entry.divisibility,
        "premine": entry.premine.to_string(),
        "mints": entry.mints.to_string(),
        "supply": entry.supply().to_string(),
        "burned": entry.burned.to_string(),
        "holders": holders,
        "etching_txid": Txid::from_byte_array(entry.etching_txid).to_string(),
        "timestamp": entry.timestamp,
        "turbo": entry.turbo,
        "terms": entry.terms.as_ref().map(|terms| json!({
            "amount": terms.amount.map(|v| v.to_string()),
            "cap": terms.cap.map(|v| v.to_string()),
            "height": [terms.height_start, terms.height_end],
            "offset": [terms.offset_start, terms.offset_end]
        }))
    })
}

pub fn make_entry(
    id: SchemaRuneId,
    rune: Rune,
    spacers: u32,
    symbol: Option<char>,
    divisibility: u8,
    premine: u128,
    terms: Option<Terms>,
    etching_txid: Txid,
    number: u64,
    timestamp: u64,
    turbo: bool,
) -> RuneEntry {
    let spaced = SpacedRune { rune, spacers };
    RuneEntry {
        id,
        rune: rune.0,
        name: rune.to_string(),
        spaced_name: spaced.to_string(),
        spacers,
        symbol: Some(symbol.unwrap_or('¤').to_string()),
        divisibility,
        premine,
        burned: 0,
        mints: 0,
        terms: terms.map(Into::into),
        etching_txid: etching_txid.to_byte_array(),
        number,
        timestamp,
        turbo,
    }
}

pub fn encode<T: BorshSerialize>(value: &T) -> Result<Vec<u8>> {
    borsh::to_vec(value).map_err(|e| anyhow!("runes encode failed: {e}"))
}

fn undo_key(height: u32) -> Vec<u8> {
    let mut key = Vec::with_capacity(UNDO_PREFIX.len() + 4);
    key.extend_from_slice(UNDO_PREFIX);
    key.extend_from_slice(&height.to_be_bytes());
    key
}

fn runes_put_key_requires_preimage(key: &[u8]) -> bool {
    if key == b"/tx_index/pointer/count"
        || key == b"/tx_index/actions/pointer/count"
        || key == b"/rune/seq/count"
        || key.starts_with(b"/tx_index/chunk_count/")
        || key.starts_with(b"/tx_index/chunk/")
        || key.starts_with(b"/tx_index/address/")
        || key.starts_with(b"/tx_index/actions/block/")
        || key.starts_with(b"/tx_index/actions/address/")
        || key.starts_with(b"/rune/by_id/")
        || key.starts_with(b"/holder/")
        || key.starts_with(b"/holder_count/")
        || key.starts_with(b"/address/")
        || key.starts_with(b"/address_balance_height/")
        || key.starts_with(b"/address_balance_height_idx/")
        || key.starts_with(b"/volume/")
        || key.starts_with(b"/volume_idx/")
        || key.starts_with(b"/tx_index/chunk/address/")
    {
        return true;
    }

    false
}

pub fn encode_u128(value: u128) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn decode_u128(value: &[u8]) -> Option<u128> {
    if value.len() != 16 {
        return None;
    }
    let mut arr = [0u8; 16];
    arr.copy_from_slice(value);
    Some(u128::from_le_bytes(arr))
}

pub fn encode_u64(value: u64) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn decode_u64(value: &[u8]) -> Option<u64> {
    if value.len() != 8 {
        return None;
    }
    let mut arr = [0u8; 8];
    arr.copy_from_slice(value);
    Some(u64::from_le_bytes(arr))
}

pub fn encode_u32(value: u32) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn decode_u32(value: &[u8]) -> Option<u32> {
    if value.len() != 4 {
        return None;
    }
    let mut arr = [0u8; 4];
    arr.copy_from_slice(value);
    Some(u32::from_le_bytes(arr))
}

fn id_key_bytes(id: SchemaRuneId) -> [u8; 12] {
    let mut out = [0u8; 12];
    out[..8].copy_from_slice(&id.block.to_be_bytes());
    out[8..].copy_from_slice(&id.tx.to_be_bytes());
    out
}

fn decode_id_key_tail(bytes: &[u8]) -> Option<SchemaRuneId> {
    if bytes.len() != 12 {
        return None;
    }
    let mut block = [0u8; 8];
    block.copy_from_slice(&bytes[..8]);
    let mut tx = [0u8; 4];
    tx.copy_from_slice(&bytes[8..12]);
    Some(SchemaRuneId { block: u64::from_be_bytes(block), tx: u32::from_be_bytes(tx) })
}

pub fn parse_rune_id(raw: &str) -> Option<SchemaRuneId> {
    let (block, tx) = raw.split_once(':')?;
    Some(SchemaRuneId { block: block.trim().parse().ok()?, tx: tx.trim().parse().ok()? })
}

pub fn normalize_name(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphabetic())
        .flat_map(|c| c.to_uppercase())
        .collect()
}

pub fn entry_key(id: SchemaRuneId) -> Vec<u8> {
    let mut key = b"/rune/by_id/".to_vec();
    key.extend_from_slice(&id_key_bytes(id));
    key
}

pub fn id_by_name_key(name: &str) -> Vec<u8> {
    let mut key = b"/rune/id_by_name/".to_vec();
    key.extend_from_slice(name.as_bytes());
    key
}

pub fn id_by_rune_key(rune: Rune) -> Vec<u8> {
    let mut key = b"/rune/id_by_rune/".to_vec();
    key.extend_from_slice(&rune.0.to_be_bytes());
    key
}

pub fn seq_key(seq: u64) -> Vec<u8> {
    let mut key = b"/rune/seq/".to_vec();
    key.extend_from_slice(&seq.to_be_bytes());
    key
}

pub fn seq_count_key() -> Vec<u8> {
    b"/rune/seq/count".to_vec()
}

pub fn outpoint_key(txid: &Txid, vout: u32) -> Vec<u8> {
    let mut key = b"/outpoint/".to_vec();
    key.extend_from_slice(txid.as_byte_array());
    key.extend_from_slice(&vout.to_be_bytes());
    key
}

pub fn decode_outpoint_key(key: &[u8]) -> Option<(Txid, u32)> {
    let tail = key.strip_prefix(b"/outpoint/")?;
    if tail.len() != 36 {
        return None;
    }
    let mut txid = [0u8; 32];
    txid.copy_from_slice(&tail[..32]);
    let mut vout = [0u8; 4];
    vout.copy_from_slice(&tail[32..36]);
    Some((Txid::from_byte_array(txid), u32::from_be_bytes(vout)))
}

pub fn address_outpoint_prefix(address: &str) -> Vec<u8> {
    let mut key = b"/address_outpoint/".to_vec();
    key.extend_from_slice(address.as_bytes());
    key.push(b'/');
    key
}

pub fn address_outpoint_key(address: &str, txid: &Txid, vout: u32) -> Vec<u8> {
    let mut key = address_outpoint_prefix(address);
    key.extend_from_slice(txid.as_byte_array());
    key.extend_from_slice(&vout.to_be_bytes());
    key
}

pub fn decode_address_outpoint_key(address: &str, key: &[u8]) -> Option<(Txid, u32)> {
    let prefix = address_outpoint_prefix(address);
    let tail = key.strip_prefix(prefix.as_slice())?;
    if tail.len() != 36 {
        return None;
    }
    let mut txid = [0u8; 32];
    txid.copy_from_slice(&tail[..32]);
    let mut vout = [0u8; 4];
    vout.copy_from_slice(&tail[32..36]);
    Some((Txid::from_byte_array(txid), u32::from_be_bytes(vout)))
}

pub fn holder_prefix(id: SchemaRuneId) -> Vec<u8> {
    let mut key = b"/holder/".to_vec();
    key.extend_from_slice(&id_key_bytes(id));
    key.push(b'/');
    key
}

pub fn holder_key(id: SchemaRuneId, address: &str) -> Vec<u8> {
    let mut key = holder_prefix(id);
    key.extend_from_slice(address.as_bytes());
    key
}

pub fn holders_count_key(id: SchemaRuneId) -> Vec<u8> {
    let mut key = b"/holder_count/".to_vec();
    key.extend_from_slice(&id_key_bytes(id));
    key
}

pub fn address_balance_prefix(address: &str) -> Vec<u8> {
    let mut key = b"/address/".to_vec();
    key.extend_from_slice(address.as_bytes());
    key.extend_from_slice(b"/balance/");
    key
}

pub fn address_balance_key(address: &str, id: SchemaRuneId) -> Vec<u8> {
    let mut key = address_balance_prefix(address);
    key.extend_from_slice(&id_key_bytes(id));
    key
}

pub fn address_balance_history_prefix(address: &str, id: SchemaRuneId) -> Vec<u8> {
    let mut key = b"/address_balance_height/v1/".to_vec();
    key.extend_from_slice(address.as_bytes());
    key.push(b'/');
    key.extend_from_slice(&id_key_bytes(id));
    key.push(b'/');
    key
}

pub fn address_balance_history_key(address: &str, id: SchemaRuneId, height: u32) -> Vec<u8> {
    let mut key = address_balance_history_prefix(address, id);
    key.extend_from_slice(&height.to_be_bytes());
    key
}

pub fn address_balance_history_list_prefix(address: &str, id: SchemaRuneId) -> Vec<u8> {
    let mut key = b"/address_balance_height_idx/v1/".to_vec();
    key.extend_from_slice(address.as_bytes());
    key.push(b'/');
    key.extend_from_slice(&id_key_bytes(id));
    key.push(b'/');
    key
}

pub fn address_balance_history_list_len_key(address: &str, id: SchemaRuneId) -> Vec<u8> {
    let mut key = address_balance_history_list_prefix(address, id);
    key.extend_from_slice(b"len");
    key
}

pub fn address_balance_history_list_idx_key(address: &str, id: SchemaRuneId, idx: u32) -> Vec<u8> {
    let mut key = address_balance_history_list_prefix(address, id);
    key.extend_from_slice(&idx.to_be_bytes());
    key
}

pub fn mint_activity_prefix(id: SchemaRuneId) -> Vec<u8> {
    let mut key = b"/activity/mint/".to_vec();
    key.extend_from_slice(&id_key_bytes(id));
    key.push(b'/');
    key
}

pub fn mint_activity_key(id: SchemaRuneId, timestamp: u64, txid: &Txid, ordinal: u32) -> Vec<u8> {
    let mut key = mint_activity_prefix(id);
    key.extend_from_slice(&timestamp.to_be_bytes());
    key.extend_from_slice(txid.as_byte_array());
    key.extend_from_slice(&ordinal.to_be_bytes());
    key
}

fn prefix_end_exclusive(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut end = prefix.to_vec();
    for i in (0..end.len()).rev() {
        if end[i] != 0xff {
            end[i] += 1;
            end.truncate(i + 1);
            return Some(end);
        }
    }
    None
}

pub fn rune_activity_index_key(
    activity: &RuneActivity,
    scope: RuneActivityScope,
    sort_by: RuneActivitySortField,
) -> Vec<u8> {
    rune_activity_index_key_from_prefix(
        rune_activity_index_prefix(activity.id, scope, sort_by),
        activity,
        sort_by,
    )
}

pub fn rune_address_activity_index_key(
    activity: &RuneActivity,
    address: &str,
    scope: RuneActivityScope,
    sort_by: RuneActivitySortField,
) -> Vec<u8> {
    rune_activity_index_key_from_prefix(
        rune_address_activity_index_prefix(address, scope, sort_by),
        activity,
        sort_by,
    )
}

pub fn rune_address_token_activity_index_key(
    activity: &RuneActivity,
    address: &str,
    scope: RuneActivityScope,
    sort_by: RuneActivitySortField,
) -> Vec<u8> {
    rune_activity_index_key_from_prefix(
        rune_address_token_activity_index_prefix(address, activity.id, scope, sort_by),
        activity,
        sort_by,
    )
}

fn rune_activity_index_key_from_prefix(
    mut key: Vec<u8>,
    activity: &RuneActivity,
    sort_by: RuneActivitySortField,
) -> Vec<u8> {
    match sort_by {
        RuneActivitySortField::Timestamp => {
            key.extend_from_slice(&activity.timestamp.to_be_bytes())
        }
        RuneActivitySortField::Amount => key.extend_from_slice(&activity.amount.to_be_bytes()),
    }
    key.extend_from_slice(&activity.height.to_be_bytes());
    key.extend_from_slice(&activity.tx_index.to_be_bytes());
    key.extend_from_slice(activity.kind.key().as_bytes());
    key.push(b'/');
    key.extend_from_slice(&activity.txid);
    key
}

pub fn rune_activity_index_prefix(
    id: SchemaRuneId,
    scope: RuneActivityScope,
    sort_by: RuneActivitySortField,
) -> Vec<u8> {
    let mut key = b"/activity/v2/".to_vec();
    key.extend_from_slice(&id_key_bytes(id));
    key.push(b'/');
    key.extend_from_slice(scope.segment());
    key.push(b'/');
    key.extend_from_slice(sort_by.segment());
    key.push(b'/');
    key
}

pub fn rune_address_activity_index_prefix(
    address: &str,
    scope: RuneActivityScope,
    sort_by: RuneActivitySortField,
) -> Vec<u8> {
    let mut key = b"/activity_address/v2/".to_vec();
    key.extend_from_slice(address.as_bytes());
    key.push(b'/');
    key.extend_from_slice(scope.segment());
    key.push(b'/');
    key.extend_from_slice(sort_by.segment());
    key.push(b'/');
    key
}

pub fn rune_address_token_activity_index_prefix(
    address: &str,
    id: SchemaRuneId,
    scope: RuneActivityScope,
    sort_by: RuneActivitySortField,
) -> Vec<u8> {
    let mut key = b"/activity_address_token/v2/".to_vec();
    key.extend_from_slice(address.as_bytes());
    key.push(b'/');
    key.extend_from_slice(&id_key_bytes(id));
    key.push(b'/');
    key.extend_from_slice(scope.segment());
    key.push(b'/');
    key.extend_from_slice(sort_by.segment());
    key.push(b'/');
    key
}

pub fn rune_volume_entry_key(kind: RuneVolumeKind, id: SchemaRuneId, address: &str) -> Vec<u8> {
    let mut key = b"/volume/".to_vec();
    key.extend_from_slice(kind.segment());
    key.push(b'/');
    key.extend_from_slice(&id_key_bytes(id));
    key.push(b'/');
    key.extend_from_slice(address.as_bytes());
    key
}

pub fn rune_volume_list_len_key(kind: RuneVolumeKind, id: SchemaRuneId) -> Vec<u8> {
    let mut key = b"/volume_idx/".to_vec();
    key.extend_from_slice(kind.segment());
    key.push(b'/');
    key.extend_from_slice(&id_key_bytes(id));
    key.extend_from_slice(b"/len");
    key
}

pub fn rune_volume_list_idx_key(kind: RuneVolumeKind, id: SchemaRuneId, idx: u32) -> Vec<u8> {
    let mut key = b"/volume_idx/".to_vec();
    key.extend_from_slice(kind.segment());
    key.push(b'/');
    key.extend_from_slice(&id_key_bytes(id));
    key.push(b'/');
    key.extend_from_slice(&idx.to_be_bytes());
    key
}

pub fn tx_io_key(txid: &Txid) -> Vec<u8> {
    let mut key = b"/tx/io/".to_vec();
    key.extend_from_slice(txid.as_byte_array());
    key
}

pub fn rune_icon_key(id: SchemaRuneId) -> Vec<u8> {
    let mut key = b"/rune/icon/".to_vec();
    key.extend_from_slice(&id_key_bytes(id));
    key
}

pub fn uncommon_goods_avg_price_usd_by_height_prefix() -> Vec<u8> {
    UNCOMMON_GOODS_AVG_PRICE_USD_BY_HEIGHT_PREFIX.to_vec()
}

pub fn uncommon_goods_avg_price_usd_by_height_key(height: u32) -> Vec<u8> {
    let mut key = Vec::with_capacity(UNCOMMON_GOODS_AVG_PRICE_USD_BY_HEIGHT_PREFIX.len() + 4);
    key.extend_from_slice(UNCOMMON_GOODS_AVG_PRICE_USD_BY_HEIGHT_PREFIX);
    key.extend_from_slice(&height.to_be_bytes());
    key
}

fn decode_height_tail(bytes: &[u8]) -> Option<u32> {
    if bytes.len() != 4 {
        return None;
    }
    let mut arr = [0u8; 4];
    arr.copy_from_slice(bytes);
    Some(u32::from_be_bytes(arr))
}

pub fn script_to_address(script: &ScriptBuf, network: bitcoin::Network) -> Option<String> {
    bitcoin::Address::from_script(script, network).ok().map(|a| a.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_provider() -> (TempDir, RunesProvider) {
        let dir = TempDir::new().expect("tempdir");
        let mdb = Mdb::open(dir.path(), b"runes:").expect("open mdb");
        (dir, RunesProvider::new(Arc::new(mdb)))
    }

    #[test]
    fn block_undo_rolls_raw_runes_state_back() {
        let (_dir, provider) = test_provider();
        let h1 = BlockHash::from_byte_array([1; 32]);
        let h2 = BlockHash::from_byte_array([2; 32]);

        provider
            .set_block_batch(vec![(b"/a".to_vec(), b"one".to_vec())], Vec::new(), 1, &h1)
            .expect("write block 1");
        provider
            .set_block_batch(vec![(b"/b".to_vec(), b"two".to_vec())], vec![b"/a".to_vec()], 2, &h2)
            .expect("write block 2");

        provider.rollback_before_height(2).expect("rollback block 2");
        assert_eq!(provider.get_index_height().expect("height"), Some(1));
        assert_eq!(provider.get_index_block_hash().expect("hash"), Some(h1));
        assert_eq!(provider.mdb().get(b"/a").expect("a"), Some(b"one".to_vec()));
        assert_eq!(provider.mdb().get(b"/b").expect("b"), None);
        assert!(!provider.has_undo_for_height(2).expect("undo 2"));

        provider.rollback_before_height(1).expect("rollback block 1");
        assert_eq!(provider.get_index_height().expect("height"), None);
        assert_eq!(provider.get_index_block_hash().expect("hash"), None);
        assert_eq!(provider.mdb().get(b"/a").expect("a"), None);
    }
}
