use crate::alkanes::trace::{
    EspoHostFunctionValues, EspoSandshrewLikeTrace, EspoSandshrewLikeTraceEvent,
    EspoSandshrewLikeTraceStatus,
};
use crate::config::{get_electrum_like, get_metashrew};
use alkanes_support::proto::alkanes as alkanes_pb;
use anyhow::Result;
use bitcoin::Transaction;
use bitcoin::block::Header;
use bitcoin::consensus::encode::deserialize;
use std::collections::HashSet;

fn get_electrum_tip() -> Result<u32> {
    let client = get_electrum_like();
    client.tip_height()
}

pub fn get_safe_tip() -> Result<u32> {
    let alkanes_tip = get_metashrew().get_canonical_tip_height()?;
    let electrum_tip = match get_electrum_tip() {
        Ok(tip) => Some(tip),
        Err(e) => {
            eprintln!("[tip] electrum/esplora tip fetch failed: {e:?}; using metashrew tip only");
            None
        }
    };

    Ok(electrum_tip.map(|t| std::cmp::min(alkanes_tip, t)).unwrap_or(alkanes_tip))
}

// ---------------------------------------------------------------------------
// Trace cleaning: metashrew host-function orphan returns
//
// Metashrew's special/precompiled extcalls (load block header, coinbase tx,
// diesel mint count, total fees) clock a bare `ExitContext` on the trace with
// NO matching `EnterContext`. Any trace of a message that touched a host
// function therefore contains more returns than invokes, and a naive
// push/pop walk mispairs every event after the first orphan: real frames get
// popped by host-function returns and the root's final return (which carries
// the storage writes and response transfers) is left with an empty stack.
//
// The cleaner below identifies the orphan returns — success returns with no
// alkanes transfers and no storage whose payload matches one of the block's
// host-function values (exact match, with a fuzzy header/coinbase fallback) —
// and removes exactly enough of them to rebalance the call stack. It is
// shared by every consumer that walks trace events as a call stack (balances,
// storage/K-V extraction, subfrost, ammdata).
// ---------------------------------------------------------------------------

/// Minimal per-event view used by the orphan-return cleaner so the same core
/// algorithm can run over both sandshrew-like and raw protobuf trace events.
pub enum CleanEventView {
    Invoke,
    /// `candidate_data` is `Some(raw response payload)` only when the return
    /// could plausibly be a host-function response: success status, no
    /// alkanes transfers and no storage writes.
    Return {
        candidate_data: Option<Vec<u8>>,
    },
    Other,
}

fn decode_hex_data(data: &str) -> Option<Vec<u8>> {
    let trimmed = data.strip_prefix("0x").unwrap_or(data);
    if trimmed.is_empty() {
        return Some(Vec::new());
    }
    hex::decode(trimmed).ok()
}

pub fn sandshrew_event_views(events: &[EspoSandshrewLikeTraceEvent]) -> Vec<CleanEventView> {
    events
        .iter()
        .map(|ev| match ev {
            EspoSandshrewLikeTraceEvent::Invoke(_) => CleanEventView::Invoke,
            EspoSandshrewLikeTraceEvent::Return(ret) => {
                let candidate_data = if ret.status == EspoSandshrewLikeTraceStatus::Success
                    && ret.response.alkanes.is_empty()
                    && ret.response.storage.is_empty()
                {
                    decode_hex_data(&ret.response.data)
                } else {
                    None
                };
                CleanEventView::Return { candidate_data }
            }
            EspoSandshrewLikeTraceEvent::Create(_) => CleanEventView::Other,
        })
        .collect()
}

pub fn protobuf_event_views(trace: &alkanes_pb::AlkanesTrace) -> Vec<CleanEventView> {
    use alkanes_pb::alkanes_trace_event::Event;
    trace
        .events
        .iter()
        .map(|ev| match &ev.event {
            Some(Event::EnterContext(_)) => CleanEventView::Invoke,
            Some(Event::ExitContext(exit)) => {
                let success = exit.status() != alkanes_pb::AlkanesTraceStatusFlag::Failure;
                let candidate_data = if success {
                    match exit.response.as_ref() {
                        Some(resp) if resp.alkanes.is_empty() && resp.storage.is_empty() => {
                            Some(resp.data.clone())
                        }
                        None => Some(Vec::new()),
                        _ => None,
                    }
                } else {
                    None
                };
                CleanEventView::Return { candidate_data }
            }
            Some(Event::CreateAlkane(_)) | None => CleanEventView::Other,
        })
        .collect()
}

