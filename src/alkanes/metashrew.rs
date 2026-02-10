use crate::alkanes::defs::AlkaneMessageContext;
use crate::alkanes::trace::PartialEspoTrace;
use crate::config::{get_metashrew_sdb, strict_check_trace_mismatches};
use crate::runtime::sdb::SDB;
use crate::schemas::SchemaAlkaneId;
use alkanes_cli_common::alkanes_pb::{AlkanesTrace, AlkanesTraceEvent};
use alkanes_support::gz;
use alkanes_support::id::AlkaneId as SupportAlkaneId;
use anyhow::{Context, Result, anyhow};
use bitcoin::OutPoint;
use bitcoin::Txid;
use bitcoin::hashes::Hash;
use metashrew_support::index_pointer::KeyValuePointer;
use prost::Message;
use protorune::message::MessageContext;
use protorune_support::balance_sheet::BalanceSheet;
use protorune_support::balance_sheet::ProtoruneRuneId;
use protorune_support::utils::consensus_encode;
use rocksdb::{Direction, IteratorMode};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

fn try_decode_trace_prost(raw: &[u8]) -> Option<AlkanesTrace> {
    AlkanesTrace::decode(raw).ok().or_else(|| {
        if raw.len() >= 4 { AlkanesTrace::decode(&raw[..raw.len() - 4]).ok() } else { None }
    })
}

fn try_decode_trace_event_prost(raw: &[u8]) -> Option<AlkanesTraceEvent> {
    AlkanesTraceEvent::decode(raw).ok().or_else(|| {
        if raw.len() >= 4 { AlkanesTraceEvent::decode(&raw[..raw.len() - 4]).ok() } else { None }
    })
}

/// Traces can be stored as raw protobuf bytes or as UTF-8 "height:HEX" blobs.
/// This helper handles both by decoding any hex payload and stripping the
/// optional 4-byte trailer some entries carry.
pub fn decode_trace_blob(bytes: &[u8]) -> Option<AlkanesTrace> {
    if let Ok(s) = std::str::from_utf8(bytes) {
        if let Some((_block, hex_part)) = s.split_once(':') {
            if let Ok(decoded) = hex::decode(hex_part) {
                if let Some(trace) = try_decode_trace_prost(&decoded) {
                    return Some(trace);
                }
            }
        }
    }

    try_decode_trace_prost(bytes)
}

/// Trace events can be stored as raw protobuf bytes or as UTF-8 "height:HEX" blobs.
pub fn decode_trace_event_blob(bytes: &[u8]) -> Option<AlkanesTraceEvent> {
    if let Ok(s) = std::str::from_utf8(bytes) {
        if let Some((_block, hex_part)) = s.split_once(':') {
            if let Ok(decoded) = hex::decode(hex_part) {
                if let Some(event) = try_decode_trace_event_prost(&decoded) {
                    return Some(event);
                }
            }
        }
    }

    try_decode_trace_event_prost(bytes)
}

fn decode_height_prefixed(bytes: &[u8]) -> Option<Vec<u8>> {
    let s = std::str::from_utf8(bytes).ok()?;
    let (_height, hex_part) = s.split_once(':')?;
    hex::decode(hex_part).ok()
}

fn ascii_length_to_le(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.is_empty() || !bytes.iter().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let parsed: u32 = std::str::from_utf8(bytes).ok()?.parse().ok()?;
    Some(parsed.to_le_bytes().to_vec())
}

fn parse_length_value(bytes: &[u8]) -> Option<u64> {
    let normalized = decode_height_prefixed(bytes).unwrap_or_else(|| bytes.to_vec());
    if normalized.is_empty() {
        return None;
    }
    if normalized.iter().all(|b| b.is_ascii_digit()) {
        return std::str::from_utf8(&normalized).ok()?.parse().ok();
    }
    match normalized.len() {
        4 => {
            let mut arr = [0u8; 4];
            arr.copy_from_slice(&normalized);
            Some(u32::from_le_bytes(arr) as u64)
        }
        8 => {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&normalized);
            Some(u64::from_le_bytes(arr))
        }
        _ => None,
    }
}

