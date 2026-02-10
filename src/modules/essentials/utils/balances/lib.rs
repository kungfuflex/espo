use super::defs::{EspoTraceType, SignedU128, SignedU128MapExt};
use super::utils::{
    Unallocated, compute_nets, is_op_return, parse_protostones, parse_short_id,
    schema_id_from_parts, transfers_to_sheet, tx_has_op_return, u128_to_u32,
};
use crate::alkanes::trace::{
    EspoBlock, EspoHostFunctionValues, EspoSandshrewLikeTrace, EspoSandshrewLikeTraceEvent,
    EspoSandshrewLikeTraceStatus, EspoTrace,
};
use crate::config::{
    debug_enabled, get_electrum_like, get_espo_db, get_metashrew, get_metashrew_sdb, get_network,
    strict_check_alkane_balances, strict_check_trace_mismatches, strict_check_utxos,
};
use crate::debug;
use crate::modules::ammdata::config::AmmDataConfig;
use crate::modules::ammdata::storage::{AmmDataTable, SearchIndexField};
use crate::modules::ammdata::utils::search::collect_search_prefixes;
use crate::modules::essentials::storage::get_holders_values_encoded;
use crate::modules::essentials::storage::{
    AddressActivityEntry, AddressAmountEntry, AlkaneBalanceTxEntry, AlkaneBalanceTxsMeta,
    AlkaneTxSummary, BALANCE_TXS_PAGE_SIZE, BalanceEntry, HolderEntry, HolderId,
    decode_address_activity_entry, decode_address_amount_vec, decode_alkane_balance_tx_entries,
    decode_balances_vec, decode_holders_vec, decode_u128_value, encode_address_activity_entry,
    encode_address_amount_vec, encode_u128_value, encode_vec, mk_outpoint, spk_to_address_str,
};
use crate::modules::essentials::storage::{
    EssentialsProvider, GetIterPrefixRevParams, GetMultiValuesParams, GetRawValueParams,
    GetScanPrefixParams, SetBatchParams,
};
use crate::runtime::mdb::Mdb;
use crate::schemas::{EspoOutpoint, SchemaAlkaneId};
use anyhow::{Context, Result, anyhow};
use bitcoin::block::Header;
use bitcoin::consensus::encode::deserialize;
use bitcoin::{ScriptBuf, Transaction, Txid, hashes::Hash};
use borsh::BorshDeserialize;
use protorune_support::protostone::{Protostone, ProtostoneEdict};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

static AMMDATA_MDB: OnceLock<Arc<Mdb>> = OnceLock::new();

fn ammdata_mdb() -> Arc<Mdb> {
    AMMDATA_MDB
        .get_or_init(|| Arc::new(Mdb::from_db(get_espo_db(), b"ammdata:")))
        .clone()
}

fn append_paged_balance_txs<F>(
    provider: &EssentialsProvider,
    meta_key: Vec<u8>,
    mut page_key: F,
    new_entries: &[AlkaneBalanceTxEntry],
    puts: &mut Vec<(Vec<u8>, Vec<u8>)>,
) -> Result<(u64, u64)>
where
    F: FnMut(u64) -> Vec<u8>,
{
    if new_entries.is_empty() {
        return Ok((0, 0));
    }

    let mut meta =
        if let Ok(resp) = provider.get_raw_value(GetRawValueParams { key: meta_key.clone() }) {
            resp.value
                .and_then(|bytes| AlkaneBalanceTxsMeta::try_from_slice(&bytes).ok())
                .unwrap_or(AlkaneBalanceTxsMeta {
                    page_size: BALANCE_TXS_PAGE_SIZE,
                    last_page: 0,
                    total_len: 0,
                })
        } else {
            AlkaneBalanceTxsMeta { page_size: BALANCE_TXS_PAGE_SIZE, last_page: 0, total_len: 0 }
        };

    if meta.page_size == 0 {
        meta.page_size = BALANCE_TXS_PAGE_SIZE;
    }
    let page_size = meta.page_size as usize;

    let mut last_entries: Vec<AlkaneBalanceTxEntry> = Vec::new();
    let mut decode_failed = false;
    if meta.total_len > 0 {
        let last_key = page_key(meta.last_page);
        if let Ok(resp) = provider.get_raw_value(GetRawValueParams { key: last_key }) {
            if let Some(bytes) = resp.value {
                if let Ok(list) = decode_alkane_balance_tx_entries(&bytes) {
                    last_entries = list;
                } else {
                    decode_failed = true;
                }
            } else {
                decode_failed = true;
            }
        } else {
            decode_failed = true;
        }
    }

    if decode_failed {
        meta.total_len = 0;
        meta.last_page = 0;
        last_entries.clear();
    }

    let mut seen: HashMap<[u8; 32], usize> = HashMap::new();
    for (idx, entry) in last_entries.iter().enumerate() {
        seen.insert(entry.txid, idx);
    }

    let mut added: u64 = 0;
    for entry in new_entries {
        if let Some(idx) = seen.get(&entry.txid).copied() {
            if last_entries[idx].outflow.is_empty() && !entry.outflow.is_empty() {
                last_entries[idx].outflow = entry.outflow.clone();
            }
            if last_entries[idx].height == 0 && entry.height != 0 {
                last_entries[idx].height = entry.height;
            }
            continue;
        }
        seen.insert(entry.txid, last_entries.len());
        last_entries.push(entry.clone());
        added = added.saturating_add(1);
    }

    let mut pages_written: u64 = 0;
    let mut page_idx = meta.last_page;
    if last_entries.len() <= page_size {
        if let Ok(buf) = encode_vec(&last_entries) {
            puts.push((page_key(page_idx), buf));
            pages_written = pages_written.saturating_add(1);
        }
    } else {
        let end = page_size.min(last_entries.len());
        if let Ok(buf) = encode_vec(&last_entries[..end].to_vec()) {
            puts.push((page_key(page_idx), buf));
            pages_written = pages_written.saturating_add(1);
        }
        let mut start = end;
        while start < last_entries.len() {
            page_idx = page_idx.saturating_add(1);
            let end = (start + page_size).min(last_entries.len());
            if let Ok(buf) = encode_vec(&last_entries[start..end].to_vec()) {
                puts.push((page_key(page_idx), buf));
                pages_written = pages_written.saturating_add(1);
            }
            start = end;
        }
    }

    meta.last_page = page_idx;
    meta.total_len = meta.total_len.saturating_add(added);
    if let Ok(buf) = borsh::to_vec(&meta) {
        puts.push((meta_key, buf));
    }

    Ok((pages_written, added))
}

pub(crate) fn clean_espo_sandshrew_like_trace(
    trace: &EspoSandshrewLikeTrace,
    host_function_values: &EspoHostFunctionValues,
) -> Option<EspoSandshrewLikeTrace> {
    let mut invokes = 0usize;
    let mut returns = 0usize;
    for ev in &trace.events {
        match ev {
            EspoSandshrewLikeTraceEvent::Invoke(_) => invokes += 1,
            EspoSandshrewLikeTraceEvent::Return(_) => returns += 1,
            EspoSandshrewLikeTraceEvent::Create(_) => {}
        }
    }

    if invokes == returns {
        return Some(trace.clone());
    }
    if returns < invokes {
        return None;
    }

    let (header, coinbase, diesel, fee) = host_function_values;
    let host_values: [&[u8]; 4] = [header, coinbase, diesel, fee];
    let mismatch = returns.saturating_sub(invokes);

    let decode_data = |data: &str| -> Option<Vec<u8>> {
        let trimmed = data.strip_prefix("0x").unwrap_or(data);
        if trimmed.is_empty() {
            return Some(Vec::new());
        }
        hex::decode(trimmed).ok()
    };

    let host_match = |data_bytes: &[u8]| -> bool {
        for host_bytes in host_values.iter() {
            if data_bytes == *host_bytes {
                return true;
            }
        }
        false
    };

    let fuzzy_host_match = |data_bytes: &[u8]| -> bool {
        if data_bytes.len() == 80 && deserialize::<Header>(data_bytes).is_ok() {
            return true;
        }
        if let Ok(tx) = deserialize::<Transaction>(data_bytes) {
            if tx.is_coinbase() {
                return true;
            }
        }
        false
    };

    let attempt_clean = |allow_fuzzy: bool| -> Option<EspoSandshrewLikeTrace> {
        let mut remove_indices: HashSet<usize> = HashSet::new();
        let mut candidate_stack: Vec<usize> = Vec::new();
        let mut total_candidates = 0usize;
        let mut depth: isize = 0;

        for (idx, ev) in trace.events.iter().enumerate() {
            match ev {
                EspoSandshrewLikeTraceEvent::Invoke(_) => {
                    depth += 1;
                }
                EspoSandshrewLikeTraceEvent::Return(ret) => {
                    let mut is_candidate = false;
                    if ret.status == EspoSandshrewLikeTraceStatus::Success
                        && ret.response.alkanes.is_empty()
                        && ret.response.storage.is_empty()
                    {
                        if let Some(data_bytes) = decode_data(&ret.response.data) {
                            if host_match(&data_bytes) {
                                is_candidate = true;
                            } else if allow_fuzzy && fuzzy_host_match(&data_bytes) {
                                is_candidate = true;
                            }
                        }
                    }
                    if is_candidate {
                        total_candidates += 1;
                        candidate_stack.push(idx);
                    }

                    depth -= 1;
                    if depth < 0 {
                        let Some(remove_idx) = candidate_stack.pop() else {
                            return None;
                        };
                        remove_indices.insert(remove_idx);
                        depth += 1;
                    }
                }
                EspoSandshrewLikeTraceEvent::Create(_) => {}
            }
        }

        if total_candidates < mismatch || remove_indices.len() != mismatch {
            return None;
        }

        let mut cleaned_events =
            Vec::with_capacity(trace.events.len().saturating_sub(remove_indices.len()));
        for (idx, ev) in trace.events.iter().enumerate() {
            if !remove_indices.contains(&idx) {
                cleaned_events.push(ev.clone());
            }
        }

        let mut cleaned_invokes = 0usize;
        let mut cleaned_returns = 0usize;
        let mut cleaned_depth: isize = 0;
        for ev in &cleaned_events {
            match ev {
                EspoSandshrewLikeTraceEvent::Invoke(_) => {
                    cleaned_invokes += 1;
                    cleaned_depth += 1;
                }
                EspoSandshrewLikeTraceEvent::Return(_) => {
                    cleaned_returns += 1;
                    cleaned_depth -= 1;
                    if cleaned_depth < 0 {
                        return None;
                    }
                }
                EspoSandshrewLikeTraceEvent::Create(_) => {}
            }
        }
        if cleaned_invokes != cleaned_returns || cleaned_depth != 0 {
            return None;
        }

        Some(EspoSandshrewLikeTrace { outpoint: trace.outpoint.clone(), events: cleaned_events })
    };

    attempt_clean(false).or_else(|| attempt_clean(true))
}

fn parse_u128_from_str(input: &str) -> Option<u128> {
    if let Some(hex) = input.strip_prefix("0x") {
        u128::from_str_radix(hex, 16).ok()
    } else {
        input.parse::<u128>().ok()
    }
}

