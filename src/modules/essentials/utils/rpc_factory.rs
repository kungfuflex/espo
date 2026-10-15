//! `metashrew_source = rpc`: wasm + template (`inspection.factory_alkane`) for a
//! newly created alkane, without the local `/alkanes/<id>` alias chain.
//!
//! The local reader loads `/alkanes/<id>`; a 32-byte payload is an alias to the
//! template it was cloned from, followed until a real binary is found, and that
//! binary's id is the template. `getbytecode` follows the same chain but does
//! not say where it ended, so the template is reconstructed:
//!
//! * **Own binary** — created by a top-level `[1,0]` (CREATE, new `2:n`) or
//!   `[3,n]` (CREATERESERVED, `4:n`) cellpack: the template is the alkane itself.
//!   Exact.
//! * **Top-level clone** — `[5,n]` / `[6,n]`: the runtime stores an alias to
//!   `2:n` / `4:n` (alkanes `run_special_cellpacks`), so the template is that
//!   id's own template, read from espo's creation record for it. Exact.
//! * **Clone made inside a contract** (an extcall to `[5,n]`/`[6,n]`): the trace
//!   does not carry the extcall's target. The template is the binary-holder whose
//!   code is byte-identical. Exact when exactly one holder has that code; when
//!   several do (33 such twin groups on mainnet as of 971500), the earliest
//!   created twin is taken and the choice is logged as `ambiguous`. This is the
//!   one place RPC mode can differ from the local reader, and only in which twin
//!   `factory_alkane` names — the wasm, and so every inspected method, is the
//!   same bytes either way.
//!
//! The `factory_hint` path (orbitals writing `/factory_id`) mirrors the local
//! reader: load the hint's code, and its template is the hint's own template.

use crate::alkanes::metashrew_rpc::MetashrewRpc;
use crate::alkanes::trace::{EspoBlock, EspoTrace};
use crate::modules::essentials::storage::{
    EssentialsProvider, GetCreationRecordParams, GetCreationRecordsOrderedParams,
};
use crate::modules::essentials::utils::inspections::{AlkaneCreationRecord, trace_succeeded};
use crate::runtime::state_at::StateAt;
use crate::schemas::SchemaAlkaneId;
use alkanes_support::cellpack::Cellpack;
use alkanes_support::proto::alkanes::alkanes_trace_event::Event;
use anyhow::{Result, anyhow};
use bitcoin::hashes::{Hash as _, sha256};
use ordinals::{Artifact, Runestone};
use protorune_support::protostone::Protostone;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreationKind {
    /// Created with its own binary from the witness envelope.
    OwnBinary,
    /// Top-level clone of this alias target.
    Clone(SchemaAlkaneId),
    /// Created by an extcall inside a contract (target not in the trace).
    Internal,
}

pub static AMBIGUOUS_TEMPLATES: AtomicU64 = AtomicU64::new(0);
pub static UNMATCHED_TEMPLATES: AtomicU64 = AtomicU64::new(0);

/// Binary holders (alkanes whose template is themselves) by code length, with a
/// lazily filled code-hash per holder. Seeded once from espo's creation
/// records, then extended as new own-binary alkanes are indexed.
#[derive(Default)]
struct HolderIndex {
    seeded: bool,
    by_len: HashMap<u64, Vec<(u32, SchemaAlkaneId)>>, // (creation_height, id)
    code_hash: HashMap<SchemaAlkaneId, [u8; 32]>,
}

fn holders() -> &'static Mutex<HolderIndex> {
    static H: OnceLock<Mutex<HolderIndex>> = OnceLock::new();
    H.get_or_init(|| Mutex::new(HolderIndex::default()))
}

fn seed_holders(provider: &EssentialsProvider, idx: &mut HolderIndex) -> Result<()> {
    if idx.seeded {
        return Ok(());
    }
    let records = provider
        .get_creation_records_ordered(GetCreationRecordsOrderedParams {
            blockhash: StateAt::Latest,
        })?
        .records;
    for rec in records {
        let Some(ins) = rec.inspection.as_ref() else { continue };
        if ins.factory_alkane == Some(rec.alkane) {
            idx.by_len
                .entry(ins.bytecode_length)
                .or_default()
                .push((rec.creation_height, rec.alkane));
        }
    }
    idx.seeded = true;
    Ok(())
}