#[derive(Clone, Default)]
struct SdbPointer<'a> {
    sdb: Option<&'a SDB>,
    key: Arc<Vec<u8>>,
    label: Option<Arc<String>>,
}

impl SdbPointer<'static> {
    fn root(label: Option<String>) -> Self {
        let label = label.and_then(|s| {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() { None } else { Some(trimmed) }
        });
        SdbPointer { sdb: None, key: Arc::new(Vec::new()), label: label.map(Arc::new) }
    }

    fn from_bytes<'a>(&self, sdb: &'a SDB, key: Vec<u8>) -> SdbPointer<'a> {
        SdbPointer { sdb: Some(sdb), key: Arc::new(key), label: self.label.clone() }
    }
}

impl<'a> SdbPointer<'a> {
    fn with_db<'b>(&self, sdb: &'b SDB) -> SdbPointer<'b> {
        SdbPointer { sdb: Some(sdb), key: self.key.clone(), label: self.label.clone() }
    }

    fn apply_label(&self, key: &[u8]) -> Vec<u8> {
        let Some(label) = &self.label else {
            return key.to_vec();
        };

        let label_bytes = label.as_bytes();
        let mut with = Vec::with_capacity(label_bytes.len() + 3 + key.len());
        with.extend_from_slice(label_bytes);
        with.extend_from_slice(b"://");
        with.extend_from_slice(key);
        with
    }

    fn key_with_label(&self) -> Vec<u8> {
        self.apply_label(self.key.as_ref())
    }

    fn is_length_key(&self) -> bool {
        self.key.as_slice().ends_with(b"/length")
    }

    fn get_raw(&self) -> Option<Vec<u8>> {
        let sdb = self.sdb?;
        if self.label.is_some() {
            let key = self.key_with_label();
            match sdb.get(&key) {
                Ok(Some(bytes)) => Some(bytes),
                _ => None,
            }
        } else {
            match sdb.get(self.key.as_ref()) {
                Ok(Some(bytes)) => Some(bytes),
                _ => None,
            }
        }
    }

    fn length_with_depth(&self, base: &[u8], depth: u8) -> Option<u64> {
        let mut length_key = Vec::with_capacity(base.len() + 7);
        length_key.extend_from_slice(base);
        length_key.extend_from_slice(b"/length");

        let length_ptr = self.with_key(&length_key);
        if let Some(bytes) = length_ptr.get_raw() {
            return parse_length_value(&bytes);
        }
        if depth >= 2 {
            return None;
        }
        let bytes = self.get_raw_with_depth(&length_key, depth + 1)?;
        parse_length_value(&bytes)
    }

    fn get_raw_with_depth(&self, base: &[u8], depth: u8) -> Option<Vec<u8>> {
        if depth > 2 {
            return None;
        }
        if let Some(len) = self.length_with_depth(base, depth) {
            if len == 0 {
                return None;
            }
            let idx = len.saturating_sub(1);
            let mut key = Vec::with_capacity(base.len() + 1 + 20);
            key.extend_from_slice(base);
            key.push(b'/');
            key.extend_from_slice(idx.to_string().as_bytes());
            let ptr = self.with_key(&key);
            if let Some(bytes) = ptr.get_raw() {
                return Some(bytes);
            }
            if depth >= 2 {
                return None;
            }
            return self.get_raw_with_depth(&key, depth + 1);
        }
        self.with_key(base).get_raw()
    }

    fn with_key(&self, key: &[u8]) -> SdbPointer<'a> {
        SdbPointer { sdb: self.sdb, key: Arc::new(key.to_vec()), label: self.label.clone() }
    }
}

impl<'a> KeyValuePointer for SdbPointer<'a> {
    fn wrap(word: &Vec<u8>) -> Self {
        SdbPointer { sdb: None, key: Arc::new(word.clone()), label: None }
    }

