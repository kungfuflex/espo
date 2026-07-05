mod amm;
mod fire;
mod frbtc;

use crate::modules::essentials::utils::balances::{
    ContractProjection, ContractProjectionContext, MempoolContractProjector, ProjectionSheet,
};
use alkanes_support::cellpack::Cellpack;
use protorune_support::protostone::Protostone;
use protorune_support::utils::decode_varint_list;
use std::io::Cursor;

pub(crate) trait MempoolContractRule {
    fn project(&mut self, ctx: &ContractProjectionContext<'_>) -> Option<ContractProjection>;
}

pub(crate) struct MempoolProjectionRegistry {
    rules: Vec<Box<dyn MempoolContractRule>>,
}

impl MempoolProjectionRegistry {
    pub(crate) fn from_latest_indices() -> Self {
        Self {
            rules: vec![
                Box::new(frbtc::FrBtcProjectionRule),
                Box::new(amm::AmmProjectionRule::new()),
                Box::new(fire::FireProjectionRule::new()),
            ],
        }
    }
}

impl MempoolContractProjector for MempoolProjectionRegistry {
    fn project(&mut self, ctx: ContractProjectionContext<'_>) -> Option<ContractProjection> {
        for rule in &mut self.rules {
            if let Some(projection) = rule.project(&ctx) {
                return Some(projection);
            }
        }
        None
    }
}

pub(crate) fn cellpack_from_protostone(protostone: &Protostone) -> Option<Cellpack> {
    if protostone.protocol_tag != 1 || protostone.message.is_empty() {
        return None;
    }
    let calldata: Vec<u8> = protostone.message.iter().flat_map(|v| v.to_be_bytes()).collect();
    let Ok(values) = decode_varint_list(&mut Cursor::new(calldata)) else {
        return None;
    };
    TryInto::<Cellpack>::try_into(values).ok()
}

pub(crate) fn add_to_sheet(
    sheet: &mut ProjectionSheet,
    id: crate::schemas::SchemaAlkaneId,
    amount: u128,
) {
    if amount == 0 {
        return;
    }
    *sheet.entry(id).or_default() =
        sheet.get(&id).copied().unwrap_or_default().saturating_add(amount);
}

pub(crate) fn remove_from_sheet(
    sheet: &mut ProjectionSheet,
    id: crate::schemas::SchemaAlkaneId,
    amount: u128,
) -> u128 {
    let Some(entry) = sheet.get_mut(&id) else {
        return 0;
    };
    let taken = (*entry).min(amount);
    *entry = entry.saturating_sub(taken);
    if *entry == 0 {
        sheet.remove(&id);
    }
    taken
}

pub(crate) fn alkane_id_from_parts(
    block: u128,
    tx: u128,
) -> Option<crate::schemas::SchemaAlkaneId> {
    Some(crate::schemas::SchemaAlkaneId {
        block: u32::try_from(block).ok()?,
        tx: u64::try_from(tx).ok()?,
    })
}
