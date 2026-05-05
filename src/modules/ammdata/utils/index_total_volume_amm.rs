use crate::modules::ammdata::consts::CanonicalQuoteUnit;
use crate::modules::ammdata::storage::{AmmDataProvider, TotalVolumeAmmUnit, encode_u128_value};
use crate::modules::ammdata::utils::index_state::IndexState;
use crate::schemas::SchemaAlkaneId;
use anyhow::Result;
use std::collections::HashMap;

pub fn prepare_total_volume_amm(
    height: u32,
    provider: &AmmDataProvider,
    canonical_quote_units: &HashMap<SchemaAlkaneId, CanonicalQuoteUnit>,
    state: &mut IndexState,
) -> Result<()> {
    if state.in_block_trade_volumes.is_empty() {
        return Ok(());
    }

    let mut block_sats = 0u128;
    let mut block_usd = 0u128;

    for (pool, (base_abs, quote_abs)) in state.in_block_trade_volumes.iter() {
        let Some(defs) = state.pools_map.get(pool) else { continue };
        let canonical = canonical_quote_units
            .get(&defs.quote_alkane_id)
            .map(|unit| (*quote_abs, *unit))
            .or_else(|| {
                canonical_quote_units.get(&defs.base_alkane_id).map(|unit| (*base_abs, *unit))
            });
        let Some((amount, unit)) = canonical else { continue };
        if amount == 0 {
            continue;
        }
        if let Some(value) = crate::modules::ammdata::canonical_quote_amount_tvl_sats(
            amount,
            unit,
            state.btc_usd_price,
        ) {
            block_sats = block_sats.saturating_add(value);
        }
        if let Some(value) = crate::modules::ammdata::canonical_quote_amount_tvl_usd(
            amount,
            unit,
            state.btc_usd_price,
        ) {
            block_usd = block_usd.saturating_add(value);
        }
    }

    if block_sats == 0 && block_usd == 0 {
        return Ok(());
    }

    let table = provider.table();
    if block_sats > 0 {
        let previous = provider
            .get_total_volume_amm_at_or_before_height(
                TotalVolumeAmmUnit::Sats,
                height.saturating_sub(1),
            )?
            .map(|(_, value)| value)
            .unwrap_or(0);
        state.total_volume_amm_writes.push((
            table.total_volume_amm_key(TotalVolumeAmmUnit::Sats, u64::from(height)),
            encode_u128_value(previous.saturating_add(block_sats))?,
        ));
    }
    if block_usd > 0 {
        let previous = provider
            .get_total_volume_amm_at_or_before_height(
                TotalVolumeAmmUnit::Usd,
                height.saturating_sub(1),
            )?
            .map(|(_, value)| value)
            .unwrap_or(0);
        state.total_volume_amm_writes.push((
            table.total_volume_amm_key(TotalVolumeAmmUnit::Usd, u64::from(height)),
            encode_u128_value(previous.saturating_add(block_usd))?,
        ));
    }

    Ok(())
}