    fn unwrap(&self) -> Arc<Vec<u8>> {
        self.key.clone()
    }

    fn set(&mut self, _v: Arc<Vec<u8>>) {}

    fn get(&self) -> Arc<Vec<u8>> {
        let Some(mut bytes) = self.get_raw_with_depth(self.key.as_ref(), 0) else {
            return Arc::new(Vec::new());
        };

        if let Some(decoded) = decode_height_prefixed(&bytes) {
            bytes = decoded;
        }

        if self.is_length_key() {
            if let Some(converted) = ascii_length_to_le(&bytes) {
                bytes = converted;
            }
        }

        Arc::new(bytes)
    }

    fn inherits(&mut self, from: &Self) {
        self.sdb = from.sdb;
        self.label = from.label.clone();
    }
}

#[allow(non_snake_case, dead_code)]
#[derive(Clone, Default)]
struct RuneTableNative<'a> {
    pub HEIGHT_TO_BLOCKHASH: SdbPointer<'a>,
    pub BLOCKHASH_TO_HEIGHT: SdbPointer<'a>,
    pub OUTPOINT_TO_RUNES: SdbPointer<'a>,
    pub OUTPOINT_TO_HEIGHT: SdbPointer<'a>,
    pub HEIGHT_TO_TRANSACTION_IDS: SdbPointer<'a>,
    pub SYMBOL: SdbPointer<'a>,
    pub CAP: SdbPointer<'a>,
    pub SPACERS: SdbPointer<'a>,
    pub OFFSETEND: SdbPointer<'a>,
    pub OFFSETSTART: SdbPointer<'a>,
    pub HEIGHTSTART: SdbPointer<'a>,
    pub HEIGHTEND: SdbPointer<'a>,
    pub AMOUNT: SdbPointer<'a>,
    pub MINTS_REMAINING: SdbPointer<'a>,
    pub PREMINE: SdbPointer<'a>,
    pub DIVISIBILITY: SdbPointer<'a>,
    pub RUNE_ID_TO_HEIGHT: SdbPointer<'a>,
    pub ETCHINGS: SdbPointer<'a>,
    pub RUNE_ID_TO_ETCHING: SdbPointer<'a>,
    pub ETCHING_TO_RUNE_ID: SdbPointer<'a>,
    pub RUNTIME_BALANCE: SdbPointer<'a>,
    pub HEIGHT_TO_RUNE_ID: SdbPointer<'a>,
    pub RUNE_ID_TO_INITIALIZED: SdbPointer<'a>,
    pub INTERNAL_MINT: SdbPointer<'a>,
    pub TXID_TO_TXINDEX: SdbPointer<'a>,
}

impl<'a> RuneTableNative<'a> {
    fn for_protocol(root: &SdbPointer<'a>, tag: u128) -> Self {
        RuneTableNative {
            HEIGHT_TO_BLOCKHASH: root.keyword("/runes/null"),
            BLOCKHASH_TO_HEIGHT: root.keyword("/runes/null"),
            HEIGHT_TO_RUNE_ID: root.keyword(format!("/runes/proto/{tag}/byheight/").as_str()),
            RUNE_ID_TO_INITIALIZED: root
                .keyword(format!("/runes/proto/{tag}/initialized/").as_str()),
            OUTPOINT_TO_RUNES: root.keyword(format!("/runes/proto/{tag}/byoutpoint/").as_str()),
            OUTPOINT_TO_HEIGHT: root.keyword("/runes/null"),
            HEIGHT_TO_TRANSACTION_IDS: root
                .keyword(format!("/runes/proto/{tag}/txids/byheight").as_str()),
            SYMBOL: root.keyword(format!("/runes/proto/{tag}/symbol/").as_str()),
            CAP: root.keyword(format!("/runes/proto/{tag}/cap/").as_str()),
            SPACERS: root.keyword(format!("/runes/proto/{tag}/spaces/").as_str()),
            OFFSETEND: root.keyword("/runes/null"),
            OFFSETSTART: root.keyword("/runes/null"),
            HEIGHTSTART: root.keyword("/runes/null"),
            HEIGHTEND: root.keyword("/runes/null"),
            AMOUNT: root.keyword("/runes/null"),
            MINTS_REMAINING: root.keyword("/runes/null"),
            PREMINE: root.keyword("/runes/null"),
            DIVISIBILITY: root.keyword(format!("/runes/proto/{tag}/divisibility/").as_str()),
            RUNE_ID_TO_HEIGHT: root.keyword("/rune/null"),
            ETCHINGS: root.keyword(format!("/runes/proto/{tag}/names").as_str()),
            RUNE_ID_TO_ETCHING: root
                .keyword(format!("/runes/proto/{tag}/etching/byruneid/").as_str()),
            ETCHING_TO_RUNE_ID: root
                .keyword(format!("/runes/proto/{tag}/runeid/byetching/").as_str()),
            RUNTIME_BALANCE: root.keyword(format!("/runes/proto/{tag}/runtime/balance").as_str()),
            INTERNAL_MINT: root.keyword(format!("/runes/proto/{tag}/mint/isinternal").as_str()),
            TXID_TO_TXINDEX: root.keyword("/txindex/byid"),
        }
    }
}