/// Compute the set of event indices that are host-function orphan returns and
/// must be removed to rebalance the trace's call stack.
///
/// Returns `Some(empty set)` when the trace is already balanced,
/// `Some(indices)` when the surplus returns could all be attributed to host
/// functions, and `None` when the trace cannot be rebalanced (truncated trace
/// or unattributable orphan returns) — callers decide their own fallback.
pub fn orphan_return_removals(
    views: &[CleanEventView],
    host_function_values: &EspoHostFunctionValues,
) -> Option<HashSet<usize>> {
    let mut invokes = 0usize;
    let mut returns = 0usize;
    for view in views {
        match view {
            CleanEventView::Invoke => invokes += 1,
            CleanEventView::Return { .. } => returns += 1,
            CleanEventView::Other => {}
        }
    }

    if invokes == returns {
        return Some(HashSet::new());
    }
    if returns < invokes {
        return None;
    }

    let (header, coinbase, diesel, fee) = host_function_values;
    let host_values: [&[u8]; 4] = [header, coinbase, diesel, fee];
    let mismatch = returns - invokes;

    // Host functions never return empty payloads; skipping empty host values
    // keeps an all-default `EspoHostFunctionValues` (mempool/tests) from
    // classifying empty-data returns as host-function responses.
    let host_match =
        |data: &[u8]| -> bool { host_values.iter().any(|hv| !hv.is_empty() && data == *hv) };
    let fuzzy_host_match = |data: &[u8]| -> bool {
        if data.len() == 80 && deserialize::<Header>(data).is_ok() {
            return true;
        }
        if let Ok(tx) = deserialize::<Transaction>(data) {
            if tx.is_coinbase() {
                return true;
            }
        }
        false
    };

    let attempt = |allow_fuzzy: bool| -> Option<HashSet<usize>> {
        let mut remove_indices: HashSet<usize> = HashSet::new();
        let mut candidate_stack: Vec<usize> = Vec::new();
        let mut total_candidates = 0usize;
        let mut depth: isize = 0;

        for (idx, view) in views.iter().enumerate() {
            match view {
                CleanEventView::Invoke => depth += 1,
                CleanEventView::Return { candidate_data } => {
                    let is_candidate = candidate_data
                        .as_ref()
                        .map(|data| host_match(data) || (allow_fuzzy && fuzzy_host_match(data)))
                        .unwrap_or(false);
                    if is_candidate {
                        total_candidates += 1;
                        candidate_stack.push(idx);
                    }
                    depth -= 1;
                    if depth < 0 {
                        let remove_idx = candidate_stack.pop()?;
                        remove_indices.insert(remove_idx);
                        depth += 1;
                    }
                }
                CleanEventView::Other => {}
            }
        }

        if total_candidates < mismatch || remove_indices.len() != mismatch {
            return None;
        }

        // Validate that the surviving events form a balanced call stack.
        let mut cleaned_invokes = 0usize;
        let mut cleaned_returns = 0usize;
        let mut cleaned_depth: isize = 0;
        for (idx, view) in views.iter().enumerate() {
            if remove_indices.contains(&idx) {
                continue;
            }
            match view {
                CleanEventView::Invoke => {
                    cleaned_invokes += 1;
                    cleaned_depth += 1;
                }
                CleanEventView::Return { .. } => {
                    cleaned_returns += 1;
                    cleaned_depth -= 1;
                    if cleaned_depth < 0 {
                        return None;
                    }
                }
                CleanEventView::Other => {}
            }
        }
        if cleaned_invokes != cleaned_returns || cleaned_depth != 0 {
            return None;
        }

        Some(remove_indices)
    };

    attempt(false).or_else(|| attempt(true))
}

/// Remove host-function orphan returns from a sandshrew-like trace so its
/// events can be walked as a balanced call stack. Returns `None` when the
/// trace cannot be rebalanced; such traces should not be stack-walked.
pub fn clean_espo_sandshrew_like_trace(
    trace: &EspoSandshrewLikeTrace,
    host_function_values: &EspoHostFunctionValues,
) -> Option<EspoSandshrewLikeTrace> {
    let views = sandshrew_event_views(&trace.events);
    let remove_indices = orphan_return_removals(&views, host_function_values)?;
    if remove_indices.is_empty() {
        return Some(trace.clone());
    }
    let events = trace
        .events
        .iter()
        .enumerate()
        .filter(|(idx, _)| !remove_indices.contains(idx))
        .map(|(_, ev)| ev.clone())
        .collect();
    Some(EspoSandshrewLikeTrace { outpoint: trace.outpoint.clone(), events })
}

#[cfg(test)]
mod trace_clean_tests {
    use super::*;
    use crate::alkanes::trace::extract_alkane_storage;
    use crate::schemas::SchemaAlkaneId;
    use bitcoin::{
        Amount, OutPoint, ScriptBuf, Sequence, TxIn, TxOut, Witness, absolute::LockTime,
        transaction::Version,
    };