/// Record a newly indexed own-binary alkane so later internal clones can match it.
pub fn note_binary_holder(alkane: SchemaAlkaneId, height: u32, code: &[u8]) {
    let mut idx = holders().lock().unwrap_or_else(|e| e.into_inner());
    let list = idx.by_len.entry(code.len() as u64).or_default();
    if !list.iter().any(|(_, id)| *id == alkane) {
        list.push((height, alkane));
    }
    idx.code_hash.insert(alkane, sha256::Hash::hash(code).to_byte_array());
}

/// How `alkane` (created in `block`, recorded at tx `rec.tx_index_in_block`)
/// came to exist, from the creating trace and the tx's protostones.
pub fn creation_kind(block: &EspoBlock, rec: &AlkaneCreationRecord) -> CreationKind {
    let Some(tx) = block.transactions.get(rec.tx_index_in_block as usize) else {
        return CreationKind::Internal;
    };
    let Some(traces) = tx.traces.as_ref() else { return CreationKind::Internal };
    for trace in traces.iter().filter(|t| trace_succeeded(t)) {
        if let Some(top_level) = create_position(trace, &rec.alkane) {
            if !top_level {
                return CreationKind::Internal;
            }
            return match protostone_cellpack(&tx.transaction, trace.outpoint.vout) {
                Some(cp) => kind_from_target(cp.target.block, cp.target.tx, &rec.alkane),
                None => CreationKind::Internal,
            };
        }
    }
    CreationKind::Internal
}

fn kind_from_target(block: u128, tx: u128, created: &SchemaAlkaneId) -> CreationKind {
    match (block, tx) {
        (1, 0) => CreationKind::OwnBinary,
        (3, n) if created.block == 4 && created.tx as u128 == n => CreationKind::OwnBinary,
        (5, n) => u64::try_from(n)
            .map(|n| CreationKind::Clone(SchemaAlkaneId { block: 2, tx: n }))
            .unwrap_or(CreationKind::Internal),
        (6, n) => u64::try_from(n)
            .map(|n| CreationKind::Clone(SchemaAlkaneId { block: 4, tx: n }))
            .unwrap_or(CreationKind::Internal),
        _ => CreationKind::Internal,
    }
}

/// `Some(true)` if `alkane`'s CreateAlkane event comes before any EnterContext
/// in this trace (the top-level message's own special cellpack), `Some(false)` if
/// it is inside a frame, `None` if this trace does not create it.
fn create_position(trace: &EspoTrace, alkane: &SchemaAlkaneId) -> Option<bool> {
    let mut entered = false;
    for ev in trace.protobuf_trace.events.iter() {
        match ev.event.as_ref() {
            Some(Event::EnterContext(_)) => entered = true,
            Some(Event::CreateAlkane(c)) => {
                let id = c.new_alkane.as_ref()?;
                let b = id.block.as_ref().map(|v| (v.lo as u128) | ((v.hi as u128) << 64))?;
                let t = id.tx.as_ref().map(|v| (v.lo as u128) | ((v.hi as u128) << 64))?;
                if b == alkane.block as u128 && t == alkane.tx as u128 {
                    return Some(!entered);
                }
            }
            _ => {}
        }
    }
    None
}

/// The alkanes cellpack of the protostone a trace's shadow vout points at
/// (`vout = outputs + 1 + protostone_index`).
fn protostone_cellpack(tx: &bitcoin::Transaction, shadow_vout: u32) -> Option<Cellpack> {
    let base = tx.output.len() as u32 + 1;
    let index = shadow_vout.checked_sub(base)? as usize;
    let Some(Artifact::Runestone(runestone)) = Runestone::decipher(tx) else { return None };
    let protostones = Protostone::from_runestone(&runestone).ok()?;
    let ps = protostones.get(index)?;
    crate::alkanes::trace::alkanes_cellpack_of(ps)
}

/// The template recorded for `alias` (its creation record's `factory_alkane`),
/// or `alias` itself when it has none — exactly where the local alias walk
/// would stop.
fn template_of(
    provider: &EssentialsProvider,
    in_block: &HashMap<SchemaAlkaneId, SchemaAlkaneId>,
    alias: &SchemaAlkaneId,
) -> Result<SchemaAlkaneId> {
    Ok(recorded_template(provider, in_block, alias)?.unwrap_or(*alias))
}

