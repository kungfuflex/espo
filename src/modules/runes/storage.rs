use crate::config::get_address_index_chunk_size;
use crate::runtime::mdb::Mdb;
use anyhow::{Result, anyhow};
use bitcoin::hashes::Hash;
use bitcoin::{ScriptBuf, Txid};
use borsh::{BorshDeserialize, BorshSerialize};
use ordinals::{Rune, RuneId, SpacedRune, Terms};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

use super::inscriptions::RuneIcon;

const INDEX_HEIGHT_KEY: &[u8] = b"/index_height";
const TX_INDEX_INLINE_CAP: usize = 8;

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
    pub height: u32,
    pub tx_index: u32,
    pub timestamp: u64,
    pub amount: u128,
    pub destination: Option<String>,
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
}

impl RuneTxIndexKind {
    fn segment(self) -> &'static [u8] {
        match self {
            Self::Block => b"block",
            Self::Address => b"address",
        }
    }
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

    pub fn get_outpoint_balances(
        &self,
        txid: &Txid,
        vout: u32,
    ) -> Result<Option<OutpointRuneBalances>> {
        self.get_entry(&outpoint_key(txid, vout))
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
        let mut rows = Vec::new();
        for item in self.mdb.scan_prefix_entries(b"/rune/by_id/")? {
            let (_key, value) = item;
            let entry = RuneEntry::try_from_slice(&value)?;
            let holders = self.get_holders_count(entry.id)?;
            rows.push((entry, holders));
        }
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.number.cmp(&b.0.number)));
        let start = limit.saturating_mul(page.saturating_sub(1));
        Ok(rows.into_iter().skip(start).take(limit).collect())
    }

    pub fn get_holders_count(&self, id: SchemaRuneId) -> Result<u64> {
        Ok(self.mdb.get(&holders_count_key(id))?.and_then(|v| decode_u64(&v)).unwrap_or(0))
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

    pub fn get_mint_activity(
        &self,
        id: SchemaRuneId,
        page: usize,
        limit: usize,
    ) -> Result<Vec<RuneMintActivity>> {
        let prefix = mint_activity_prefix(id);
        let mut rows = Vec::new();
        for item in self.mdb.scan_prefix_entries(&prefix)? {
            let (_key, value) = item;
            rows.push(RuneMintActivity::try_from_slice(&value)?);
        }
        rows.sort_by(|a, b| {
            b.timestamp.cmp(&a.timestamp).then_with(|| b.tx_index.cmp(&a.tx_index))
        });
        let start = limit.saturating_mul(page.saturating_sub(1));
        Ok(rows.into_iter().skip(start).take(limit).collect())
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

pub fn rune_tx_pointer_key(id: u64) -> Vec<u8> {
    let mut key = b"/tx_index/pointer/".to_vec();
    key.extend_from_slice(&id.to_be_bytes());
    key
}

pub fn rune_tx_block_list_key(height: u64) -> Vec<u8> {
    let mut key = b"/tx_index/block/".to_vec();
    key.extend_from_slice(&height.to_be_bytes());
    key
}

pub fn rune_tx_address_list_key(address: &str) -> Vec<u8> {
    let mut key = b"/tx_index/address/".to_vec();
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
        "etching_txid": hex::encode(entry.etching_txid),
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

pub fn script_to_address(script: &ScriptBuf, network: bitcoin::Network) -> Option<String> {
    bitcoin::Address::from_script(script, network).ok().map(|a| a.to_string())
}