fn mint_deltas_from_trace(
    trace: &EspoSandshrewLikeTrace,
    host_function_values: &EspoHostFunctionValues,
) -> Option<BTreeMap<SchemaAlkaneId, u128>> {
    let trace = clean_espo_sandshrew_like_trace(trace, host_function_values)?;

    #[derive(Clone)]
    struct Frame {
        owner: Option<SchemaAlkaneId>,
        mint_candidate: bool,
        incoming: Vec<(SchemaAlkaneId, u128)>,
    }

    let mut stack: Vec<Frame> = Vec::new();
    let mut deltas: BTreeMap<SchemaAlkaneId, u128> = BTreeMap::new();

    for ev in &trace.events {
        match ev {
            EspoSandshrewLikeTraceEvent::Invoke(inv) => {
                let typ = inv.typ.to_ascii_lowercase();
                let is_static = typ == "staticcall";
                let mut mint_candidate = false;
                if !is_static {
                    let opcode_match = inv
                        .context
                        .inputs
                        .get(2)
                        .and_then(|s| parse_u128_from_str(s))
                        .filter(|op| *op == 77)
                        .is_some()
                        || inv
                            .context
                            .inputs
                            .get(0)
                            .and_then(|s| parse_u128_from_str(s))
                            .filter(|op| *op == 77)
                            .is_some();
                    if opcode_match {
                        mint_candidate = true;
                    }
                }
                let incoming = if mint_candidate {
                    inv.context
                        .incoming_alkanes
                        .iter()
                        .filter_map(|t| {
                            let id = parse_short_id(&t.id)?;
                            let value = parse_u128_from_str(&t.value)?;
                            if value == 0 {
                                return None;
                            }
                            Some((id, value))
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                stack.push(Frame {
                    owner: parse_short_id(&inv.context.myself),
                    mint_candidate,
                    incoming,
                });
            }
            EspoSandshrewLikeTraceEvent::Return(ret) => {
                let Some(frame) = stack.pop() else {
                    return None;
                };
                if !frame.mint_candidate {
                    continue;
                }
                if ret.status != EspoSandshrewLikeTraceStatus::Success {
                    continue;
                }
                let Some(owner) = frame.owner else {
                    continue;
                };
                let mut returned: Vec<(SchemaAlkaneId, u128)> = ret
                    .response
                    .alkanes
                    .iter()
                    .filter_map(|t| {
                        let id = parse_short_id(&t.id)?;
                        let value = parse_u128_from_str(&t.value)?;
                        if value == 0 {
                            return None;
                        }
                        Some((id, value))
                    })
                    .collect();
                if !frame.incoming.is_empty() && !returned.is_empty() {
                    for (inc_id, inc_value) in &frame.incoming {
                        if let Some(pos) = returned
                            .iter()
                            .position(|(id, value)| id == inc_id && value == inc_value)
                        {
                            returned.remove(pos);
                        }
                    }
                }
                if let Some((_, value)) = returned.iter().find(|(id, _)| *id == owner) {
                    *deltas.entry(owner).or_default() =
                        deltas.get(&owner).copied().unwrap_or(0).saturating_add(*value);
                }
            }
            EspoSandshrewLikeTraceEvent::Create(_) => {}
        }
    }

    if !stack.is_empty() {
        return None;
    }

    Some(deltas)
}

pub(crate) fn accumulate_alkane_balance_deltas(
    trace: &EspoSandshrewLikeTrace,
    _txid: &Txid,
    host_function_values: &EspoHostFunctionValues,
) -> (bool, HashMap<SchemaAlkaneId, BTreeMap<SchemaAlkaneId, SignedU128>>) {
    let debug = debug_enabled();
    let module = "essentials.balances";
    let timer = debug::start_if(debug);
    let Some(trace) = clean_espo_sandshrew_like_trace(trace, host_function_values) else {
        if strict_check_trace_mismatches() {
            eprintln!(
                "[balances][strict] dropped trace: failed to clean sandshrew-like events (txid={})",
                _txid
            );
        }
        return (false, HashMap::new());
    };
    debug::log_elapsed(module, "accumulate.clean_trace", timer);
    if std::env::var_os("ESPO_LOG_HOST_FUNCTION_VALUES").is_some() {
        let (header, coinbase, diesel, fee) = host_function_values;
        eprintln!(
            "[balances] host_function_values header={} coinbase={} diesel={} fee={}",
            hex::encode(header),
            hex::encode(coinbase),
            hex::encode(diesel),
            hex::encode(fee),
        );
    }

    // We treat the trace as a call stack (invoke ... return), and only apply balance
    // changes when a frame returns successfully. This lets us drop an entire subtree
    // of effects if a parent frame fails or is static (reverts all children).
    //
    // Rules implemented:
    // - Normal calls: incoming credits go to `myself`, outgoing debits come from `myself`.
    // - Delegate calls: still credit `myself` for incoming, but the "parent" for both
    //   incoming and outgoing is the nearest NORMAL ancestor frame (skip delegates).
    // - Static calls: ignored completely (no effects, children ignored).
    // - Create events: ignored.
    // - Returned alkanes pay to the nearest normal parent (never to a delegate).
    // - We allow negative deltas here; final balance checks happen later.
    // - Self-token deltas are kept for outflow reporting; balances/holders ignore them later.

    #[derive(Copy, Clone, Eq, PartialEq, Debug)]
    enum FrameKind {
        Normal,
        Delegate,
        Static,
    }

    #[derive(Clone)]
    struct Frame {
        kind: FrameKind,
        owner: SchemaAlkaneId,
        incoming: BTreeMap<SchemaAlkaneId, u128>,
        parent_normal: Option<SchemaAlkaneId>,
        deltas: HashMap<SchemaAlkaneId, BTreeMap<SchemaAlkaneId, SignedU128>>,
    }

    // Find the nearest NORMAL frame in the current stack (delegates/statics are skipped).
    fn nearest_normal_owner(stack: &[Frame]) -> Option<SchemaAlkaneId> {
        stack.iter().rev().find_map(|frame| {
            if matches!(frame.kind, FrameKind::Normal) { Some(frame.owner) } else { None }
        })
    }

    // Add a signed delta for a (owner, token) pair.
    // Self-token deltas are kept for outflow reporting; balances filter them later.
    fn add_delta(
        outflows: &mut HashMap<SchemaAlkaneId, BTreeMap<SchemaAlkaneId, SignedU128>>,
        owner: SchemaAlkaneId,
        token: SchemaAlkaneId,
        delta: SignedU128,
    ) {
        if delta.is_zero() {
            return;
        }
        let remove = {
            let entry = outflows.entry(owner).or_default();
            entry.add_signed(token, delta);
            entry.is_empty()
        };
        if remove {
            outflows.remove(&owner);
        }
    }

    // Apply a transfer (amount of token) from -> to into a delta map.
    fn apply_transfer(
        outflows: &mut HashMap<SchemaAlkaneId, BTreeMap<SchemaAlkaneId, SignedU128>>,
        from: Option<SchemaAlkaneId>,
        to: Option<SchemaAlkaneId>,
        token: SchemaAlkaneId,
        amount: u128,
    ) {
        if amount == 0 {
            return;
        }
        if let Some(owner) = from {
            add_delta(outflows, owner, token, SignedU128::negative(amount));
        }
        if let Some(owner) = to {
            add_delta(outflows, owner, token, SignedU128::positive(amount));
        }
    }

    // Merge a child's delta map into its parent (used to drop effects on failure/static).
    fn merge_deltas(
        target: &mut HashMap<SchemaAlkaneId, BTreeMap<SchemaAlkaneId, SignedU128>>,
        child: HashMap<SchemaAlkaneId, BTreeMap<SchemaAlkaneId, SignedU128>>,
    ) {
        for (owner, per_token) in child {
            if per_token.is_empty() {
                continue;
            }
            let remove = {
                let entry = target.entry(owner).or_default();
                for (token, delta) in per_token {
                    entry.add_signed(token, delta);
                }
                entry.is_empty()
            };
            if remove {
                target.remove(&owner);
            }
        }
    }

    let mut outflows: HashMap<SchemaAlkaneId, BTreeMap<SchemaAlkaneId, SignedU128>> =
        HashMap::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut root_reverted = false;

    for ev in &trace.events {
        match ev {
            EspoSandshrewLikeTraceEvent::Invoke(inv) => {
                // Determine call kind and the nearest normal parent BEFORE pushing the frame.
                let kind = match inv.typ.to_ascii_lowercase().as_str() {
                    "delegatecall" => FrameKind::Delegate,
                    "staticcall" => FrameKind::Static,
                    _ => FrameKind::Normal,
                };
                let Some(owner) = parse_short_id(&inv.context.myself) else { continue };
                let parent_normal = nearest_normal_owner(&stack);

                // Static calls are ignored, but we still push a frame to keep stack depth.
                let incoming = if matches!(kind, FrameKind::Static) {
                    BTreeMap::new()
                } else {
                    transfers_to_sheet(&inv.context.incoming_alkanes)
                };

                stack.push(Frame { kind, owner, incoming, parent_normal, deltas: HashMap::new() });
            }
            EspoSandshrewLikeTraceEvent::Return(ret) => {
                let Some(mut frame) = stack.pop() else {
                    // Mismatched return: treat as a reverted root.
                    root_reverted = true;
                    continue;
                };

                // Failed frames (and their children) are ignored.
                if ret.status == EspoSandshrewLikeTraceStatus::Failure {
                    if stack.is_empty() && matches!(frame.kind, FrameKind::Normal) {
                        root_reverted = true;
                    }
                    continue;
                }

                // Static calls are ignored completely (including children).
                if matches!(frame.kind, FrameKind::Static) {
                    continue;
                }

                // Incoming: transfer from nearest normal parent -> this frame's owner.
                for (token, amount) in &frame.incoming {
                    apply_transfer(
                        &mut frame.deltas,
                        frame.parent_normal,
                        Some(frame.owner),
                        *token,
                        *amount,
                    );
                }

                // Outgoing: transfer from this frame's owner -> nearest normal parent.
                let outgoing = transfers_to_sheet(&ret.response.alkanes);
                for (token, amount) in &outgoing {
                    apply_transfer(
                        &mut frame.deltas,
                        Some(frame.owner),
                        frame.parent_normal,
                        *token,
                        *amount,
                    );
                }

                // Merge this frame's (successful) subtree effects upward.
                if let Some(parent) = stack.last_mut() {
                    merge_deltas(&mut parent.deltas, frame.deltas);
                } else {
                    merge_deltas(&mut outflows, frame.deltas);
                }
            }
            EspoSandshrewLikeTraceEvent::Create(_) => {
                // Create events are ignored per rules.
            }
        }
    }

    if root_reverted || !stack.is_empty() {
        return (false, HashMap::new());
    }

    (true, outflows)
}

/* -------------------------- Edicts + routing (multi-protostone, per your rules) -------------------------- */

/// Whether `vout` is a valid, spendable, non-OP_RETURN output index for this tx.
fn is_valid_spend_vout(tx: &Transaction, vout: u32) -> bool {
    let i = vout as usize;
    i < tx.output.len() && !is_op_return(&tx.output[i].script_pubkey)
}

fn apply_transfers_multi(
    tx: &Transaction,
    protostones: &[Protostone],
    traces_for_tx: &[EspoTrace],
    mut seed_unalloc: Unallocated, // VIN balances only
) -> Result<HashMap<u32, Vec<BalanceEntry>>> {
    let mut out_map: HashMap<u32, Vec<BalanceEntry>> = HashMap::new();

    let n_outputs: u32 = tx.output.len() as u32;
    let multicast_index: u32 = n_outputs; // runes multicast
    let shadow_base: u32 = n_outputs.saturating_add(1);
    let shadow_end: u32 = shadow_base + protostones.len() as u32 - 1;

    // Spendable (non-OP_RETURN)
    let spendable_vouts: Vec<u32> = tx
        .output
        .iter()
        .enumerate()
        .filter_map(|(i, o)| if is_op_return(&o.script_pubkey) { None } else { Some(i as u32) })
        .collect();

    // Map shadow index -> trace (prefer match by Invoke.vout; fallback by order)
    let mut trace_by_shadow: HashMap<u32, &EspoSandshrewLikeTrace> = HashMap::new();

    for t in traces_for_tx {
        // prefer the vout recorded in the first Invoke; else use the outpoint's vout
        let mut vout_opt: Option<u32> = None;
        for ev in &t.sandshrew_trace.events {
            if let EspoSandshrewLikeTraceEvent::Invoke(inv) = ev {
                vout_opt = Some(inv.context.vout);
                break;
            }
        }
        let vout = vout_opt.unwrap_or(t.outpoint.vout);

        // only keep traces that actually point into this tx's shadow range
        if vout >= shadow_base && vout <= shadow_end {
            trace_by_shadow.insert(vout, &t.sandshrew_trace);
        }
    }

    // Sheet incoming routed explicitly to protostone[i] (from previous pointers/edicts/refunds)
    let mut incoming_shadow: Vec<BTreeMap<SchemaAlkaneId, u128>> =
        vec![BTreeMap::new(); protostones.len()];

    // helpers
    fn push_to_vout(
        out_map: &mut HashMap<u32, Vec<BalanceEntry>>,
        vout: u32,
        delta: &BTreeMap<SchemaAlkaneId, u128>,
    ) {
        if delta.is_empty() {
            return;
        }
        let e = out_map.entry(vout).or_default();
        for (rid, &amt) in delta {
            if amt > 0 {
                e.push(BalanceEntry { alkane: *rid, amount: amt });
            }
        }
    }

    fn route_delta(
        target: u32,
        delta: &BTreeMap<SchemaAlkaneId, u128>,
        out_map: &mut HashMap<u32, Vec<BalanceEntry>>,
        incoming_shadow: &mut [BTreeMap<SchemaAlkaneId, u128>],
        tx: &Transaction,
        spendable_vouts: &[u32],
        n_outputs: u32,
        multicast_index: u32,
        shadow_base: u32,
        shadow_end: u32,
    ) {
        if delta.is_empty() {
            return;
        }

        if target == multicast_index {
            if spendable_vouts.is_empty() {
                return;
            }
            let m = spendable_vouts.len() as u128;
            for (rid, &total_amt) in delta.iter() {
                if total_amt == 0 {
                    continue;
                }
                let per = total_amt / m;
                let rem = (total_amt % m) as usize;
                for (i, out_i) in spendable_vouts.iter().enumerate() {
                    let mut amt = per;
                    if i < rem {
                        amt = amt.saturating_add(1);
                    }
                    if amt == 0 {
                        continue;
                    }
                    out_map
                        .entry(*out_i)
                        .or_default()
                        .push(BalanceEntry { alkane: *rid, amount: amt });
                }
            }
            return;
        }

        if target < n_outputs {
            if !is_valid_spend_vout(tx, target) {
                return;
            }
            push_to_vout(out_map, target, delta);
            return;
        }

        if target >= shadow_base && target <= shadow_end {
            let idx = (target - shadow_base) as usize;
            let sheet = &mut incoming_shadow[idx];
            for (rid, &amt) in delta {
                if amt == 0 {
                    continue;
                }
                *sheet.entry(*rid).or_default() =
                    sheet.get(rid).copied().unwrap_or(0).saturating_add(amt);
            }
            return;
        }
        // else burn by omission
    }

    fn apply_single_edict(
        sheet: &mut BTreeMap<SchemaAlkaneId, u128>,
        ed: &ProtostoneEdict,
        out_map: &mut HashMap<u32, Vec<BalanceEntry>>,
        incoming_shadow: &mut [BTreeMap<SchemaAlkaneId, u128>],
        tx: &Transaction,
        spendable_vouts: &[u32],
        n_outputs: u32,
        multicast_index: u32,
        shadow_base: u32,
        shadow_end: u32,
    ) -> Result<()> {
        // guard
        if ed.id.block == 0 && ed.id.tx > 0 {
            return Ok(());
        }
        let out_idx = u128_to_u32(ed.output)?;
        let rid = schema_id_from_parts(ed.id.block, ed.id.tx)?;

        // ---- SPECIAL: multicast target (output == n_outputs) ----
        if out_idx == multicast_index {
            if spendable_vouts.is_empty() {
                return Ok(());
            }

            // how much is available on the sheet for this rune
            let entry = sheet.entry(rid).or_default();
            let have = *entry;
            if have == 0 {
                return Ok(());
            }

            if ed.amount == 0 {
                // even split of ALL available (what you already had working)
                let mut delta = BTreeMap::new();
                delta.insert(rid, have);
                // zero it out from the sheet before routing
                *entry = 0;
                sheet.remove(&rid);

                route_delta(
                    out_idx,
                    &delta,
                    out_map,
                    incoming_shadow,
                    tx,
                    spendable_vouts,
                    n_outputs,
                    multicast_index,
                    shadow_base,
                    shadow_end,
                );
            } else {
                // amount > 0 → treat ed.amount as PER-VOUT CAP, and use ALL available
                let mut remaining = have;
                let mut used: u128 = 0;

                for v in spendable_vouts {
                    if remaining == 0 {
                        break;
                    }
                    let give = remaining.min(ed.amount);
                    if give == 0 {
                        break;
                    }
                    out_map.entry(*v).or_default().push(BalanceEntry { alkane: rid, amount: give });
                    remaining = remaining.saturating_sub(give);
                    used = used.saturating_add(give);
                }

                // subtract only what we actually allocated; leave any leftover on the sheet
                *entry = entry.saturating_sub(used);
                if *entry == 0 {
                    sheet.remove(&rid);
                }
            }

            return Ok(());
        }

        // ---- normal (non-multicast) targets: original behavior ----
        let have = sheet.get(&rid).copied().unwrap_or(0);
        let need = if ed.amount == 0 { have } else { ed.amount.min(have) };
        if need == 0 {
            return Ok(());
        }

        // take from sheet
        let entry = sheet.entry(rid).or_default();
        let take = (*entry).min(need);
        *entry = entry.saturating_sub(take);
        if *entry == 0 {
            sheet.remove(&rid);
        }
        if take == 0 {
            return Ok(());
        }

        // route normally
        let mut delta = BTreeMap::new();
        delta.insert(rid, take);
        route_delta(
            out_idx,
            &delta,
            out_map,
            incoming_shadow,
            tx,
            spendable_vouts,
            n_outputs,
            multicast_index,
            shadow_base,
            shadow_end,
        );
        Ok(())
    }

    // process in order
    for (i, ps) in protostones.iter().enumerate() {
        let shadow_vout = shadow_base + i as u32;

        // sheet starts with explicitly routed incoming to this shadow.
        let mut sheet: BTreeMap<SchemaAlkaneId, u128> = BTreeMap::new();

        // merge routed-in firstx
        for (rid, amt) in std::mem::take(&mut incoming_shadow[i]) {
            if amt == 0 {
                continue;
            }
            *sheet.entry(rid).or_default() =
                sheet.get(&rid).copied().unwrap_or(0).saturating_add(amt);
        }

        // if there is a trace for this protostone, compute net_out and status
        let (net_in, net_out, status) = match trace_by_shadow.get(&shadow_vout) {
            Some(trace) => compute_nets(trace),
            None => (None, None, EspoTraceType::NOTRACE),
        };
        // Metashrew can omit incoming_alkanes on reverted traces; fall back to NOTRACE
        // so original VIN balances are still available for edicts/pointers.
        let revert_missing_incoming =
            status == EspoTraceType::REVERT && net_in.as_ref().map_or(true, |m| m.is_empty());
        let status = if revert_missing_incoming { EspoTraceType::NOTRACE } else { status };

        // On success, consume incoming amounts so only returned/minted balances remain.
        if status == EspoTraceType::SUCCESS {
            if let Some(ref net_in_map) = net_in {
                for (rid, amt) in net_in_map {
                    if *amt == 0 {
                        continue;
                    }
                    let entry = sheet.entry(*rid).or_default();
                    *entry = entry.saturating_sub(*amt);
                    if *entry == 0 {
                        sheet.remove(rid);
                    }
                }
            }
        }

        // add net_out to sheet
        if status == EspoTraceType::SUCCESS {
            if let Some(ref net_out_map) = net_out {
                for (rid, amt) in net_out_map {
                    if *amt == 0 {
                        continue;
                    }
                    *sheet.entry(*rid).or_default() =
                        sheet.get(rid).copied().unwrap_or(0).saturating_add(*amt);
                }
            }
        }
        // merge VIN balances ONLY into protostone 0’s sheet
        if i == 0 && status == EspoTraceType::NOTRACE {
            for (rid, amt) in seed_unalloc.drain_all() {
                if amt == 0 {
                    continue;
                }
                *sheet.entry(rid).or_default() =
                    sheet.get(&rid).copied().unwrap_or(0).saturating_add(amt);
            }
        }

        // If we have a status and it is Failure → refund net_in (only), skip edicts.
        if status == EspoTraceType::REVERT {
            if let Some(ref net_in_map) = net_in {
                if let Some(refund_ptr) = ps.refund {
                    route_delta(
                        refund_ptr,
                        &net_in_map,
                        &mut out_map,
                        &mut incoming_shadow,
                        tx,
                        &spendable_vouts,
                        n_outputs,
                        multicast_index,
                        shadow_base,
                        shadow_end,
                    );
                }
                // if no refund pointer → burn (do nothing)
            }
            // Skip edicts on failure
            continue;
        }

        // Success path (or no status info): apply edicts against the current sheet
        if !ps.edicts.is_empty() {
            for ed in &ps.edicts {
                if let Err(e) = apply_single_edict(
                    &mut sheet,
                    ed,
                    &mut out_map,
                    &mut incoming_shadow,
                    tx,
                    &spendable_vouts,
                    n_outputs,
                    multicast_index,
                    shadow_base,
                    shadow_end,
                ) {
                    eprintln!("[ESSENTIALS::balances] WARN edict apply failed: {e:?}");
                }
            }
        }

        // leftovers after edicts:
        if !sheet.is_empty() {
            if let Some(ptr) = ps.pointer {
                route_delta(
                    ptr,
                    &sheet,
                    &mut out_map,
                    &mut incoming_shadow,
                    tx,
                    &spendable_vouts,
                    n_outputs,
                    multicast_index,
                    shadow_base,
                    shadow_end,
                );
            } else {
                // per your note: do NOT auto-chain; send to first non-OP_RETURN vout
                if let Some(v) = spendable_vouts.first().copied() {
                    push_to_vout(&mut out_map, v, &sheet);
                }
                // else burn by omission
            }
        }
    }

    Ok(out_map)
}

/* -------------------------- Holders helpers -------------------------- */

fn holder_order_key(id: &HolderId) -> String {
    match id {
        HolderId::Address(a) => format!("addr:{a}"),
        HolderId::Alkane(id) => format!("alkane:{:010}:{:020}", id.block, id.tx),
    }
}

fn apply_holders_delta(
    mut holders: Vec<HolderEntry>,
    holder: &HolderId,
    delta: SignedU128,
) -> Vec<HolderEntry> {
    let idx = holders.iter().position(|h| h.holder == *holder);
    let (is_negative, amount) = delta.as_parts();
    if amount == 0 {
        return holders;
    }
    if !is_negative {
        if let Some(i) = idx {
            holders[i].amount = holders[i].amount.saturating_add(amount);
        } else {
            holders.push(HolderEntry { holder: holder.clone(), amount });
        }
    } else if let Some(i) = idx {
        let cur = holders[i].amount;
        if amount > cur {
            panic!(
                "[balances] negative holder balance detected (holder={:?}, cur={}, sub={})",
                holders[i].holder, cur, amount
            );
        }
        let after = cur - amount;
        if after == 0 {
            holders.swap_remove(i);
        } else {
            holders[i].amount = after;
        }
    }
    holders.sort_by(|a, b| match b.amount.cmp(&a.amount) {
        std::cmp::Ordering::Equal => holder_order_key(&a.holder).cmp(&holder_order_key(&b.holder)),
        o => o,
    });
    holders
}

fn sort_address_amount_entries(entries: &mut Vec<AddressAmountEntry>) {
    entries.sort_by(|a, b| match b.amount.cmp(&a.amount) {
        std::cmp::Ordering::Equal => a.address.cmp(&b.address),
        o => o,
    });
}

/* ===========================================================
Public API
=========================================================== */

#[allow(unused_assignments)]
pub fn bulk_update_balances_for_block(
    provider: &EssentialsProvider,
    block: &EspoBlock,
) -> Result<()> {
    let debug = debug_enabled();
    let module = "essentials.balances";
    let network = get_network();
    let table = provider.table();
    let search_cfg = AmmDataConfig::load_from_global_config().ok();
    let search_index_enabled = search_cfg.as_ref().map(|c| c.search_index_enabled).unwrap_or(false);
    let mut search_prefix_min =
        search_cfg.as_ref().map(|c| c.search_prefix_min_len as usize).unwrap_or(2);
    let mut search_prefix_max =
        search_cfg.as_ref().map(|c| c.search_prefix_max_len as usize).unwrap_or(6);
    if search_prefix_min == 0 {
        search_prefix_min = 2;
    }
    if search_prefix_max < search_prefix_min {
        search_prefix_max = search_prefix_min;
    }
    let mut ammdata_puts: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut ammdata_deletes: Vec<Vec<u8>> = Vec::new();

    eprintln!("[balances] >>> begin block #{} (txs={})", block.height, block.transactions.len());

    // --------- stats ----------
    let mut stat_outpoints_marked_spent: usize = 0;
    let mut stat_outpoints_written: usize = 0;
    let mut stat_minus_by_alk: BTreeMap<SchemaAlkaneId, u128> = BTreeMap::new();
    let mut stat_plus_by_alk: BTreeMap<SchemaAlkaneId, u128> = BTreeMap::new();
    let mut minted_delta_by_alk: BTreeMap<SchemaAlkaneId, u128> = BTreeMap::new();
    let mut alkane_tx_summaries: Vec<AlkaneTxSummary> = Vec::new();
    let mut alkane_block_txids: Vec<[u8; 32]> = Vec::new();
    let mut alkane_address_txids: HashMap<String, Vec<[u8; 32]>> = HashMap::new();
    let mut latest_trace_txids: Vec<[u8; 32]> = Vec::new();
    let mut transfer_volume_delta: HashMap<SchemaAlkaneId, HashMap<String, u128>> = HashMap::new();
    let mut total_received_delta: HashMap<SchemaAlkaneId, HashMap<String, u128>> = HashMap::new();
    let mut address_activity_transfer_delta: HashMap<String, HashMap<SchemaAlkaneId, u128>> =
        HashMap::new();
    let mut address_activity_received_delta: HashMap<String, HashMap<SchemaAlkaneId, u128>> =
        HashMap::new();

    let push_balance_tx_entry = |map: &mut HashMap<SchemaAlkaneId, Vec<AlkaneBalanceTxEntry>>,
                                 alk: SchemaAlkaneId,
                                 entry: AlkaneBalanceTxEntry| {
        let entries = map.entry(alk).or_default();
        if let Some(existing) = entries.iter_mut().find(|e| e.txid == entry.txid) {
            if existing.outflow.is_empty() && !entry.outflow.is_empty() {
                existing.outflow = entry.outflow;
            }
            if existing.height == 0 && entry.height != 0 {
                existing.height = entry.height;
            }
            return;
        }
        entries.push(entry);
    };
    let push_balance_tx_entry_pair =
        |map: &mut HashMap<(SchemaAlkaneId, SchemaAlkaneId), Vec<AlkaneBalanceTxEntry>>,
         owner: SchemaAlkaneId,
         token: SchemaAlkaneId,
         entry: AlkaneBalanceTxEntry| {
            let entries = map.entry((owner, token)).or_default();
            if let Some(existing) = entries.iter_mut().find(|e| e.txid == entry.txid) {
                if existing.outflow.is_empty() && !entry.outflow.is_empty() {
                    existing.outflow = entry.outflow;
                }
                if existing.height == 0 && entry.height != 0 {
                    existing.height = entry.height;
                }
                return;
            }
            entries.push(entry);
        };

    // holders_delta[alk][addr] = SignedU128 delta
    let mut holders_delta: HashMap<SchemaAlkaneId, BTreeMap<HolderId, SignedU128>> = HashMap::new();
    let mut alkane_balance_delta: HashMap<SchemaAlkaneId, BTreeMap<SchemaAlkaneId, SignedU128>> =
        HashMap::new();
    let mut alkane_balance_tx_entries: HashMap<SchemaAlkaneId, Vec<AlkaneBalanceTxEntry>> =
        HashMap::new();
    let mut alkane_balance_tx_entries_by_token: HashMap<
        (SchemaAlkaneId, SchemaAlkaneId),
        Vec<AlkaneBalanceTxEntry>,
    > = HashMap::new();
    let mut alkane_balance_delta_src: HashMap<
        (SchemaAlkaneId, SchemaAlkaneId),
        AlkaneBalanceTxEntry,
    > = HashMap::new();

    // Records for inputs spent in this block (for persistence w/ tx_spent)
    #[derive(Clone)]
    struct SpentOutpointRecord {
        outpoint: EspoOutpoint,      // original outpoint (tx_spent = None)
        addr: Option<String>,        // resolved address
        balances: Vec<BalanceEntry>, // balances stored on the outpoint
        spk: Option<ScriptBuf>,      // script (for reverse index)
        spent_by: Vec<u8>,           // BE txid of spending tx
    }
    let mut spent_outpoints: HashMap<String, SpentOutpointRecord> = HashMap::new();

    // Ephemeral state for CPFP within the same block
    let mut ephem_outpoint_balances: HashMap<String, Vec<BalanceEntry>> = HashMap::new();
    let mut ephem_outpoint_addr: HashMap<String, String> = HashMap::new();
    let mut ephem_outpoint_spk: HashMap<String, ScriptBuf> = HashMap::new();
    let mut ephem_outpoint_struct: HashMap<String, EspoOutpoint> = HashMap::new();
    let mut consumed_ephem_outpoints: HashMap<String, Vec<u8>> = HashMap::new(); // outpoint_str -> spender txid

    // ---------- Pass A: collect block-created outpoints & external inputs ----------
    let timer = debug::start_if(debug);
    let mut block_created_outs: HashSet<String> = HashSet::new();
    for atx in &block.transactions {
        let tx = &atx.transaction;
        if !tx_has_op_return(tx) {
            continue; // no OP_RETURN → no Alkanes activity on its outputs
        }
        let txid = tx.compute_txid();
        for (vout, _o) in tx.output.iter().enumerate() {
            let op = mk_outpoint(txid.as_byte_array().to_vec(), vout as u32, None);
            block_created_outs.insert(op.as_outpoint_string());
        }
    }

    // Collect all non-ephemeral vins across the block (dedup)
    let mut external_inputs_vec: Vec<EspoOutpoint> = Vec::new();
    let mut external_inputs_set: HashSet<(Vec<u8>, u32)> = HashSet::new();

    for atx in &block.transactions {
        for input in &atx.transaction.input {
            let op = mk_outpoint(
                input.previous_output.txid.as_byte_array().to_vec(),
                input.previous_output.vout,
                None,
            );
            let in_str = op.as_outpoint_string();
            if !block_created_outs.contains(&in_str) {
                let key = (op.txid.clone(), op.vout);
                if external_inputs_set.insert(key) {
                    external_inputs_vec.push(op);
                }
            }
        }
    }

    debug::log_elapsed(module, "pass_a_collect_outpoints", timer);
    // ---------- Pass B: fetch external inputs (batch read) ----------
    let timer = debug::start_if(debug);
    let mut balances_by_outpoint: HashMap<(Vec<u8>, u32), Vec<BalanceEntry>> = HashMap::new();
    let mut addr_by_outpoint: HashMap<(Vec<u8>, u32), String> = HashMap::new();
    let mut spk_by_outpoint: HashMap<(Vec<u8>, u32), ScriptBuf> = HashMap::new();

    if !external_inputs_vec.is_empty() {
        let mut k_balances: Vec<Vec<u8>> = Vec::with_capacity(external_inputs_vec.len());
        let mut k_addr: Vec<Vec<u8>> = Vec::with_capacity(external_inputs_vec.len());
        let mut k_spk: Vec<Vec<u8>> = Vec::with_capacity(external_inputs_vec.len());

        for op in &external_inputs_vec {
            k_balances.push(table.outpoint_balances_key(op)?);
            k_addr.push(table.outpoint_addr_key(op)?);
            k_spk.push(table.utxo_spk_key(op)?);
        }

        let v_balances =
            provider.get_multi_values(GetMultiValuesParams { keys: k_balances })?.values;
        let v_addr = provider.get_multi_values(GetMultiValuesParams { keys: k_addr })?.values;
        let v_spk = provider.get_multi_values(GetMultiValuesParams { keys: k_spk })?.values;

        for (i, op) in external_inputs_vec.iter().enumerate() {
            let key = (op.txid.clone(), op.vout);

            if let Some(bytes) = &v_balances[i] {
                if let Ok(bals) = decode_balances_vec(bytes) {
                    if !bals.is_empty() {
                        balances_by_outpoint.insert(key.clone(), bals);
                    }
                }
            }
            if let Some(addr_bytes) = &v_addr[i] {
                if let Ok(s) = std::str::from_utf8(addr_bytes) {
                    addr_by_outpoint.insert(key.clone(), s.to_string());
                }
            }
            if let Some(spk_bytes) = &v_spk[i] {
                spk_by_outpoint.insert(key, ScriptBuf::from(spk_bytes.clone()));
            }
        }
    }
    debug::log_elapsed(module, "pass_b_fetch_inputs", timer);

    let timer = debug::start_if(debug);
    let mut block_tx_index: HashMap<Txid, usize> = HashMap::new();
    for (idx, atx) in block.transactions.iter().enumerate() {
        block_tx_index.insert(atx.transaction.compute_txid(), idx);
    }

    let mut trace_prevout_txids: Vec<Txid> = Vec::new();
    let mut trace_prevout_set: HashSet<Txid> = HashSet::new();
    for atx in &block.transactions {
        let has_traces = atx.traces.as_ref().map_or(false, |t| !t.is_empty());
        if !has_traces {
            continue;
        }
        for input in &atx.transaction.input {
            if input.previous_output.is_null() {
                continue;
            }
            let prev_txid = input.previous_output.txid;
            if block_tx_index.contains_key(&prev_txid) {
                continue;
            }
            if trace_prevout_set.insert(prev_txid) {
                trace_prevout_txids.push(prev_txid);
            }
        }
    }

    // TODO: extend prevout fallback to all alkane txs (not just traced) for full address coverage.
    debug::log_elapsed(module, "trace_prevout_scan", timer);
    let timer = debug::start_if(debug);
    let mut trace_prev_tx_map: HashMap<Txid, Transaction> = HashMap::new();
    if !trace_prevout_txids.is_empty() {
        let electrum_like = get_electrum_like();
        let start = Instant::now();
        let raw_prev = electrum_like
            .batch_transaction_get_raw(&trace_prevout_txids)
            .unwrap_or_default();
        eprintln!(
            "[balances] traced prevout fetch: block={} prevouts={} elapsed_ms={}",
            block.height,
            trace_prevout_txids.len(),
            start.elapsed().as_millis()
        );
        for (i, raw_prev) in raw_prev.into_iter().enumerate() {
            if raw_prev.is_empty() {
                continue;
            }
            if let Ok(prev_tx) = deserialize::<Transaction>(&raw_prev) {
                trace_prev_tx_map.insert(trace_prevout_txids[i], prev_tx);
            }
        }
    }
    debug::log_elapsed(module, "trace_prevout_fetch", timer);

    // ---------- Main per-tx loop ----------
    let process_timer = debug::start_if(debug);
    for atx in &block.transactions {
        let tx = &atx.transaction;
        let txid = tx.compute_txid();
        let txid_bytes = txid.to_byte_array();
        let mut tx_addrs: HashSet<String> = HashSet::new();
        let mut vin_addrs: HashSet<String> = HashSet::new();
        let mut vout_addrs: HashSet<String> = HashSet::new();
        let mut tx_transfer_amounts_by_alkane: BTreeMap<SchemaAlkaneId, u128> = BTreeMap::new();
        let mut has_alkane_vin = false;
        let has_traces = atx.traces.as_ref().map_or(false, |t| !t.is_empty());
        let mut holder_alkanes_changed: HashSet<SchemaAlkaneId> = HashSet::new();
        let mut local_alkane_delta: HashMap<SchemaAlkaneId, BTreeMap<SchemaAlkaneId, SignedU128>> =
            HashMap::new();
        let mut tx_mint_deltas: BTreeMap<SchemaAlkaneId, u128> = BTreeMap::new();

        let mut add_holder_delta =
            |alk: SchemaAlkaneId,
             holder: HolderId,
             delta: SignedU128,
             holder_changed: &mut HashSet<SchemaAlkaneId>| {
                if delta.is_zero() {
                    return;
                }
                if let HolderId::Alkane(a) = holder {
                    holder_changed.insert(a);
                }
                let entry = holders_delta.entry(alk).or_default();
                let slot = entry.entry(holder.clone()).or_insert_with(SignedU128::zero);
                *slot += delta;
                if slot.is_zero() {
                    entry.remove(&holder);
                }
            };

        // Seed from VIN balances only
        let mut seed_unalloc = Unallocated::default();

        // Gather ephemerals for this tx & apply; for externals, use prefetched maps
        for input in &tx.input {
            let in_op = mk_outpoint(
                input.previous_output.txid.as_byte_array().to_vec(),
                input.previous_output.vout,
                None,
            );
            let in_key = (in_op.txid.clone(), in_op.vout);
            let in_str = in_op.as_outpoint_string();

            if !input.previous_output.is_null() {
                let mut input_addr: Option<String> = None;
                if let Some(idx) = block_tx_index.get(&input.previous_output.txid) {
                    if let Some(prev_out) = block.transactions[*idx]
                        .transaction
                        .output
                        .get(input.previous_output.vout as usize)
                    {
                        input_addr = spk_to_address_str(&prev_out.script_pubkey, network);
                    }
                }
                if input_addr.is_none() {
                    if let Some(addr) = addr_by_outpoint.get(&in_key) {
                        input_addr = Some(addr.clone());
                    } else if let Some(spk) = spk_by_outpoint.get(&in_key) {
                        input_addr = spk_to_address_str(spk, network);
                    }
                }
                if input_addr.is_none() && has_traces {
                    if let Some(prev_tx) = trace_prev_tx_map.get(&input.previous_output.txid) {
                        if let Some(prev_out) =
                            prev_tx.output.get(input.previous_output.vout as usize)
                        {
                            input_addr = spk_to_address_str(&prev_out.script_pubkey, network);
                        }
                    }
                }
                if let Some(addr) = input_addr {
                    tx_addrs.insert(addr.clone());
                    vin_addrs.insert(addr);
                }
            }

            // 1) Ephemeral? (created earlier in this same block)
            if let Some(bals) = ephem_outpoint_balances.get(&in_str) {
                consumed_ephem_outpoints.insert(in_str.clone(), txid.as_byte_array().to_vec());
                has_alkane_vin = true;

                if let Some(addr) = ephem_outpoint_addr.get(&in_str) {
                    tx_addrs.insert(addr.clone());
                    vin_addrs.insert(addr.clone());
                    for be in bals {
                        add_holder_delta(
                            be.alkane,
                            HolderId::Address(addr.clone()),
                            SignedU128::negative(be.amount),
                            &mut holder_alkanes_changed,
                        );
                        *stat_minus_by_alk.entry(be.alkane).or_default() = stat_minus_by_alk
                            .get(&be.alkane)
                            .copied()
                            .unwrap_or(0)
                            .saturating_add(be.amount);
                    }
                    // we only track addr-row deletes for DB-resident rows; ephemerals were not persisted yet
                }
                for be in bals {
                    seed_unalloc.add(be.alkane, be.amount);
                }
                // record for persistence as spent
                let rec = SpentOutpointRecord {
                    outpoint: in_op.clone(),
                    addr: ephem_outpoint_addr.get(&in_str).cloned(),
                    balances: bals.clone(),
                    spk: ephem_outpoint_spk.get(&in_str).cloned(),
                    spent_by: txid.to_byte_array().to_vec(),
                };
                spent_outpoints.entry(in_str.clone()).or_insert(rec);
                continue;
            }

            // 2) External input: resolve from prefetched maps (no DB calls here)
            if let Some(bals) = balances_by_outpoint.get(&in_key).cloned() {
                has_alkane_vin = true;
                // resolve address: /outpoint_addr first, else /utxo_spk → address
                let mut resolved_addr = addr_by_outpoint.get(&in_key).cloned();
                if resolved_addr.is_none() {
                    if let Some(spk) = spk_by_outpoint.get(&in_key) {
                        resolved_addr = spk_to_address_str(spk, network);
                    }
                }

                if let Some(ref addr) = resolved_addr {
                    tx_addrs.insert(addr.clone());
                    vin_addrs.insert(addr.clone());
                    // holders-- and mark legacy addr-row delete
                    for be in &bals {
                        add_holder_delta(
                            be.alkane,
                            HolderId::Address(addr.clone()),
                            SignedU128::negative(be.amount),
                            &mut holder_alkanes_changed,
                        );
                        *stat_minus_by_alk.entry(be.alkane).or_default() = stat_minus_by_alk
                            .get(&be.alkane)
                            .copied()
                            .unwrap_or(0)
                            .saturating_add(be.amount);
                    }
                }

                for be in &bals {
                    seed_unalloc.add(be.alkane, be.amount);
                }

                // record for persistence with spend metadata
                let rec = SpentOutpointRecord {
                    outpoint: in_op.clone(),
                    addr: resolved_addr.clone(),
                    balances: bals.clone(),
                    spk: spk_by_outpoint.get(&in_key).cloned(),
                    spent_by: txid.to_byte_array().to_vec(),
                };
                spent_outpoints.entry(in_str.clone()).or_insert(rec);
            }
            // else: no balances row → nothing to do for this vin
        }

        // apply transfers with your semantics
        let traces_for_tx: Vec<EspoTrace> = atx.traces.clone().unwrap_or_default();
        if !traces_for_tx.is_empty() {
            for t in &traces_for_tx {
                let (ok, deltas) = accumulate_alkane_balance_deltas(
                    &t.sandshrew_trace,
                    &txid,
                    &block.host_function_values,
                );
                if !ok {
                    // Trace-level failure should not discard deltas from other traces in this tx.
                    continue;
                }
                if let Some(mints) =
                    mint_deltas_from_trace(&t.sandshrew_trace, &block.host_function_values)
                {
                    for (alkane, delta) in mints {
                        if delta == 0 {
                            continue;
                        }
                        *tx_mint_deltas.entry(alkane).or_default() =
                            tx_mint_deltas.get(&alkane).copied().unwrap_or(0).saturating_add(delta);
                    }
                }
                for (owner, per_token) in deltas {
                    let entry = local_alkane_delta.entry(owner).or_default();
                    for (tok, delta) in per_token {
                        let slot = entry.entry(tok).or_insert_with(SignedU128::zero);
                        *slot += delta;
                        if slot.is_zero() {
                            entry.remove(&tok);
                        }
                    }
                }
            }
        }
        if !tx_mint_deltas.is_empty() {
            for (alkane, delta) in tx_mint_deltas {
                *minted_delta_by_alk.entry(alkane).or_default() =
                    minted_delta_by_alk.get(&alkane).copied().unwrap_or(0).saturating_add(delta);
            }
        }

        let allocations = if tx_has_op_return(tx) {
            let protostones = parse_protostones(tx)?;
            // apply transfers only when there’s a proto/runestone carrier
            apply_transfers_multi(tx, &protostones, &traces_for_tx, seed_unalloc)?
        } else {
            // No OP_RETURN → no Alkanes allocations (but we already did VIN cleanup/holders--)
            HashMap::<u32, Vec<BalanceEntry>>::new()
        };
        // record outputs ephemerally (for same-block spends)
        for (vout_idx, entries_for_vout) in allocations {
            if entries_for_vout.is_empty() || vout_idx as usize >= tx.output.len() {
                continue;
            }
            let output = &tx.output[vout_idx as usize];
            if is_op_return(&output.script_pubkey) {
                continue;
            }

            if let Some(address_str) = spk_to_address_str(&output.script_pubkey, network) {
                tx_addrs.insert(address_str.clone());
                // Combine duplicates
                let mut amounts_by_alkane: BTreeMap<SchemaAlkaneId, u128> = BTreeMap::new();
                for entry in entries_for_vout {
                    *amounts_by_alkane.entry(entry.alkane).or_default() = amounts_by_alkane
                        .get(&entry.alkane)
                        .copied()
                        .unwrap_or(0)
                        .saturating_add(entry.amount);
                }

                let balances_for_outpoint: Vec<BalanceEntry> = amounts_by_alkane
                    .iter()
                    .map(|(alkane_id, amount)| BalanceEntry { alkane: *alkane_id, amount: *amount })
                    .collect();

                let created_outpoint = mk_outpoint(txid.as_byte_array().to_vec(), vout_idx, None);
                let outpoint_str = created_outpoint.as_outpoint_string();

                // cache for same-block spends
                ephem_outpoint_balances.insert(outpoint_str.clone(), balances_for_outpoint.clone());
                ephem_outpoint_addr.insert(outpoint_str.clone(), address_str.clone());
                ephem_outpoint_spk.insert(outpoint_str.clone(), output.script_pubkey.clone());
                ephem_outpoint_struct.insert(outpoint_str.clone(), created_outpoint.clone());

                // holders++ stats
                for (alkane_id, delta_amount) in amounts_by_alkane {
                    *tx_transfer_amounts_by_alkane.entry(alkane_id).or_default() =
                        tx_transfer_amounts_by_alkane
                            .get(&alkane_id)
                            .copied()
                            .unwrap_or(0)
                            .saturating_add(delta_amount);
                    let total_by_addr = total_received_delta.entry(alkane_id).or_default();
                    *total_by_addr.entry(address_str.clone()).or_default() = total_by_addr
                        .get(&address_str)
                        .copied()
                        .unwrap_or(0)
                        .saturating_add(delta_amount);
                    let activity_by_addr =
                        address_activity_received_delta.entry(address_str.clone()).or_default();
                    *activity_by_addr.entry(alkane_id).or_default() = activity_by_addr
                        .get(&alkane_id)
                        .copied()
                        .unwrap_or(0)
                        .saturating_add(delta_amount);
                    add_holder_delta(
                        alkane_id,
                        HolderId::Address(address_str.clone()),
                        SignedU128::positive(delta_amount),
                        &mut holder_alkanes_changed,
                    );
                    *stat_plus_by_alk.entry(alkane_id).or_default() = stat_plus_by_alk
                        .get(&alkane_id)
                        .copied()
                        .unwrap_or(0)
                        .saturating_add(delta_amount);
                }

                stat_outpoints_written += 1;
            }
        }

        if !tx_transfer_amounts_by_alkane.is_empty() {
            for output in &tx.output {
                if is_op_return(&output.script_pubkey) {
                    continue;
                }
                if let Some(addr) = spk_to_address_str(&output.script_pubkey, network) {
                    vout_addrs.insert(addr);
                }
            }

            let mut participants = vin_addrs;
            participants.extend(vout_addrs);
            if !participants.is_empty() {
                for (alkane_id, amount) in tx_transfer_amounts_by_alkane {
                    let per_addr = transfer_volume_delta.entry(alkane_id).or_default();
                    for addr in participants.iter() {
                        *per_addr.entry(addr.clone()).or_default() =
                            per_addr.get(addr).copied().unwrap_or(0).saturating_add(amount);
                        let activity =
                            address_activity_transfer_delta.entry(addr.clone()).or_default();
                        *activity.entry(alkane_id).or_default() =
                            activity.get(&alkane_id).copied().unwrap_or(0).saturating_add(amount);
                    }
                }
            }
        }
        for (holder_alk, per_token) in &local_alkane_delta {
            let entry_outflow = AlkaneBalanceTxEntry {
                txid: txid_bytes,
                height: block.height,
                outflow: per_token.clone(),
            };
            for (token, delta) in per_token {
                if delta.is_zero() {
                    continue;
                }
                alkane_balance_delta_src.insert((*holder_alk, *token), entry_outflow.clone());
                push_balance_tx_entry_pair(
                    &mut alkane_balance_tx_entries_by_token,
                    *holder_alk,
                    *token,
                    entry_outflow.clone(),
                );
                if *token == *holder_alk {
                    // Keep self-token outflows for summaries/ammdata, but don't persist balances.
                    continue;
                }
                add_holder_delta(
                    *token,
                    HolderId::Alkane(*holder_alk),
                    *delta,
                    &mut holder_alkanes_changed,
                );
                let (is_negative, mag) = delta.as_parts();
                if is_negative {
                    *stat_minus_by_alk.entry(*token).or_default() =
                        stat_minus_by_alk.get(token).copied().unwrap_or(0).saturating_add(mag);
                } else {
                    *stat_plus_by_alk.entry(*token).or_default() =
                        stat_plus_by_alk.get(token).copied().unwrap_or(0).saturating_add(mag);
                }
                let entry = alkane_balance_delta.entry(*holder_alk).or_default();
                let slot = entry.entry(*token).or_insert_with(SignedU128::zero);
                *slot += *delta;
                if slot.is_zero() {
                    entry.remove(token);
                }
            }
        }

        for owner in &holder_alkanes_changed {
            let outflow = local_alkane_delta.get(owner).cloned().unwrap_or_else(BTreeMap::new);
            let entry = AlkaneBalanceTxEntry { txid: txid_bytes, height: block.height, outflow };
            push_balance_tx_entry(&mut alkane_balance_tx_entries, *owner, entry);
        }

        let is_alkane_tx = has_alkane_vin || has_traces;
        if is_alkane_tx {
            for output in &tx.output {
                if is_op_return(&output.script_pubkey) {
                    continue;
                }
                if let Some(addr) = spk_to_address_str(&output.script_pubkey, network) {
                    tx_addrs.insert(addr);
                }
            }

            let mut outflows: Vec<AlkaneBalanceTxEntry> = Vec::new();
            for (_owner, per_token) in &local_alkane_delta {
                if per_token.is_empty() {
                    continue;
                }
                outflows.push(AlkaneBalanceTxEntry {
                    txid: txid_bytes,
                    height: block.height,
                    outflow: per_token.clone(),
                });
            }

            let traces: Vec<EspoSandshrewLikeTrace> = if has_traces {
                atx.traces
                    .as_ref()
                    .map(|list| list.iter().map(|t| t.sandshrew_trace.clone()).collect())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };

            alkane_tx_summaries.push(AlkaneTxSummary {
                txid: txid_bytes,
                traces,
                outflows,
                height: block.height,
            });
            alkane_block_txids.push(txid_bytes);
            for addr in &tx_addrs {
                alkane_address_txids.entry(addr.clone()).or_default().push(txid_bytes);
            }
            if has_traces {
                latest_trace_txids.push(txid_bytes);
            }
        }
    }

    debug::log_elapsed(module, "process_transactions_loop", process_timer);

    // Ensure txid indexes are recorded for every alkane/token delta we are about to persist.
    for ((owner, token), entry) in &alkane_balance_delta_src {
        push_balance_tx_entry(&mut alkane_balance_tx_entries, *owner, entry.clone());
        push_balance_tx_entry_pair(
            &mut alkane_balance_tx_entries_by_token,
            *owner,
            *token,
            entry.clone(),
        );
    }

    // Accumulate alkane holder deltas (alkane -> token) and prepare rows for persistence.
    let timer = debug::start_if(debug);
    let mut alkane_balances_rows: HashMap<SchemaAlkaneId, Vec<BalanceEntry>> = HashMap::new();
    if !alkane_balance_delta.is_empty() {
        let mut owners: Vec<SchemaAlkaneId> = alkane_balance_delta.keys().copied().collect();
        owners.sort();
        let mut keys: Vec<Vec<u8>> = Vec::with_capacity(owners.len());
        for owner in &owners {
            keys.push(table.alkane_balances_key(owner));
        }
        let existing = provider.get_multi_values(GetMultiValuesParams { keys })?.values;

        for (idx, owner) in owners.iter().enumerate() {
            let mut amounts: BTreeMap<SchemaAlkaneId, u128> = BTreeMap::new();

            if let Some(bytes) = &existing.get(idx).and_then(|v| v.as_ref()) {
                if let Ok(entries) = decode_balances_vec(bytes) {
                    for be in entries {
                        if be.amount == 0 {
                            continue;
                        }
                        amounts.insert(be.alkane, be.amount);
                    }
                }
            }

            if let Some(delta_map) = alkane_balance_delta.get(owner) {
                for (token, delta) in delta_map {
                    let (is_negative, mag) = delta.as_parts();
                    if mag == 0 {
                        continue;
                    }
                    let cur = amounts.get(token).copied().unwrap_or(0);
                    let updated = if !is_negative {
                        cur.saturating_add(mag)
                    } else {
                        if mag > cur {
                            let txid_str = alkane_balance_delta_src
                                .get(&(*owner, *token))
                                .map(|entry| Txid::from_byte_array(entry.txid))
                                .map(|t| t.to_string())
                                .unwrap_or_else(|| "unknown".to_string());
                            panic!(
                                "[balances] negative alkane balance detected (txid={}, owner={}:{}, token={}:{}, cur={}, sub={})",
                                txid_str, owner.block, owner.tx, token.block, token.tx, cur, mag
                            );
                        }
                        cur - mag
                    };
                    if updated == 0 {
                        amounts.remove(token);
                    } else {
                        amounts.insert(*token, updated);
                    }
                }
            }

            let mut vec_entries: Vec<BalanceEntry> = amounts
                .into_iter()
                .map(|(alkane, amount)| BalanceEntry { alkane, amount })
                .collect();
            vec_entries
                .sort_by(|a, b| b.amount.cmp(&a.amount).then_with(|| a.alkane.cmp(&b.alkane)));
            alkane_balances_rows.insert(*owner, vec_entries);
        }
    }
    debug::log_elapsed(module, "process_transactions_build_balance_rows", timer);

    let timer = debug::start_if(debug);
    let mut alkane_balance_txs_by_height_row: BTreeMap<SchemaAlkaneId, Vec<AlkaneBalanceTxEntry>> =
        BTreeMap::new();
    if !alkane_balance_tx_entries.is_empty() {
        for (alkane, entries) in &alkane_balance_tx_entries {
            if entries.is_empty() {
                continue;
            }
            alkane_balance_txs_by_height_row.insert(*alkane, entries.clone());
        }
    }
    debug::log_elapsed(module, "process_transactions_build_txs_by_height", timer);

    let timer = debug::start_if(debug);
    let mut address_offsets: HashMap<String, u64> = HashMap::new();
    if !alkane_address_txids.is_empty() {
        let mut addrs: Vec<String> = alkane_address_txids.keys().cloned().collect();
        addrs.sort();
        let mut keys: Vec<Vec<u8>> = Vec::with_capacity(addrs.len());
        for addr in &addrs {
            keys.push(table.alkane_address_len_key(addr));
        }
        let existing = provider.get_multi_values(GetMultiValuesParams { keys })?.values;
        for (idx, addr) in addrs.iter().enumerate() {
            let len = existing
                .get(idx)
                .and_then(|v| v.as_ref())
                .and_then(|b| {
                    if b.len() == 8 {
                        let mut arr = [0u8; 8];
                        arr.copy_from_slice(b);
                        Some(u64::from_le_bytes(arr))
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            address_offsets.insert(addr.clone(), len);
        }
    }
    debug::log_elapsed(module, "process_transactions_address_offsets", timer);

    let timer = debug::start_if(debug);
    let mut latest_traces: Vec<[u8; 32]> = provider
        .get_raw_value(GetRawValueParams { key: table.alkane_latest_traces_key() })?
        .value
        .and_then(|b| Vec::<[u8; 32]>::try_from_slice(&b).ok())
        .unwrap_or_default();
    if !latest_trace_txids.is_empty() {
        for txid in latest_trace_txids {
            latest_traces.insert(0, txid);
        }
        if latest_traces.len() > 20 {
            latest_traces.truncate(20);
        }
    }
    debug::log_elapsed(module, "process_transactions_latest_traces", timer);

    debug::log_elapsed(module, "process_transactions", process_timer);

    // logging metric
    stat_outpoints_marked_spent = spent_outpoints.len();

    // Build unified rows (new outputs + spent inputs)
    let timer = debug::start_if(debug);
    struct NewRow {
        outpoint: EspoOutpoint,
        addr: String,
        enc_balances: Vec<u8>,
        uspk_val: Option<Vec<u8>>, // spk bytes
    }
    let mut new_rows: Vec<NewRow> = Vec::new();

    // map outpoint string -> row data
    let mut row_map: HashMap<String, NewRow> = HashMap::new();

    // Persist block-created outputs (mark as spent if consumed within same block)
    for (out_str, vec_out) in &ephem_outpoint_balances {
        let addr = match ephem_outpoint_addr.get(out_str) {
            Some(a) => a.clone(),
            None => continue,
        };
        let mut op = match ephem_outpoint_struct.get(out_str) {
            Some(o) => o.clone(),
            None => continue,
        };

        if let Some(spender) = consumed_ephem_outpoints.get(out_str) {
            op.tx_spent = Some(spender.clone());
        }

        let enc_balances = encode_vec(vec_out)?;
        let uspk_val = ephem_outpoint_spk.get(out_str).map(|spk| spk.as_bytes().to_vec());

        row_map.insert(out_str.clone(), NewRow { outpoint: op, addr, enc_balances, uspk_val });
    }

    // Persist external inputs (spent) and any ephemerals consumed in-block
    for (out_str, rec) in &spent_outpoints {
        let addr = match &rec.addr {
            Some(a) => a.clone(),
            None => continue,
        };
        let mut op = rec.outpoint.clone();
        op.tx_spent = Some(rec.spent_by.clone());
        let enc_balances = encode_vec(&rec.balances)?;
        let uspk_val = rec.spk.as_ref().map(|spk| spk.as_bytes().to_vec());

        row_map
            .entry(out_str.clone())
            .and_modify(|row| {
                row.outpoint.tx_spent = Some(rec.spent_by.clone());
                if row.uspk_val.is_none() {
                    row.uspk_val = uspk_val.clone();
                }
            })
            .or_insert(NewRow { outpoint: op, addr, enc_balances, uspk_val });
    }

    for (_, row) in row_map {
        new_rows.push(row);
    }
    debug::log_elapsed(module, "process_transactions_build_new_rows", timer);

    // --- Cleanup keys for outpoints that were spent (remove unspent variants) ---
    let mut del_keys_outpoint_balances: Vec<Vec<u8>> = Vec::new();
    let mut del_keys_outpoint_addr: Vec<Vec<u8>> = Vec::new();
    let mut del_keys_utxo_spk: Vec<Vec<u8>> = Vec::new();
    let mut del_keys_addr_balances: Vec<Vec<u8>> = Vec::new();

    for row in &new_rows {
        if row.outpoint.tx_spent.is_some() {
            let unspent = mk_outpoint(row.outpoint.txid.clone(), row.outpoint.vout, None);

            del_keys_outpoint_balances.push(table.outpoint_balances_key(&unspent)?);
            del_keys_outpoint_addr.push(table.outpoint_addr_key(&unspent)?);
            del_keys_utxo_spk.push(table.utxo_spk_key(&unspent)?);
            del_keys_addr_balances.push(table.balances_key(&row.addr, &unspent)?);
        }
    }

    // ---- single write-batch ----
    let mut puts: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut deletes: Vec<Vec<u8>> = Vec::new();

    // A) Address-scoped deletes
    deletes.extend(del_keys_addr_balances);

    // B) Reverse-index cleanup
    deletes.extend(del_keys_outpoint_balances);
    deletes.extend(del_keys_outpoint_addr);
    deletes.extend(del_keys_utxo_spk);

    // C) Persist new outputs (unspent + spent with tx_spent metadata)
    for row in &new_rows {
        let bkey = match table.balances_key(&row.addr, &row.outpoint) {
            Ok(k) => k,
            Err(_) => continue,
        };
        let obkey = match table.outpoint_balances_key(&row.outpoint) {
            Ok(k) => k,
            Err(_) => continue,
        };
        let oaddr_key = match table.outpoint_addr_key(&row.outpoint) {
            Ok(k) => k,
            Err(_) => continue,
        };
        let uspk_key = match table.utxo_spk_key(&row.outpoint) {
            Ok(k) => k,
            Err(_) => continue,
        };

        puts.push((bkey, row.enc_balances.clone()));
        puts.push((obkey, row.enc_balances.clone()));
        puts.push((oaddr_key, row.addr.as_bytes().to_vec()));
        if let Some(ref spk_bytes) = row.uspk_val {
            puts.push((uspk_key, spk_bytes.clone()));
            puts.push((table.addr_spk_key(&row.addr), spk_bytes.clone()));
        }
    }

    // C2) Persist alkane balance index (alkane -> Vec<BalanceEntry>)
    for (owner, entries) in alkane_balances_rows.iter() {
        let key = table.alkane_balances_key(owner);
        let encoded = encode_vec(entries).ok();
        if entries.is_empty() {
            deletes.push(key);
        } else if let Some(buf) = encoded.as_ref() {
            puts.push((key, buf.clone()));
        }
        if let Some(buf) = encoded {
            let height_key = table.alkane_balances_by_height_key(owner, block.height);
            puts.push((height_key, buf));
        }
    }

    // C3) Persist alkane balance change txids (paged)
    let timer = debug::start_if(debug);
    let mut paged_tokens: usize = 0;
    let mut paged_pairs: usize = 0;
    let mut paged_pages: u64 = 0;
    let mut paged_entries: u64 = 0;

    if !alkane_balance_tx_entries.is_empty() {
        let mut tokens: Vec<SchemaAlkaneId> = alkane_balance_tx_entries.keys().copied().collect();
        tokens.sort();
        paged_tokens = tokens.len();
        for tok in &tokens {
            if let Some(new_entries) = alkane_balance_tx_entries.get(tok) {
                let (pages_written, entries_added) = append_paged_balance_txs(
                    provider,
                    table.alkane_balance_txs_meta_key(tok),
                    |page| table.alkane_balance_txs_page_key(tok, page),
                    new_entries,
                    &mut puts,
                )?;
                paged_pages = paged_pages.saturating_add(pages_written);
                paged_entries = paged_entries.saturating_add(entries_added);
            }
        }
    }

    if !alkane_balance_tx_entries_by_token.is_empty() {
        let mut pairs: Vec<(SchemaAlkaneId, SchemaAlkaneId)> =
            alkane_balance_tx_entries_by_token.keys().copied().collect();
        pairs.sort();
        paged_pairs = pairs.len();
        for (owner, token) in &pairs {
            if let Some(new_entries) = alkane_balance_tx_entries_by_token.get(&(*owner, *token)) {
                let (pages_written, entries_added) = append_paged_balance_txs(
                    provider,
                    table.alkane_balance_txs_by_token_meta_key(owner, token),
                    |page| table.alkane_balance_txs_by_token_page_key(owner, token, page),
                    new_entries,
                    &mut puts,
                )?;
                paged_pages = paged_pages.saturating_add(pages_written);
                paged_entries = paged_entries.saturating_add(entries_added);
            }
        }
    }

    if debug {
        eprintln!(
            "[balances] paged_balance_txs stats: tokens={} pairs={} pages_written={} entries_added={}",
            paged_tokens, paged_pairs, paged_pages, paged_entries
        );
    }
    debug::log_elapsed(module, "process_transactions_update_balance_txs_paged", timer);

    // C3c) Persist alkane balance change txids by height
    let height_key = table.alkane_balance_txs_by_height_key(block.height);
    if alkane_balance_txs_by_height_row.is_empty() {
        deletes.push(height_key);
    } else if let Ok(buf) = borsh::to_vec(&alkane_balance_txs_by_height_row) {
        puts.push((height_key, buf));
    }

    // C4) Persist alkane tx summaries + block/address indexes
    for summary in &alkane_tx_summaries {
        if let Ok(buf) = borsh::to_vec(summary) {
            puts.push((table.alkane_tx_summary_key(&summary.txid), buf));
        }
    }

    let block_len = alkane_block_txids.len() as u64;
    puts.push((table.alkane_block_len_key(block.height as u64), block_len.to_le_bytes().to_vec()));
    for (idx, txid_bytes) in alkane_block_txids.iter().enumerate() {
        puts.push((
            table.alkane_block_txid_key(block.height as u64, idx as u64),
            txid_bytes.to_vec(),
        ));
    }

    for (addr, txids) in alkane_address_txids.iter() {
        let start = address_offsets.get(addr).copied().unwrap_or(0);
        for (i, txid_bytes) in txids.iter().enumerate() {
            let idx = start + i as u64;
            puts.push((table.alkane_address_txid_key(addr, idx), txid_bytes.to_vec()));
        }
        let new_len = start + txids.len() as u64;
        puts.push((table.alkane_address_len_key(addr), new_len.to_le_bytes().to_vec()));
    }

    if let Ok(buf) = borsh::to_vec(&latest_traces) {
        if latest_traces.is_empty() {
            deletes.push(table.alkane_latest_traces_key());
        } else {
            puts.push((table.alkane_latest_traces_key(), buf));
        }
    }

    // D) Holders deltas
    for (alkane, per_holder) in holders_delta.iter() {
        let holders_key = table.holders_key(alkane);
        let holders_count_key = table.holders_count_key(alkane);

        let current_holders =
            provider.get_raw_value(GetRawValueParams { key: holders_key.clone() })?.value;
        let mut vec_holders: Vec<HolderEntry> = match current_holders {
            Some(bytes) => decode_holders_vec(&bytes).unwrap_or_default(),
            None => Vec::new(),
        };
        let prev_count = vec_holders.len() as u64;
        for (holder, delta) in per_holder {
            vec_holders = apply_holders_delta(vec_holders, holder, *delta);
        }
        let new_count = vec_holders.len() as u64;
        if search_index_enabled {
            let rec = provider
                .get_creation_record(crate::modules::essentials::storage::GetCreationRecordParams {
                    alkane: *alkane,
                })
                .ok()
                .and_then(|resp| resp.record);
            if let Some(rec) = rec {
                let prefixes = collect_search_prefixes(
                    &rec.names,
                    &rec.symbols,
                    search_prefix_min,
                    search_prefix_max,
                );
                if !prefixes.is_empty() {
                    let mdb = ammdata_mdb();
                    let table_amm = AmmDataTable::new(mdb.as_ref());
                    for prefix in prefixes {
                        ammdata_puts.push((
                            table_amm.token_search_index_key_u64(
                                SearchIndexField::Holders,
                                &prefix,
                                new_count,
                                alkane,
                            ),
                            Vec::new(),
                        ));
                        if prev_count != new_count {
                            ammdata_deletes.push(table_amm.token_search_index_key_u64(
                                SearchIndexField::Holders,
                                &prefix,
                                prev_count,
                                alkane,
                            ));
                        }
                    }
                }
            }
        }
        let new_index_key = table.alkane_holders_ordered_key(new_count, alkane);
        if prev_count != new_count {
            let prev_index_key = table.alkane_holders_ordered_key(prev_count, alkane);
            deletes.push(prev_index_key);
        }
        puts.push((new_index_key, Vec::new()));

        let supply: u128 = vec_holders.iter().map(|h| h.amount).sum();
        let supply_latest_key = table.circulating_supply_latest_key(alkane);
        let prev_supply = provider
            .get_raw_value(GetRawValueParams { key: supply_latest_key.clone() })?
            .value
            .and_then(|v| decode_u128_value(&v).ok())
            .unwrap_or(0);
        if supply != prev_supply {
            let encoded = encode_u128_value(supply)?;
            puts.push((table.circulating_supply_key(alkane, block.height), encoded.clone()));
            puts.push((supply_latest_key, encoded));
        }

        if vec_holders.is_empty() {
            deletes.push(holders_key);
        } else if let Ok((encoded_holders_vec, encoded_holders_count_vec)) =
            get_holders_values_encoded(vec_holders)
        {
            puts.push((holders_key, encoded_holders_vec));
            puts.push((holders_count_key, encoded_holders_count_vec));
        }
    }

    // E) Transfer volume + total received + address activity
    for (alkane, per_addr) in transfer_volume_delta.iter() {
        let key = table.transfer_volume_key(alkane);
        let current = provider.get_raw_value(GetRawValueParams { key: key.clone() })?.value;
        let mut map: HashMap<String, u128> = HashMap::new();
        if let Some(bytes) = current {
            if let Ok(entries) = decode_address_amount_vec(&bytes) {
                for entry in entries {
                    map.insert(entry.address, entry.amount);
                }
            }
        }
        for (addr, delta) in per_addr {
            *map.entry(addr.clone()).or_default() =
                map.get(addr).copied().unwrap_or(0).saturating_add(*delta);
        }
        let mut entries: Vec<AddressAmountEntry> = map
            .into_iter()
            .map(|(address, amount)| AddressAmountEntry { address, amount })
            .collect();
        sort_address_amount_entries(&mut entries);
        if let Ok(buf) = encode_address_amount_vec(&entries) {
            puts.push((key, buf));
        }
    }

    for (alkane, per_addr) in total_received_delta.iter() {
        let key = table.total_received_key(alkane);
        let current = provider.get_raw_value(GetRawValueParams { key: key.clone() })?.value;
        let mut map: HashMap<String, u128> = HashMap::new();
        if let Some(bytes) = current {
            if let Ok(entries) = decode_address_amount_vec(&bytes) {
                for entry in entries {
                    map.insert(entry.address, entry.amount);
                }
            }
        }
        for (addr, delta) in per_addr {
            *map.entry(addr.clone()).or_default() =
                map.get(addr).copied().unwrap_or(0).saturating_add(*delta);
        }
        let mut entries: Vec<AddressAmountEntry> = map
            .into_iter()
            .map(|(address, amount)| AddressAmountEntry { address, amount })
            .collect();
        sort_address_amount_entries(&mut entries);
        if let Ok(buf) = encode_address_amount_vec(&entries) {
            puts.push((key, buf));
        }
    }

    if !address_activity_transfer_delta.is_empty() || !address_activity_received_delta.is_empty() {
        let mut addr_keys: HashSet<String> = HashSet::new();
        addr_keys.extend(address_activity_transfer_delta.keys().cloned());
        addr_keys.extend(address_activity_received_delta.keys().cloned());
        for addr in addr_keys {
            let key = table.address_activity_key(&addr);
            let current = provider.get_raw_value(GetRawValueParams { key: key.clone() })?.value;
            let mut entry = match current {
                Some(bytes) => decode_address_activity_entry(&bytes).unwrap_or_default(),
                None => AddressActivityEntry::default(),
            };
            if let Some(per_alk) = address_activity_transfer_delta.get(&addr) {
                for (alk, delta) in per_alk {
                    *entry.transfer_volume.entry(*alk).or_default() =
                        entry.transfer_volume.get(alk).copied().unwrap_or(0).saturating_add(*delta);
                }
            }
            if let Some(per_alk) = address_activity_received_delta.get(&addr) {
                for (alk, delta) in per_alk {
                    *entry.total_received.entry(*alk).or_default() =
                        entry.total_received.get(alk).copied().unwrap_or(0).saturating_add(*delta);
                }
            }
            if let Ok(buf) = encode_address_activity_entry(&entry) {
                puts.push((key, buf));
            }
        }
    }

    for (alkane, delta) in minted_delta_by_alk.iter() {
        if *delta == 0 {
            continue;
        }
        let latest_key = table.total_minted_latest_key(alkane);
        let prev_total = provider
            .get_raw_value(GetRawValueParams { key: latest_key.clone() })?
            .value
            .and_then(|v| decode_u128_value(&v).ok())
            .unwrap_or(0);
        let new_total = prev_total.saturating_add(*delta);
        let encoded = encode_u128_value(new_total)?;
        puts.push((table.total_minted_key(alkane, block.height), encoded.clone()));
        puts.push((latest_key, encoded));
    }

    debug::log_elapsed(module, "build_writes", timer);
    let timer = debug::start_if(debug);
    let check_balances = strict_check_alkane_balances();
    let check_utxos = strict_check_utxos();
    if check_balances || check_utxos {
        let metashrew = get_metashrew();
        let height_u64 = block.height as u64;
        let metashrew_sdb = get_metashrew_sdb();
        metashrew_sdb
            .catch_up_now()
            .context("metashrew catch_up before strict checks")?;
        let sdb = metashrew_sdb.as_ref();

        let mut balance_mismatches: Vec<(SchemaAlkaneId, SchemaAlkaneId, u128, u128)> = Vec::new();
        if check_balances {
            let balances_from_rows = |owner: &SchemaAlkaneId| -> HashMap<SchemaAlkaneId, u128> {
                let entries = alkane_balances_rows.get(owner).unwrap_or_else(|| {
                    panic!(
                        "[balances][strict] missing prewrite balances (owner={}:{})",
                        owner.block, owner.tx
                    )
                });
                let mut agg: HashMap<SchemaAlkaneId, u128> = HashMap::new();
                for entry in entries {
                    if entry.amount == 0 {
                        continue;
                    }
                    *agg.entry(entry.alkane).or_default() =
                        agg.get(&entry.alkane).copied().unwrap_or(0).saturating_add(entry.amount);
                }
                if let Some(self_balance) = lookup_self_balance(owner) {
                    if self_balance == 0 {
                        agg.remove(owner);
                    } else {
                        agg.insert(*owner, self_balance);
                    }
                }
                agg
            };

            let mut local_cache: HashMap<SchemaAlkaneId, HashMap<SchemaAlkaneId, u128>> =
                HashMap::new();

            let mut changed_pairs: Vec<(SchemaAlkaneId, SchemaAlkaneId)> = Vec::new();
            for (owner, per_token) in &alkane_balance_delta {
                for (token, delta) in per_token {
                    if delta.is_zero() {
                        continue;
                    }
                    changed_pairs.push((*owner, *token));
                }
            }
            changed_pairs.sort();
            changed_pairs.dedup();

            for (owner, token) in changed_pairs {
                if !local_cache.contains_key(&owner) {
                    let balances = balances_from_rows(&owner);
                    local_cache.insert(owner, balances);
                }
                let local_balance =
                    local_cache.get(&owner).and_then(|m| m.get(&token).copied()).unwrap_or(0);

                let metashrew_balance = match metashrew.get_reserves_for_alkane_with_db(
                    sdb,
                    &owner,
                    &token,
                    Some(height_u64),
                ) {
                    Ok(Some(bal)) => bal,
                    Ok(None) => 0,
                    Err(e) => {
                        panic!(
                            "[balances][strict] metashrew lookup failed (owner={}:{}, token={}:{}, height={}): {e:?}",
                            owner.block, owner.tx, token.block, token.tx, height_u64
                        );
                    }
                };

                if local_balance != metashrew_balance {
                    balance_mismatches.push((owner, token, local_balance, metashrew_balance));
                }
            }
        }

        struct UtxoMismatch {
            outpoint: EspoOutpoint,
            addr: String,
            local: BTreeMap<SchemaAlkaneId, u128>,
            metashrew: BTreeMap<SchemaAlkaneId, u128>,
        }
        let mut utxo_mismatches: Vec<UtxoMismatch> = Vec::new();
        if check_utxos {
            let to_balance_map = |entries: &[BalanceEntry]| -> BTreeMap<SchemaAlkaneId, u128> {
                let mut out = BTreeMap::new();
                for entry in entries {
                    if entry.amount == 0 {
                        continue;
                    }
                    *out.entry(entry.alkane).or_default() = out
                        .get(&entry.alkane)
                        .copied()
                        .unwrap_or(0u128)
                        .saturating_add(entry.amount);
                }
                out
            };
            let parse_txid = |txid_bytes: &[u8]| -> Result<Txid> {
                if txid_bytes.len() != 32 {
                    return Err(anyhow!("invalid txid length {}", txid_bytes.len()));
                }
                let mut arr = [0u8; 32];
                arr.copy_from_slice(txid_bytes);
                Ok(Txid::from_byte_array(arr))
            };

            for row in &new_rows {
                if row.outpoint.tx_spent.is_some() {
                    continue;
                }
                let txid = parse_txid(&row.outpoint.txid).unwrap_or_else(|e| {
                    panic!(
                        "[balances][strict] invalid outpoint txid bytes ({}:{}): {e}",
                        row.outpoint.as_outpoint_string(),
                        row.outpoint.vout
                    )
                });
                let local_entries = decode_balances_vec(&row.enc_balances).unwrap_or_else(|e| {
                    panic!(
                        "[balances][strict] decode outpoint balances failed ({}:{}): {e:?}",
                        row.outpoint.as_outpoint_string(),
                        row.outpoint.vout
                    )
                });
                let local_map = to_balance_map(&local_entries);

                let meta_entries = metashrew
                    .get_outpoint_alkane_balances_with_db(sdb, &txid, row.outpoint.vout)
                    .unwrap_or_else(|e| {
                        panic!(
                            "[balances][strict] metashrew outpoint lookup failed ({}:{}): {e:?}",
                            row.outpoint.as_outpoint_string(),
                            row.outpoint.vout
                        )
                    });
                let mut meta_map = BTreeMap::new();
                for (id, amount) in meta_entries {
                    if amount == 0 {
                        continue;
                    }
                    let schema = schema_id_from_parts(id.block, id.tx).unwrap_or_else(|e| {
                        panic!(
                            "[balances][strict] invalid metashrew alkane id ({}:{}): {e:?}",
                            id.block, id.tx
                        )
                    });
                    *meta_map.entry(schema).or_default() =
                        meta_map.get(&schema).copied().unwrap_or(0u128).saturating_add(amount);
                }

                if local_map != meta_map {
                    utxo_mismatches.push(UtxoMismatch {
                        outpoint: row.outpoint.clone(),
                        addr: row.addr.clone(),
                        local: local_map,
                        metashrew: meta_map,
                    });
                }
            }
        }

        if !balance_mismatches.is_empty() || !utxo_mismatches.is_empty() {
            if check_balances {
                let mut height_history_cache: HashMap<
                    SchemaAlkaneId,
                    Vec<(u32, Vec<BalanceEntry>)>,
                > = HashMap::new();
                let mut self_balance_cache: HashMap<SchemaAlkaneId, Option<u128>> = HashMap::new();

                let balance_for_token = |entries: &[BalanceEntry],
                                         owner: &SchemaAlkaneId,
                                         token: &SchemaAlkaneId,
                                         self_balance: Option<u128>|
                 -> u128 {
                    let mut amount = 0u128;
                    for entry in entries {
                        if entry.alkane == *token {
                            amount = amount.saturating_add(entry.amount);
                        }
                    }
                    if *token == *owner {
                        if let Some(self_balance) = self_balance {
                            return self_balance;
                        }
                    }
                    amount
                };

                let mut find_mismatch_origin = |owner: &SchemaAlkaneId,
                                                token: &SchemaAlkaneId,
                                                current_balance: u128|
                 -> Option<(u32, u128, u128, bool)> {
                    let history = if let Some(cached) = height_history_cache.get(owner) {
                        cached.clone()
                    } else {
                        let prefix = table.alkane_balances_by_height_prefix(owner);
                        let keys = match provider
                            .get_scan_prefix(GetScanPrefixParams { prefix: prefix.clone() })
                        {
                            Ok(v) => v.keys,
                            Err(_) => Vec::new(),
                        };
                        if keys.is_empty() {
                            return None;
                        }
                        let values = match provider
                            .get_multi_values(GetMultiValuesParams { keys: keys.clone() })
                        {
                            Ok(v) => v.values,
                            Err(_) => Vec::new(),
                        };
                        let mut entries_by_height: Vec<(u32, Vec<BalanceEntry>)> = Vec::new();
                        for (key, value) in keys.iter().zip(values) {
                            let Some(bytes) = value else {
                                continue;
                            };
                            if !key.starts_with(&prefix) {
                                continue;
                            }
                            let suffix = &key[prefix.len()..];
                            if suffix.len() != 4 {
                                continue;
                            }
                            let mut arr = [0u8; 4];
                            arr.copy_from_slice(suffix);
                            let height = u32::from_be_bytes(arr);
                            if let Ok(list) = decode_balances_vec(&bytes) {
                                entries_by_height.push((height, list));
                            }
                        }
                        if entries_by_height.is_empty() {
                            return None;
                        }
                        entries_by_height.sort_by_key(|(h, _)| *h);
                        height_history_cache.insert(*owner, entries_by_height.clone());
                        entries_by_height
                    };

                    if history.is_empty() {
                        return None;
                    }

                    let self_balance = *self_balance_cache
                        .entry(*owner)
                        .or_insert_with(|| lookup_self_balance(owner));

                    let mut snapshots: Vec<(u32, u128)> = history
                        .iter()
                        .map(|(height, entries)| {
                            (*height, balance_for_token(entries, owner, token, self_balance))
                        })
                        .collect();
                    snapshots.sort_by_key(|(h, _)| *h);
                    snapshots.dedup_by_key(|(h, _)| *h);
                    let current_height = block.height;
                    snapshots.retain(|(h, _)| *h <= current_height);

                    if let Some(last) = snapshots.last_mut() {
                        if last.0 == current_height {
                            last.1 = current_balance;
                        } else if last.0 < current_height {
                            snapshots.push((current_height, current_balance));
                        }
                    } else {
                        return None;
                    }

                    #[derive(Clone, Copy)]
                    struct Segment {
                        start: u32,
                        end: u32,
                        balance: u128,
                    }

                    let mut segments: Vec<Segment> = Vec::with_capacity(snapshots.len());
                    for idx in 0..snapshots.len() {
                        let (start, balance) = snapshots[idx];
                        let end = if idx + 1 < snapshots.len() {
                            let next_start = snapshots[idx + 1].0;
                            if next_start == 0 { 0 } else { next_start.saturating_sub(1) }
                        } else {
                            current_height
                        };
                        if end < start {
                            continue;
                        }
                        segments.push(Segment { start, end, balance });
                    }

                    if segments.is_empty() {
                        return None;
                    }

                    let mut meta_cache: HashMap<u32, u128> = HashMap::new();
                    let mut metashrew_at = |height: u32| -> u128 {
                        if let Some(val) = meta_cache.get(&height).copied() {
                            return val;
                        }
                        let height_u64 = height as u64;
                        let value = match metashrew.get_reserves_for_alkane_with_db(
                            sdb,
                            owner,
                            token,
                            Some(height_u64),
                        ) {
                            Ok(Some(bal)) => bal,
                            Ok(None) => 0,
                            Err(e) => {
                                panic!(
                                    "[balances][strict] metashrew lookup failed (owner={}:{}, token={}:{}, height={}): {e:?}",
                                    owner.block, owner.tx, token.block, token.tx, height_u64
                                );
                            }
                        };
                        meta_cache.insert(height, value);
                        value
                    };

                    for idx in (0..segments.len()).rev() {
                        let seg = segments[idx];
                        let meta_start = metashrew_at(seg.start);
                        if meta_start == seg.balance {
                            let mut lo = seg.start;
                            let mut hi = seg.end;
                            while lo < hi {
                                let mid = lo + (hi - lo) / 2;
                                let meta_mid = metashrew_at(mid);
                                if meta_mid == seg.balance {
                                    lo = mid + 1;
                                } else {
                                    hi = mid;
                                }
                            }
                            let meta_at = metashrew_at(lo);
                            return Some((lo, seg.balance, meta_at, true));
                        }

                        if idx == 0 || seg.start == 0 {
                            return Some((seg.start, seg.balance, meta_start, false));
                        }

                        let prev_end = seg.start - 1;
                        let prev_balance = segments[idx - 1].balance;
                        let meta_prev_end = metashrew_at(prev_end);
                        if meta_prev_end == prev_balance {
                            return Some((seg.start, seg.balance, meta_start, true));
                        }
                    }

                    None
                };

                for (owner, token, local_balance, metashrew_balance) in &balance_mismatches {
                    eprintln!(
                        "[balances][strict] mismatch height={} owner={}:{} token={}:{} local={} metashrew={}",
                        height_u64,
                        owner.block,
                        owner.tx,
                        token.block,
                        token.tx,
                        local_balance,
                        metashrew_balance
                    );

                    let mut txids: Vec<String> = alkane_balance_tx_entries_by_token
                        .get(&(*owner, *token))
                        .map(|entries| {
                            entries
                                .iter()
                                .map(|entry| Txid::from_byte_array(entry.txid).to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    txids.sort();
                    txids.dedup();

                    if txids.is_empty() {
                        eprintln!(
                            "[balances][strict] balance-change txids: none (owner={}:{}, token={}:{})",
                            owner.block, owner.tx, token.block, token.tx
                        );
                    } else {
                        eprintln!("[balances][strict] balance-change txids: {}", txids.join(","));
                    }

                    if let Some((first_height, local_at, meta_at, exact)) =
                        find_mismatch_origin(owner, token, *local_balance)
                    {
                        if exact {
                            eprintln!(
                                "[balances][strict] mismatch origin height={} owner={}:{} token={}:{} local={} metashrew={}",
                                first_height,
                                owner.block,
                                owner.tx,
                                token.block,
                                token.tx,
                                local_at,
                                meta_at
                            );
                        } else {
                            eprintln!(
                                "[balances][strict] mismatch origin at or before height={} owner={}:{} token={}:{} local={} metashrew={}",
                                first_height,
                                owner.block,
                                owner.tx,
                                token.block,
                                token.tx,
                                local_at,
                                meta_at
                            );
                        }
                    }
                }
            }

            if check_utxos {
                let fmt_sheet = |sheet: &BTreeMap<SchemaAlkaneId, u128>| -> String {
                    if sheet.is_empty() {
                        return "empty".to_string();
                    }
                    sheet
                        .iter()
                        .map(|(id, amt)| format!("{}:{}={}", id.block, id.tx, amt))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                for mismatch in &utxo_mismatches {
                    eprintln!(
                        "[balances][strict] utxo mismatch outpoint={} addr={} local=[{}] metashrew=[{}]",
                        mismatch.outpoint.as_outpoint_string(),
                        mismatch.addr,
                        fmt_sheet(&mismatch.local),
                        fmt_sheet(&mismatch.metashrew)
                    );
                }
            }

            panic!(
                "[balances][strict] metashrew mismatch at height {} (alkanes={} utxos={})",
                height_u64,
                balance_mismatches.len(),
                utxo_mismatches.len()
            );
        }
    }
    debug::log_elapsed(module, "strict_mode_checks", timer);

    let timer = debug::start_if(debug);
    provider.set_batch(SetBatchParams { puts, deletes })?;
    debug::log_elapsed(module, "write_batch", timer);

    if search_index_enabled && (!ammdata_puts.is_empty() || !ammdata_deletes.is_empty()) {
        let mdb = ammdata_mdb();
        let res = mdb.bulk_write(|wb| {
            for key in &ammdata_deletes {
                wb.delete(key);
            }
            for (key, value) in &ammdata_puts {
                wb.put(key, value);
            }
        });
        if let Err(e) = res {
            eprintln!(
                "[balances] ammdata search index write failed at height {}: {e}",
                block.height
            );
        }
    }

    let minus_total: u128 = stat_minus_by_alk.values().copied().sum();
    let plus_total: u128 = stat_plus_by_alk.values().copied().sum();

    eprintln!(
        "[balances] block #{}, txs={}, outpoints_written={}, outpoints_marked_spent={}, alkanes_added={}, alkanes_removed={}, unique_add={}, unique_remove={}",
        block.height,
        block.transactions.len(),
        stat_outpoints_written,
        stat_outpoints_marked_spent,
        plus_total,
        minus_total,
        stat_plus_by_alk.len(),
        stat_minus_by_alk.len()
    );
    eprintln!("[balances] <<< end   block #{}", block.height);

    Ok(())
}

fn lookup_self_balance(alk: &SchemaAlkaneId) -> Option<u128> {
    match get_metashrew().get_reserves_for_alkane(alk, alk, None) {
        Ok(val) => val,
        Err(e) => {
            eprintln!(
                "[balances] WARN: self-balance lookup failed for {}:{} ({e:?})",
                alk.block, alk.tx
            );
            None
        }
    }
}

pub fn get_balance_for_address(
    provider: &EssentialsProvider,
    address: &str,
) -> Result<HashMap<SchemaAlkaneId, u128>> {
    let mut prefix = b"/balances/".to_vec();
    prefix.extend_from_slice(address.as_bytes());
    prefix.push(b'/');

    let keys = provider.get_scan_prefix(GetScanPrefixParams { prefix: prefix.clone() })?.keys;
    let vals = provider.get_multi_values(GetMultiValuesParams { keys: keys.clone() })?.values;

    let mut agg: HashMap<SchemaAlkaneId, u128> = HashMap::new();
    for (k, v) in keys.iter().zip(vals) {
        if let Some(bytes) = v {
            // decode outpoint from key to determine spend status
            if k.len() <= prefix.len() {
                continue;
            }
            let Ok(op) = EspoOutpoint::try_from_slice(&k[prefix.len()..]) else { continue };
            if op.tx_spent.is_some() {
                continue; // skip spent outpoints when computing live balance
            }
            if let Ok(bals) = decode_balances_vec(&bytes) {
                for be in bals {
                    *agg.entry(be.alkane).or_default() =
                        agg.get(&be.alkane).copied().unwrap_or(0).saturating_add(be.amount);
                }
            }
        }
    }
    Ok(agg)
}

pub fn get_alkane_balances(
    provider: &EssentialsProvider,
    owner: &SchemaAlkaneId,
) -> Result<HashMap<SchemaAlkaneId, u128>> {
    let table = provider.table();
    let key = table.alkane_balances_key(owner);
    let mut agg: HashMap<SchemaAlkaneId, u128> = HashMap::new();

    if let Some(bytes) = provider.get_raw_value(GetRawValueParams { key })?.value {
        if let Ok(bals) = decode_balances_vec(&bytes) {
            for be in bals {
                if be.amount == 0 {
                    continue;
                }
                *agg.entry(be.alkane).or_default() =
                    agg.get(&be.alkane).copied().unwrap_or(0).saturating_add(be.amount);
            }
        }
    }

    if let Some(self_balance) = lookup_self_balance(owner) {
        if self_balance == 0 {
            agg.remove(owner);
        } else {
            agg.insert(*owner, self_balance);
        }
    }

    Ok(agg)
}

pub fn get_alkane_balances_at_or_before(
    provider: &EssentialsProvider,
    owner: &SchemaAlkaneId,
    height: u32,
) -> Result<(HashMap<SchemaAlkaneId, u128>, Option<u32>)> {
    let table = provider.table();

    let decode_agg = |bytes: &[u8]| -> HashMap<SchemaAlkaneId, u128> {
        let mut agg: HashMap<SchemaAlkaneId, u128> = HashMap::new();
        if let Ok(bals) = decode_balances_vec(bytes) {
            for be in bals {
                if be.amount == 0 {
                    continue;
                }
                *agg.entry(be.alkane).or_default() =
                    agg.get(&be.alkane).copied().unwrap_or(0).saturating_add(be.amount);
            }
        }
        agg
    };

    let exact_key = table.alkane_balances_by_height_key(owner, height);
    if let Some(bytes) = provider.get_raw_value(GetRawValueParams { key: exact_key })?.value {
        return Ok((decode_agg(&bytes), Some(height)));
    }

    let prefix = table.alkane_balances_by_height_prefix(owner);
    let entries = provider
        .get_iter_prefix_rev(GetIterPrefixRevParams { prefix: prefix.clone() })?
        .entries;

    for (key, value) in entries {
        if !key.starts_with(&prefix) {
            continue;
        }
        let suffix = &key[prefix.len()..];
        if suffix.len() != 4 {
            continue;
        }
        let mut arr = [0u8; 4];
        arr.copy_from_slice(suffix);
        let snapshot_height = u32::from_be_bytes(arr);
        if snapshot_height > height {
            continue;
        }
        return Ok((decode_agg(&value), Some(snapshot_height)));
    }

    Ok((HashMap::new(), None))
}

#[derive(Default, Clone, Debug)]
pub struct OutpointLookup {
    pub balances: Vec<BalanceEntry>,
    pub spent_by: Option<Txid>,
}

pub fn get_outpoint_balances(
    provider: &EssentialsProvider,
    txid: &Txid,
    vout: u32,
) -> Result<Vec<BalanceEntry>> {
    let table = provider.table();
    let pref = table.outpoint_balances_prefix(txid.as_byte_array().as_slice(), vout)?;
    let keys = provider.get_scan_prefix(GetScanPrefixParams { prefix: pref.clone() })?.keys;
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let vals = provider.get_multi_values(GetMultiValuesParams { keys: keys.clone() })?.values;
    for (_k, v) in keys.into_iter().zip(vals) {
        if let Some(bytes) = v {
            if let Ok(bals) = decode_balances_vec(&bytes) {
                return Ok(bals);
            }
        }
    }
    Ok(Vec::new())
}

pub fn get_outpoint_balances_with_spent(
    provider: &EssentialsProvider,
    txid: &Txid,
    vout: u32,
) -> Result<OutpointLookup> {
    const OUTPOINT_BALANCES_PREFIX: &[u8] = b"/outpoint_balances/";

    let table = provider.table();
    let pref = table.outpoint_balances_prefix(txid.as_byte_array().as_slice(), vout)?;
    let keys = provider.get_scan_prefix(GetScanPrefixParams { prefix: pref.clone() })?.keys;
    if keys.is_empty() {
        return Ok(OutpointLookup::default());
    }

    let vals = provider.get_multi_values(GetMultiValuesParams { keys: keys.clone() })?.values;
    let mut fallback: Option<OutpointLookup> = None;

    for (k, v) in keys.into_iter().zip(vals) {
        let Some(bytes) = v else { continue };

        let spent_by = if k.len() > OUTPOINT_BALANCES_PREFIX.len() {
            EspoOutpoint::try_from_slice(&k[OUTPOINT_BALANCES_PREFIX.len()..])
                .ok()
                .and_then(|op| op.tx_spent.and_then(|t| Txid::from_slice(&t).ok()))
        } else {
            None
        };

        if let Ok(balances) = decode_balances_vec(&bytes) {
            let lookup = OutpointLookup { balances, spent_by };
            if lookup.spent_by.is_some() {
                return Ok(lookup);
            }
            if fallback.is_none() {
                fallback = Some(lookup);
            }
        }
    }

    Ok(fallback.unwrap_or_default())
}

pub fn get_outpoint_balances_with_spent_batch(
    provider: &EssentialsProvider,
    outpoints: &[(Txid, u32)],
) -> Result<HashMap<(Txid, u32), OutpointLookup>> {
    const OUTPOINT_BALANCES_PREFIX: &[u8] = b"/outpoint_balances/";

    let mut key_map: Vec<(Vec<u8>, Txid, u32)> = Vec::new();
    let table = provider.table();
    for (txid, vout) in outpoints {
        let pref = table.outpoint_balances_prefix(txid.as_byte_array().as_slice(), *vout)?;
        let keys = provider.get_scan_prefix(GetScanPrefixParams { prefix: pref.clone() })?.keys;
        for k in keys {
            key_map.push((k, *txid, *vout));
        }
    }

    let mut out: HashMap<(Txid, u32), OutpointLookup> = HashMap::new();
    if key_map.is_empty() {
        return Ok(out);
    }

    let keys: Vec<Vec<u8>> = key_map.iter().map(|(k, _, _)| k.clone()).collect();
    let vals = provider.get_multi_values(GetMultiValuesParams { keys: keys.clone() })?.values;

    for ((k, txid, vout), val) in key_map.into_iter().zip(vals.into_iter()) {
        let Some(bytes) = val else { continue };
        let spent_by = if k.len() > OUTPOINT_BALANCES_PREFIX.len() {
            EspoOutpoint::try_from_slice(&k[OUTPOINT_BALANCES_PREFIX.len()..])
                .ok()
                .and_then(|op| op.tx_spent.and_then(|t| Txid::from_slice(&t).ok()))
        } else {
            None
        };
        if let Ok(balances) = decode_balances_vec(&bytes) {
            let entry = OutpointLookup { balances, spent_by };
            let slot = out.entry((txid, vout)).or_insert_with(OutpointLookup::default);
            if entry.spent_by.is_some() || slot.balances.is_empty() {
                *slot = entry;
            }
        }
    }

    for (txid, vout) in outpoints {
        out.entry((*txid, *vout)).or_insert_with(OutpointLookup::default);
    }

    Ok(out)
}

pub fn get_holders_for_alkane(
    provider: &EssentialsProvider,
    alk: SchemaAlkaneId,
    page: usize,
    limit: usize,
) -> Result<(usize /*total*/, u128 /*supply*/, Vec<HolderEntry>)> {
    let table = provider.table();
    let key = table.holders_key(&alk);
    let cur = provider.get_raw_value(GetRawValueParams { key })?.value;
    let mut all = match cur {
        Some(bytes) => decode_holders_vec(&bytes).unwrap_or_default(),
        None => Vec::new(),
    };
    if let Some(self_balance) = lookup_self_balance(&alk) {
        if self_balance > 0 {
            if let Some(existing) = all.iter_mut().find(|h| h.holder == HolderId::Alkane(alk)) {
                existing.amount = self_balance;
            } else {
                all.push(HolderEntry { holder: HolderId::Alkane(alk), amount: self_balance });
            }
        } else {
            all.retain(|h| h.holder != HolderId::Alkane(alk));
        }
    }

    all.sort_by(|a, b| match b.amount.cmp(&a.amount) {
        std::cmp::Ordering::Equal => holder_order_key(&a.holder).cmp(&holder_order_key(&b.holder)),
        o => o,
    });
    let total = all.len();
    let supply: u128 = all.iter().map(|h| h.amount).sum();
    let p = page.max(1);
    let l = limit.max(1);
    let off = l.saturating_mul(p - 1);
    let end = (off + l).min(total);
    let slice = if off >= total { vec![] } else { all[off..end].to_vec() };
    Ok((total, supply, slice))
}

pub fn get_transfer_volume_for_alkane(
    provider: &EssentialsProvider,
    alk: SchemaAlkaneId,
    page: usize,
    limit: usize,
) -> Result<(usize, Vec<AddressAmountEntry>)> {
    let table = provider.table();
    let key = table.transfer_volume_key(&alk);
    let cur = provider.get_raw_value(GetRawValueParams { key })?.value;
    let mut all = match cur {
        Some(bytes) => decode_address_amount_vec(&bytes).unwrap_or_default(),
        None => Vec::new(),
    };
    sort_address_amount_entries(&mut all);
    let total = all.len();
    let p = page.max(1);
    let l = limit.max(1);
    let off = l.saturating_mul(p - 1);
    let end = (off + l).min(total);
    let slice = if off >= total { vec![] } else { all[off..end].to_vec() };
    Ok((total, slice))
}

pub fn get_total_received_for_alkane(
    provider: &EssentialsProvider,
    alk: SchemaAlkaneId,
    page: usize,
    limit: usize,
) -> Result<(usize, Vec<AddressAmountEntry>)> {
    let table = provider.table();
    let key = table.total_received_key(&alk);
    let cur = provider.get_raw_value(GetRawValueParams { key })?.value;
    let mut all = match cur {
        Some(bytes) => decode_address_amount_vec(&bytes).unwrap_or_default(),
        None => Vec::new(),
    };
    sort_address_amount_entries(&mut all);
    let total = all.len();
    let p = page.max(1);
    let l = limit.max(1);
    let off = l.saturating_mul(p - 1);
    let end = (off + l).min(total);
    let slice = if off >= total { vec![] } else { all[off..end].to_vec() };
    Ok((total, slice))
}

pub fn get_address_activity_for_address(
    provider: &EssentialsProvider,
    address: &str,
) -> Result<AddressActivityEntry> {
    let table = provider.table();
    let key = table.address_activity_key(address);
    let cur = provider.get_raw_value(GetRawValueParams { key })?.value;
    let entry = match cur {
        Some(bytes) => decode_address_activity_entry(&bytes).unwrap_or_default(),
        None => AddressActivityEntry::default(),
    };
    Ok(entry)
}

pub fn get_scriptpubkey_for_address(
    provider: &EssentialsProvider,
    addr: &str,
) -> Result<Option<ScriptBuf>> {
    let table = provider.table();
    let key = table.addr_spk_key(addr);
    let v = provider.get_raw_value(GetRawValueParams { key })?.value;
    Ok(v.map(ScriptBuf::from))
}