/// The template espo recorded for `alias` (this block's resolutions first),
/// or `None` when espo has no creation record for it (e.g. it predates the
/// index's genesis, or is precompiled).
fn recorded_template(
    provider: &EssentialsProvider,
    in_block: &HashMap<SchemaAlkaneId, SchemaAlkaneId>,
    alias: &SchemaAlkaneId,
) -> Result<Option<SchemaAlkaneId>> {
    if let Some(t) = in_block.get(alias) {
        return Ok(Some(*t));
    }
    let rec = provider
        .get_creation_record(GetCreationRecordParams {
            blockhash: StateAt::Latest,
            alkane: *alias,
        })?
        .record;
    Ok(rec.and_then(|r| r.inspection.and_then(|i| i.factory_alkane)))
}

/// TiKV source: a template from espo's records, else the cache's alias
/// record, else the id itself (the local walk's answer for a binary holder
/// espo has no record of).
fn template_of_tikv(
    rpc: &MetashrewRpc,
    provider: &EssentialsProvider,
    in_block: &HashMap<SchemaAlkaneId, SchemaAlkaneId>,
    id: &SchemaAlkaneId,
) -> Result<SchemaAlkaneId> {
    if let Some(t) = recorded_template(provider, in_block, id)? {
        return Ok(t);
    }
    Ok(rpc.tikv_alias(id)?.unwrap_or(*id))
}

/// Which binary holder's code `code` is, for an internally created clone.
fn match_template(
    rpc: &MetashrewRpc,
    provider: &EssentialsProvider,
    code: &[u8],
    height: u32,
    created: &SchemaAlkaneId,
) -> Result<SchemaAlkaneId> {
    let target = sha256::Hash::hash(code).to_byte_array();
    let mut idx = holders().lock().unwrap_or_else(|e| e.into_inner());
    seed_holders(provider, &mut idx)?;
    let candidates: Vec<(u32, SchemaAlkaneId)> =
        idx.by_len.get(&(code.len() as u64)).cloned().unwrap_or_default();
    let mut matches: BTreeMap<(u32, SchemaAlkaneId), ()> = BTreeMap::new();
    for (h, id) in candidates {
        let hash = match idx.code_hash.get(&id) {
            Some(hash) => *hash,
            None => {
                let Some(bytes) = rpc.bytecode(&id, Some(height))? else { continue };
                let hash = sha256::Hash::hash(&bytes).to_byte_array();
                idx.code_hash.insert(id, hash);
                hash
            }
        };
        if hash == target {
            matches.insert((h, id), ());
        }
    }
    let mut it = matches.keys();
    match (it.next(), it.next()) {
        (Some((_, only)), None) => Ok(*only),
        (Some((_, first)), Some(_)) => {
            AMBIGUOUS_TEMPLATES.fetch_add(1, Ordering::Relaxed);
            eprintln!(
                "[ESSENTIALS][rpc] ambiguous template for {}:{}: {} byte-identical holders {:?}; taking the earliest ({}:{})",
                created.block,
                created.tx,
                matches.len(),
                matches.keys().map(|(_, id)| format!("{}:{}", id.block, id.tx)).collect::<Vec<_>>(),
                first.block,
                first.tx
            );
            Ok(*first)
        }
        (None, _) => {
            // No holder has this code: the clone's template is not a binary
            // espo has recorded (e.g. a precompiled id). Report the alkane
            // itself, as the local reader does for a payload it cannot alias.
            UNMATCHED_TEMPLATES.fetch_add(1, Ordering::Relaxed);
            eprintln!(
                "[ESSENTIALS][rpc] no binary holder matches the code of {}:{}; template = itself",
                created.block, created.tx
            );
            Ok(*created)
        }
    }
}