#[allow(non_snake_case)]
#[derive(Clone)]
struct TraceTablesNative<'a> {
    pub TRACES_NATIVE: SdbPointer<'a>,
    pub TRACES_BY_HEIGHT_NATIVE: SdbPointer<'a>,
}

impl<'a> TraceTablesNative<'a> {
    fn new(root: &SdbPointer<'a>) -> Self {
        let traces = root.keyword("/trace/");
        TraceTablesNative { TRACES_NATIVE: traces.clone(), TRACES_BY_HEIGHT_NATIVE: traces }
    }
}

pub struct MetashrewAdapter {
    root: SdbPointer<'static>,
}

impl MetashrewAdapter {
    pub fn new(label: Option<String>) -> MetashrewAdapter {
        MetashrewAdapter { root: SdbPointer::root(label) }
    }

    fn root_ptr<'a>(&self, db: &'a SDB) -> SdbPointer<'a> {
        self.root.with_db(db)
    }

    fn rune_table<'a>(&self, db: &'a SDB) -> RuneTableNative<'a> {
        let root = self.root_ptr(db);
        RuneTableNative::for_protocol(&root, AlkaneMessageContext::protocol_tag())
    }

    fn outpoint_runes_ptr<'a>(&self, db: &'a SDB, outpoint: &OutPoint) -> Result<SdbPointer<'a>> {
        let table = self.rune_table(db);
        let outpoint_bytes = consensus_encode(outpoint)?;
        Ok(table.OUTPOINT_TO_RUNES.select(&outpoint_bytes))
    }

    fn outpoint_balances_from_id_to_balance(
        &self,
        db: &SDB,
        outpoint: &OutPoint,
    ) -> Result<Vec<(SupportAlkaneId, u128)>> {
        let ptr = self.outpoint_runes_ptr(db, outpoint)?;
        let id_base = ptr.keyword("/id_to_balance");
        let scan_prefix = id_base.key_with_label();

        let mut seen_ids: HashSet<Vec<u8>> = HashSet::new();
        let mut it = db.iterator(IteratorMode::From(&scan_prefix, Direction::Forward));
        while let Some(Ok((k, _v))) = it.next() {
            if !k.starts_with(&scan_prefix) {
                break;
            }
            let suffix = &k[scan_prefix.len()..];
            if suffix.len() < 32 {
                continue;
            }
            seen_ids.insert(suffix[..32].to_vec());
        }

        let mut out = Vec::new();
        for id_bytes in seen_ids {
            let balance = id_base.select(&id_bytes).get_value::<u128>();
            if balance == 0 {
                continue;
            }
            let rune_id = ProtoruneRuneId::try_from(id_bytes)?;
            let alkane_id: SupportAlkaneId = rune_id.into();
            out.push((alkane_id, balance));
        }

        Ok(out)
    }

    fn latest_version_ptr<'a>(&self, base: &SdbPointer<'a>) -> Option<SdbPointer<'a>> {
        let len = base.length();
        if len == 0 { None } else { Some(base.select_index(len.saturating_sub(1))) }
    }

    fn load_wasm_inner(
        &self,
        ptr: &SdbPointer<'_>,
        id: SupportAlkaneId,
        seen: &mut HashSet<(u128, u128)>,
        hops: usize,
    ) -> Result<Option<(Vec<u8>, SupportAlkaneId)>> {
        const MAX_HOPS: usize = 64;
        if hops > MAX_HOPS {
            return Err(anyhow!("alias chain too deep (possible cycle)"));
        }
        if !seen.insert((id.block, id.tx)) {
            return Err(anyhow!("alias cycle detected at ({}, {})", id.block, id.tx));
        }

        let id_bytes: Vec<u8> = (&id).into();
        let payload = ptr.select(&id_bytes).get();
        if payload.is_empty() {
            return Ok(None);
        }

        if payload.len() == 32 {
            let alias = SupportAlkaneId::try_from(payload.as_ref().clone())
                .map_err(|e| anyhow!("decode alkane alias for ({}, {}): {e}", id.block, id.tx))?;
            return self.load_wasm_inner(ptr, alias, seen, hops + 1);
        }

        let bytes = gz::decompress(payload.as_ref().clone())
            .map_err(|e| anyhow!("decompress alkane wasm payload from metashrew: {e}"))?;
        Ok(Some((bytes, id)))
    }

    pub fn get_alkane_wasm_bytes_with_db(
        &self,
        db: &SDB,
        alkane: &SchemaAlkaneId,
    ) -> Result<Option<(Vec<u8>, SchemaAlkaneId)>> {
        let mut seen = HashSet::new();
        let root = self.root_ptr(db);
        let base = root.keyword("/alkanes/");
        let alkane_id = SupportAlkaneId { block: alkane.block as u128, tx: alkane.tx as u128 };
        let res = self.load_wasm_inner(&base, alkane_id, &mut seen, 0)?;
        if let Some((bytes, sid)) = res {
            let block: u32 = sid
                .block
                .try_into()
                .map_err(|_| anyhow!("factory alkane block does not fit into u32"))?;
            let tx: u64 = sid
                .tx
                .try_into()
                .map_err(|_| anyhow!("factory alkane tx does not fit into u64"))?;
            Ok(Some((bytes, SchemaAlkaneId { block, tx })))
        } else {
            Ok(None)
        }
    }

    pub fn get_alkane_wasm_bytes(
        &self,
        alkane: &SchemaAlkaneId,
    ) -> Result<Option<(Vec<u8>, SchemaAlkaneId)>> {
        let db = get_metashrew_sdb();
        db.catch_up_now().context("metashrew catch_up before wasm fetch")?;
        self.get_alkane_wasm_bytes_with_db(db.as_ref(), alkane)
    }

    pub fn get_alkanes_tip_height(&self) -> Result<u32> {
        static LAST_LOGGED_HEIGHT: AtomicU32 = AtomicU32::new(u32::MAX);
        let db = get_metashrew_sdb();
        let height_ptr = self.root.from_bytes(db.as_ref(), b"__INTERNAL/height".to_vec());
        let height = height_ptr.get_value::<u32>();
        let prev = LAST_LOGGED_HEIGHT.load(Ordering::Relaxed);
        if prev != height {
            eprintln!("[metashrew] indexed height: {}", height);
            LAST_LOGGED_HEIGHT.store(height, Ordering::Relaxed);
        }
        Ok(height)
    }

    /// Fetch all traces for a txid directly from the secondary DB, without needing block height.
    pub fn traces_for_tx(&self, txid: &Txid) -> Result<Vec<PartialEspoTrace>> {
        let db = get_metashrew_sdb();
        self.traces_for_tx_with_db(db.as_ref(), txid)
    }

    pub fn traces_for_tx_with_db(&self, db: &SDB, txid: &Txid) -> Result<Vec<PartialEspoTrace>> {
        db.catch_up_now().context("metashrew catch_up before scanning traces_for_tx")?;

        let root = self.root_ptr(db);
        let traces = TraceTablesNative::new(&root);

        let tx_be = txid.to_byte_array().to_vec();
        let mut tx_le = tx_be.clone();
        tx_le.reverse();

        let mut traces_by_outpoint: HashMap<Vec<u8>, Option<AlkanesTrace>> = HashMap::new();

        let mut scan_prefix = |tx_bytes: &[u8], tx_bytes_are_be: bool| -> Result<()> {
            let mut prefix = b"/trace/".to_vec();
            prefix.extend_from_slice(tx_bytes);
            let prefix = self.root.apply_label(&prefix);

            let mut tx_outpoint = tx_bytes.to_vec();
            if tx_bytes_are_be {
                tx_outpoint.reverse();
            }

            let mut it = db.iterator(IteratorMode::From(&prefix, Direction::Forward));
            while let Some(Ok((k, v))) = it.next() {
                if !k.starts_with(&prefix) {
                    break;
                }

                let suffix = &k[prefix.len()..];
                if suffix.len() < 4 {
                    continue;
                }
                let vout_le = &suffix[..4];

                let mut outpoint = Vec::with_capacity(tx_outpoint.len() + 4);
                outpoint.extend_from_slice(&tx_outpoint);
                outpoint.extend_from_slice(vout_le);

                let entry = traces_by_outpoint.entry(outpoint).or_insert(None);
                if entry.is_none() && suffix.len() == 4 {
                    if let Some(trace) = decode_trace_blob(&v) {
                        *entry = Some(trace);
                    }
                }
            }

            Ok(())
        };

        scan_prefix(&tx_be, true)?;
        if tx_le != tx_be {
            scan_prefix(&tx_le, false)?;
        }

        let mut out: Vec<PartialEspoTrace> = Vec::new();
        for (outpoint, fallback) in traces_by_outpoint {
            let trace_bytes = traces.TRACES_NATIVE.select(&outpoint).get();
            let trace =
                if !trace_bytes.is_empty() { decode_trace_blob(&trace_bytes) } else { fallback };
            if let Some(trace) = trace {
                out.push(PartialEspoTrace { protobuf_trace: trace, outpoint });
            }
        }

        Ok(out)
    }

    pub fn get_reserves_for_alkane_with_db(
        &self,
        db: &SDB,
        who_alkane: &SchemaAlkaneId,
        what_alkane: &SchemaAlkaneId,
        height: Option<u64>,
    ) -> Result<Option<u128>> {
        let root = self.root_ptr(db);
        let what_id =
            SupportAlkaneId { block: what_alkane.block as u128, tx: what_alkane.tx as u128 };
        let who_id = SupportAlkaneId { block: who_alkane.block as u128, tx: who_alkane.tx as u128 };
        let what_bytes: Vec<u8> = (&what_id).into();
        let who_bytes: Vec<u8> = (&who_id).into();
        let pointer = root
            .keyword("/alkanes/")
            .select(&what_bytes)
            .keyword("/balances/")
            .select(&who_bytes);

        let parse_entry = |entry_bytes: &[u8]| -> Result<(u64, u128)> {
            let entry_str = std::str::from_utf8(entry_bytes)
                .map_err(|e| anyhow!("utf8 decode balance entry: {e}"))?;
            let (height_str, hex_part) =
                entry_str.split_once(':').ok_or_else(|| anyhow!("balance entry missing ':'"))?;

            let updated_height: u64 = height_str
                .parse()
                .map_err(|e| anyhow!("parse balance height '{height_str}': {e}"))?;

            let raw_balance = hex::decode(hex_part)
                .map_err(|e| anyhow!("hex decode balance payload '{hex_part}': {e}"))?;
            if raw_balance.len() != 16 {
                return Err(anyhow!(
                    "balance payload length {}, expected 16 bytes",
                    raw_balance.len()
                ));
            }
            let mut bal_bytes = [0u8; 16];
            bal_bytes.copy_from_slice(&raw_balance);

            Ok((updated_height, u128::from_le_bytes(bal_bytes)))
        };

        let read_entry_at = |idx: u32| -> Result<Option<(u64, u128)>> {
            let entry_bytes = match pointer.select_index(idx).get_raw() {
                Some(bytes) => bytes,
                None => return Ok(None),
            };
            parse_entry(&entry_bytes).map(Some)
        };

        let length = pointer.length();
        if length == 0 {
            return Ok(None);
        }
        let last_idx = length.saturating_sub(1);

        let Some(target_height) = height else {
            let Some((_height, balance)) = read_entry_at(last_idx)? else {
                return Ok(None);
            };
            return Ok(Some(balance));
        };

        let Some((latest_height, latest_bal)) = read_entry_at(last_idx)? else {
            return Ok(None);
        };
        if latest_height <= target_height {
            return Ok(Some(latest_bal));
        }

        let mut low = 0u32;
        let mut high = last_idx;
        let mut best: Option<(u64, u128)> = None;

        while low <= high {
            let mid = low + (high - low) / 2;
            let Some((entry_height, entry_balance)) = read_entry_at(mid)? else {
                return Ok(None);
            };

            if entry_height <= target_height {
                best = Some((entry_height, entry_balance));
                if mid == u32::MAX {
                    break;
                }
                low = mid + 1;
            } else {
                if mid == 0 {
                    break;
                }
                high = mid - 1;
            }
        }

        Ok(best.map(|(_, bal)| bal))
    }

    pub fn get_reserves_for_alkane(
        &self,
        who_alkane: &SchemaAlkaneId,
        what_alkane: &SchemaAlkaneId,
        height: Option<u64>,
    ) -> Result<Option<u128>> {
        let db = get_metashrew_sdb();
        self.get_reserves_for_alkane_with_db(db.as_ref(), who_alkane, what_alkane, height)
    }

    pub fn get_outpoint_alkane_balances_with_db(
        &self,
        db: &SDB,
        txid: &Txid,
        vout: u32,
    ) -> Result<Vec<(SupportAlkaneId, u128)>> {
        let outpoint = OutPoint::new(*txid, vout);
        let map_out = self.outpoint_balances_from_id_to_balance(db, &outpoint)?;
        if !map_out.is_empty() {
            return Ok(map_out);
        }
        let ptr = self.outpoint_runes_ptr(db, &outpoint)?;
        let runes_base = ptr.keyword("/runes");
        let balances_base = ptr.keyword("/balances");

        let runes_list = self.latest_version_ptr(&runes_base);
        let balances_list = self.latest_version_ptr(&balances_base);
        let (Some(runes_list), Some(balances_list)) = (runes_list, balances_list) else {
            return Ok(Vec::new());
        };

        let runes_len = runes_list.length();
        let balances_len = balances_list.length();
        if runes_len == 0 || balances_len == 0 {
            return Ok(Vec::new());
        }
        if balances_len < runes_len {
            return Err(anyhow!(
                "outpoint balance array missing balances: runes_len={} balances_len={}",
                runes_len,
                balances_len
            ));
        }
        if balances_len > runes_len {
            eprintln!(
                "[metashrew] outpoint balance arrays: extra balances ignored (runes_len={} balances_len={})",
                runes_len, balances_len
            );
        }

        let mut out = Vec::with_capacity(runes_len as usize);
        for idx in 0..runes_len {
            let rune_id = ProtoruneRuneId::from(runes_list.select_index(idx).get());
            let balance = balances_list.select_index(idx).get_value::<u128>();
            if balance == 0 {
                continue;
            }
            let alkane_id: SupportAlkaneId = rune_id.into();
            out.push((alkane_id, balance));
        }

        Ok(out)
    }

    pub fn get_outpoint_alkane_balances(
        &self,
        txid: &Txid,
        vout: u32,
    ) -> Result<Vec<(SupportAlkaneId, u128)>> {
        let db = get_metashrew_sdb();
        db.catch_up_now().context("metashrew catch_up before outpoint balances")?;
        self.get_outpoint_alkane_balances_with_db(db.as_ref(), txid, vout)
    }

    pub fn get_outpoint_alkane_balance_for_id_with_db(
        &self,
        db: &SDB,
        txid: &Txid,
        vout: u32,
        id: &SupportAlkaneId,
    ) -> Result<Option<u128>> {
        let outpoint = OutPoint::new(*txid, vout);
        let ptr = self.outpoint_runes_ptr(db, &outpoint)?;
        let sheet = BalanceSheet::new_ptr_backed(ptr);
        let rune_id: ProtoruneRuneId = (*id).into();
        let balance = sheet.load_balance(&rune_id);
        if balance == 0 { Ok(None) } else { Ok(Some(balance)) }
    }

    pub fn traces_for_block_as_prost(&self, block: u64) -> Result<Vec<PartialEspoTrace>> {
        let db = get_metashrew_sdb();
        self.traces_for_block_as_prost_with_db(db.as_ref(), block)
    }

    pub fn traces_for_block_as_prost_with_db(
        &self,
        db: &SDB,
        block: u64,
    ) -> Result<Vec<PartialEspoTrace>> {
        // Ensure the secondary view is fresh before scanning traces.
        db.catch_up_now().context("metashrew catch_up before scanning traces")?;
        let root = self.root_ptr(db);
        let traces = TraceTablesNative::new(&root);
        let outpoints = traces.TRACES_BY_HEIGHT_NATIVE.select_value(block).get_list();
        let list_len = outpoints.len();

        let mut missing_trace_blobs = 0usize;
        let mut bad_pointers = 0usize;
        let mut final_traces: Vec<PartialEspoTrace> = Vec::new();
        let mut seen_outpoints: HashSet<Vec<u8>> = HashSet::new();

        for outpoint_arc in outpoints {
            let mut outpoint = outpoint_arc.as_ref().clone();
            if outpoint.is_empty() {
                bad_pointers = bad_pointers.saturating_add(1);
                continue;
            }
            if outpoint.len() > 36 {
                outpoint.truncate(36);
            }
            if outpoint.len() < 36 {
                bad_pointers = bad_pointers.saturating_add(1);
                continue;
            }
            if !seen_outpoints.insert(outpoint.clone()) {
                continue;
            }

            let trace_bytes = traces.TRACES_NATIVE.select(&outpoint).get();
            if trace_bytes.is_empty() {
                missing_trace_blobs = missing_trace_blobs.saturating_add(1);
                continue;
            }
            if let Some(protobuf_trace) = decode_trace_blob(&trace_bytes) {
                final_traces.push(PartialEspoTrace { protobuf_trace, outpoint });
            } else {
                missing_trace_blobs = missing_trace_blobs.saturating_add(1);
            }
        }
        if final_traces.is_empty() {
            eprintln!("[metashrew] block {block}: pointers={list_len} traces=0");
        }

        if missing_trace_blobs > 0 || bad_pointers > 0 {
            let mut reason = String::new();
            if missing_trace_blobs > 0 {
                reason.push_str(&format!("missing_trace_blobs={missing_trace_blobs}"));
            }
            if bad_pointers > 0 {
                if !reason.is_empty() {
                    reason.push_str(", ");
                }
                reason.push_str(&format!("bad_pointers={bad_pointers}"));
            }
            let warning =
                format!("[metashrew] warn: block {block}: trace index looks incomplete ({reason})");
            eprintln!("{warning}");
            if strict_check_trace_mismatches() {
                panic!("{warning}");
            }
        }

        Ok(final_traces)
    }
}
