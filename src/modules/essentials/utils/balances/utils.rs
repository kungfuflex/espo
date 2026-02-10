use super::defs::EspoTraceType;
use crate::alkanes::trace::{
    EspoSandshrewLikeTrace, EspoSandshrewLikeTraceEvent, EspoSandshrewLikeTraceReturnData,
    EspoSandshrewLikeTraceShortId, EspoSandshrewLikeTraceStatus, EspoSandshrewLikeTraceTransfer,
};
use crate::schemas::SchemaAlkaneId;
use anyhow::{Result, anyhow};
use bitcoin::{ScriptBuf, Transaction};
use ordinals::{Artifact, Runestone};
use protorune_support::protostone::Protostone;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

#[inline]
pub(super) fn tx_has_op_return(tx: &Transaction) -> bool {
    tx.output.iter().any(|o| is_op_return(&o.script_pubkey))
}

pub(super) fn parse_protostones(tx: &Transaction) -> Result<Vec<Protostone>> {
    let runestone = match Runestone::decipher(tx) {
        Some(Artifact::Runestone(r)) => r,
        _ => return Ok(vec![]),
    };
    let protos = Protostone::from_runestone(&runestone)
        .map_err(|e| anyhow!("failed to parse protostones: {e}"))?;
    Ok(protos)
}

#[derive(Default, Clone)]
pub(super) struct Unallocated {
    map: HashMap<SchemaAlkaneId, u128>,
}
impl Unallocated {
    pub(super) fn add(&mut self, id: SchemaAlkaneId, amt: u128) {
        *self.map.entry(id).or_default() =
            self.map.get(&id).copied().unwrap_or(0).saturating_add(amt);
    }
    #[allow(dead_code)]
    pub(super) fn get(&self, id: &SchemaAlkaneId) -> u128 {
        self.map.get(id).copied().unwrap_or(0)
    }

    #[allow(dead_code)]
    pub(super) fn take(&mut self, id: &SchemaAlkaneId, amt: u128) -> u128 {
        let cur = self.get(id);
        let take = cur.min(amt);
        if take == cur {
            self.map.remove(id);
        } else if let Some(e) = self.map.get_mut(id) {
            *e = cur - take;
        }
        take
    }
    pub(super) fn drain_all(&mut self) -> BTreeMap<SchemaAlkaneId, u128> {
        let mut merged: BTreeMap<SchemaAlkaneId, u128> = BTreeMap::new();
        for (rid, amt) in self.map.drain() {
            if amt == 0 {
                continue;
            }
            *merged.entry(rid).or_default() =
                merged.get(&rid).copied().unwrap_or(0).saturating_add(amt);
        }
        merged
    }
    #[allow(dead_code)]
    pub(super) fn is_empty(&self) -> bool {
        self.map.values().all(|&v| v == 0)
    }
}

impl fmt::Display for Unallocated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut items: Vec<_> = self.map.iter().filter(|&(_, &amt)| amt != 0).collect();

        items.sort_by(|(a, _), (b, _)| a.cmp(b));

        write!(f, "{{")?;
        let mut first = true;
        for (id, amt) in items {
            if !first {
                write!(f, ", ")?;
            }
            first = false;
            write!(f, "{}={}", id, amt)?;
        }
        write!(f, "}}")
    }
}

pub(super) fn is_op_return(spk: &ScriptBuf) -> bool {
    let b = spk.as_bytes();
    !b.is_empty() && b[0] == bitcoin::opcodes::all::OP_RETURN.to_u8()
}

pub(super) fn u128_to_u32(v: u128) -> Result<u32> {
    u32::try_from(v).map_err(|_| anyhow!("downcast failed: {v} does not fit into u32"))
}
fn u128_to_u64(v: u128) -> Result<u64> {
    u64::try_from(v).map_err(|_| anyhow!("downcast failed: {v} does not fit into u64"))
}
pub(super) fn schema_id_from_parts(block_u128: u128, tx_u128: u128) -> Result<SchemaAlkaneId> {
    Ok(SchemaAlkaneId { block: u128_to_u32(block_u128)?, tx: u128_to_u64(tx_u128)? })
}