/// `(wasm, template)` for a creation record, over RPC, at the creation height.
pub fn creation_wasm_rpc(
    rpc: &MetashrewRpc,
    provider: &EssentialsProvider,
    block: &EspoBlock,
    rec: &AlkaneCreationRecord,
    factory_hint: Option<&SchemaAlkaneId>,
    in_block: &HashMap<SchemaAlkaneId, SchemaAlkaneId>,
) -> Result<Option<(Vec<u8>, SchemaAlkaneId)>> {
    let height = block.height;
    if let Some(hint) = factory_hint {
        // local: prefer the hint's code; on None/Err fall back to the alkane
        if let Ok(Some(bytes)) = rpc.bytecode(hint, Some(height)) {
            let template = if rpc.traces_from_tikv() {
                template_of_tikv(rpc, provider, in_block, hint)?
            } else {
                template_of(provider, in_block, hint)?
            };
            return Ok(Some((bytes, template)));
        }
    }
    let Some(bytes) = rpc.bytecode(&rec.alkane, Some(height))? else { return Ok(None) };
    // The genesis alkanes are seeded into the genesis block without a create
    // event (essentials main.rs); they hold their own binaries.
    let genesis_seeded = block.height
        == crate::modules::essentials::consts::essentials_genesis_block(crate::config::get_network())
        && crate::modules::essentials::consts::ESSENTIALS_GENESIS_INSPECTIONS
            .iter()
            .any(|(b, t, _)| *b == rec.alkane.block && *t == rec.alkane.tx);
    let kind = if genesis_seeded { CreationKind::OwnBinary } else { creation_kind(block, rec) };
    let template = if rpc.traces_from_tikv() {
        // TiKV source: the alias records written beside the block entries are
        // authoritative. A clone made inside a contract has no other exact
        // source, so without its record the block fails (and is retried) —
        // nothing is guessed. Where a record exists for a creation we can also
        // derive, the two must agree.
        let recorded = rpc.tikv_alias(&rec.alkane)?;
        // Only a derivation espo can back with its own records is compared
        // against the alias record; an alias target espo never recorded (it
        // predates the index's genesis) is left to the record.
        let derived = match kind {
            CreationKind::OwnBinary => Some(rec.alkane),
            CreationKind::Clone(alias) => recorded_template(provider, in_block, &alias)?,
            CreationKind::Internal => None,
        };
        match (recorded, derived) {
            (Some(r), Some(d)) if r != d => anyhow::bail!(
                "template mismatch for {}:{}: alias record says {}:{}, cellpack says {}:{}",
                rec.alkane.block, rec.alkane.tx, r.block, r.tx, d.block, d.tx
            ),
            (Some(r), _) => r,
            (None, Some(d)) => d,
            // Test harness only: ESPO_RPC_ALIAS_FALLBACK=bytecode lets an
            // equivalence run proceed before the cache carries alias records,
            // counting every guess. Never set in production.
            (None, None) if std::env::var("ESPO_RPC_ALIAS_FALLBACK").as_deref() == Ok("bytecode") => {
                eprintln!(
                    "[ESSENTIALS][rpc] ESPO_RPC_ALIAS_FALLBACK: guessing the template of {}:{} by bytecode",
                    rec.alkane.block, rec.alkane.tx
                );
                match_template(rpc, provider, &bytes, height, &rec.alkane)?
            }
            (None, None) => anyhow::bail!(
                "no alias record for {}:{} (created inside a contract at height {height}); the trace cache needs alias decoration for this block",
                rec.alkane.block, rec.alkane.tx
            ),
        }
    } else {
        match kind {
            CreationKind::OwnBinary => rec.alkane,
            CreationKind::Clone(alias) => template_of(provider, in_block, &alias)?,
            CreationKind::Internal => match_template(rpc, provider, &bytes, height, &rec.alkane)?,
        }
    };
    if template == rec.alkane {
        note_binary_holder(rec.alkane, height, &bytes);
    }
    Ok(Some((bytes, template)))
}

pub fn rpc_source() -> Result<std::sync::Arc<MetashrewRpc>> {
    crate::alkanes::metashrew_rpc::get().ok_or_else(|| anyhow!("metashrew rpc source not initialized"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_cellpack_targets_map_to_creation_kinds() {
        let created = SchemaAlkaneId { block: 2, tx: 100 };
        assert_eq!(kind_from_target(1, 0, &created), CreationKind::OwnBinary);
        assert_eq!(
            kind_from_target(5, 77, &created),
            CreationKind::Clone(SchemaAlkaneId { block: 2, tx: 77 })
        );
        assert_eq!(
            kind_from_target(6, 520, &created),
            CreationKind::Clone(SchemaAlkaneId { block: 4, tx: 520 })
        );
        let reserved = SchemaAlkaneId { block: 4, tx: 9 };
        assert_eq!(kind_from_target(3, 9, &reserved), CreationKind::OwnBinary);
        assert_eq!(kind_from_target(3, 8, &reserved), CreationKind::Internal);
        assert_eq!(kind_from_target(2, 5, &created), CreationKind::Internal);
    }
}