    fn sample_tx() -> Transaction {
        Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![TxOut { value: Amount::ZERO, script_pubkey: ScriptBuf::new() }],
        }
    }

    fn pb_id(block: u64, tx: u64) -> alkanes_pb::AlkaneId {
        alkanes_pb::AlkaneId {
            block: Some(alkanes_pb::Uint128 { lo: block, hi: 0 }),
            tx: Some(alkanes_pb::Uint128 { lo: tx, hi: 0 }),
        }
    }

    fn pb_enter(block: u64, tx: u64) -> alkanes_pb::AlkanesTraceEvent {
        alkanes_pb::AlkanesTraceEvent {
            event: Some(alkanes_pb::alkanes_trace_event::Event::EnterContext(
                alkanes_pb::AlkanesEnterContext {
                    call_type: 0,
                    context: Some(alkanes_pb::TraceContext {
                        inner: Some(alkanes_pb::Context {
                            myself: Some(pb_id(block, tx)),
                            caller: Some(pb_id(0, 0)),
                            inputs: vec![],
                            vout: 4,
                            incoming_alkanes: vec![],
                        }),
                        fuel: 0,
                    }),
                },
            )),
        }
    }

    fn pb_exit(data: Vec<u8>, storage: Vec<(&[u8], &[u8])>) -> alkanes_pb::AlkanesTraceEvent {
        alkanes_pb::AlkanesTraceEvent {
            event: Some(alkanes_pb::alkanes_trace_event::Event::ExitContext(
                alkanes_pb::AlkanesExitContext {
                    status: alkanes_pb::AlkanesTraceStatusFlag::Success as i32,
                    response: Some(alkanes_pb::ExtendedCallResponse {
                        alkanes: vec![],
                        storage: storage
                            .into_iter()
                            .map(|(k, v)| alkanes_pb::KeyValuePair {
                                key: k.to_vec(),
                                value: v.to_vec(),
                            })
                            .collect(),
                        data,
                    }),
                },
            )),
        }
    }

    /// Regression for the stale /salsa_global_state key of 2:68478: metashrew
    /// host-function calls emit bare ExitContext events, and the naive stack
    /// walk paired the first orphan with the root frame then dropped the real
    /// root return carrying the storage writes.
    #[test]
    fn extract_alkane_storage_survives_host_function_orphan_returns() {
        let header = vec![0xABu8; 80];
        let host_values: EspoHostFunctionValues =
            (header.clone(), Vec::new(), Vec::new(), Vec::new());

        let trace = alkanes_pb::AlkanesTrace {
            events: vec![
                pb_enter(2, 68478),
                pb_exit(header.clone(), vec![]),
                pb_exit(header.clone(), vec![]),
                pb_exit(Vec::new(), vec![(b"/salsa_global_state", b"\x01\x02")]),
            ],
        };

        let out = extract_alkane_storage(&trace, &sample_tx(), &host_values).expect("extract");
        let owner = SchemaAlkaneId { block: 2, tx: 68478 };
        let kvs = out.get(&owner).expect("storage attributed to root contract");
        let entry = kvs.get(b"/salsa_global_state".as_slice()).expect("key recorded");
        assert_eq!(entry.1, b"\x01\x02".to_vec());
    }

    /// Orphan returns whose payload matches no host-function value (exact or
    /// fuzzy) leave the trace unbalanced; extraction falls back to the raw
    /// walk instead of erroring.
    #[test]
    fn extract_alkane_storage_falls_back_when_trace_cannot_be_rebalanced() {
        let host_values = EspoHostFunctionValues::default();
        let trace = alkanes_pb::AlkanesTrace {
            events: vec![
                pb_enter(2, 68478),
                pb_exit(b"JUNK".to_vec(), vec![]),
                pb_exit(Vec::new(), vec![(b"/salsa_global_state", b"\x01\x02")]),
            ],
        };

        let out = extract_alkane_storage(&trace, &sample_tx(), &host_values).expect("extract");
        let owner = SchemaAlkaneId { block: 2, tx: 68478 };
        let recorded =
            out.get(&owner).map(|kvs| kvs.contains_key(b"/salsa_global_state".as_slice()));
        assert_ne!(recorded, Some(true), "unbalanced fallback keeps the old naive pairing");
    }

    /// The relocated sandshrew-like cleaner still strips fuzzy header orphans
    /// (80-byte payloads) when exact host values are unavailable.
    #[test]
    fn clean_sandshrew_trace_removes_fuzzy_header_orphans() {
        let header_hex = format!("0x{}", "ab".repeat(80));
        let trace_json = format!(
            r#"{{
  "outpoint": "{}:4",
  "events": [
    {{"event":"invoke","data":{{"type":"call","context":{{"myself":{{"block":"0x2","tx":"0x10b7e"}},"caller":{{"block":"0x0","tx":"0x0"}},"inputs":[],"incomingAlkanes":[],"vout":4}},"fuel":1}}}},
    {{"event":"return","data":{{"status":"success","response":{{"alkanes":[],"data":"{header_hex}","storage":[]}}}}}},
    {{"event":"return","data":{{"status":"success","response":{{"alkanes":[],"data":"0x","storage":[{{"key":"/salsa_global_state","value":"0x0102"}}]}}}}}}
  ]
}}"#,
            "00".repeat(32),
        );
        let trace: EspoSandshrewLikeTrace =
            serde_json::from_str(&trace_json).expect("parse sandshrew trace");

        let cleaned = clean_espo_sandshrew_like_trace(&trace, &EspoHostFunctionValues::default())
            .expect("trace should rebalance via fuzzy header match");
        assert_eq!(cleaned.events.len(), 2);
        let EspoSandshrewLikeTraceEvent::Return(last) = cleaned.events.last().unwrap() else {
            panic!("expected return event last");
        };
        assert_eq!(last.response.storage.len(), 1);
        assert_eq!(last.response.storage[0].key, "/salsa_global_state");
    }
}