fn parse_hex_u32(s: &str) -> Option<u32> {
    let x = s.strip_prefix("0x").unwrap_or(s);
    u32::from_str_radix(x, 16).ok()
}
fn parse_hex_u64(s: &str) -> Option<u64> {
    let x = s.strip_prefix("0x").unwrap_or(s);
    u64::from_str_radix(x, 16).ok()
}
fn parse_hex_u128(s: &str) -> Option<u128> {
    let x = s.strip_prefix("0x").unwrap_or(s);
    u128::from_str_radix(x, 16).ok()
}

pub(super) fn compute_nets(
    trace: &EspoSandshrewLikeTrace,
) -> (Option<BTreeMap<SchemaAlkaneId, u128>>, Option<BTreeMap<SchemaAlkaneId, u128>>, EspoTraceType)
{
    let mut netin: Option<BTreeMap<SchemaAlkaneId, u128>> = None;
    for ev in &trace.events {
        if let EspoSandshrewLikeTraceEvent::Invoke(inv) = ev {
            let mut m = BTreeMap::new();
            for t in &inv.context.incoming_alkanes {
                if let (Some(blk), Some(tx), Some(val)) =
                    (parse_hex_u32(&t.id.block), parse_hex_u64(&t.id.tx), parse_hex_u128(&t.value))
                {
                    let k = SchemaAlkaneId { block: blk, tx };
                    *m.entry(k).or_default() =
                        m.get(&k).copied().unwrap_or(0u128).saturating_add(val);
                }
            }
            netin = Some(m);
            break;
        }
    }

    let mut last_ret: Option<&EspoSandshrewLikeTraceReturnData> = None;
    for ev in &trace.events {
        if let EspoSandshrewLikeTraceEvent::Return(r) = ev {
            last_ret = Some(r);
        }
    }

    let (netout, status): (Option<BTreeMap<SchemaAlkaneId, u128>>, EspoTraceType) = match last_ret {
        None => (None, EspoTraceType::NOTRACE),
        Some(r) => {
            let mut m = BTreeMap::new();
            for t in &r.response.alkanes {
                if let (Some(blk), Some(tx), Some(val)) =
                    (parse_hex_u32(&t.id.block), parse_hex_u64(&t.id.tx), parse_hex_u128(&t.value))
                {
                    let k = SchemaAlkaneId { block: blk, tx };
                    *m.entry(k).or_default() =
                        m.get(&k).copied().unwrap_or(0u128).saturating_add(val);
                }
            }
            let cls = match r.status {
                EspoSandshrewLikeTraceStatus::Failure => EspoTraceType::REVERT,
                EspoSandshrewLikeTraceStatus::Success => EspoTraceType::SUCCESS,
            };
            (Some(m), cls)
        }
    };

    (netin, netout, status)
}

pub(super) fn parse_short_id(id: &EspoSandshrewLikeTraceShortId) -> Option<SchemaAlkaneId> {
    let block = parse_hex_u32(&id.block)?;
    let tx = parse_hex_u64(&id.tx)?;
    Some(SchemaAlkaneId { block, tx })
}

pub(super) fn transfers_to_sheet(
    transfers: &[EspoSandshrewLikeTraceTransfer],
) -> BTreeMap<SchemaAlkaneId, u128> {
    let mut m = BTreeMap::new();
    for t in transfers {
        if let (Some(block), Some(tx), Some(val)) =
            (parse_hex_u32(&t.id.block), parse_hex_u64(&t.id.tx), parse_hex_u128(&t.value))
        {
            if val == 0 {
                continue;
            }
            let k = SchemaAlkaneId { block, tx };
            *m.entry(k).or_default() = m.get(&k).copied().unwrap_or(0u128).saturating_add(val);
        }
    }
    m
}
