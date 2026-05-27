use crate::modules::ammdata::consts::{AMOUNT_SCALE, CanonicalQuoteUnit};
use crate::modules::ammdata::storage::{
    AmmDataProvider, GetTokenMetricsParams, TotalVolumeAmmUnit, encode_u128_value,
};
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

    let trades = state
        .in_block_trade_volumes
        .iter()
        .map(|(pool, volumes)| (*pool, *volumes))
        .collect::<Vec<_>>();

    for (pool, (base_abs, quote_abs)) in trades {
        let Some(defs) = state.pools_map.get(&pool).copied() else { continue };
        let (token, amount) = if canonical_quote_units.contains_key(&defs.quote_alkane_id) {
            (defs.quote_alkane_id, quote_abs)
        } else {
            (defs.base_alkane_id, base_abs)
        };
        let (usd, sats) =
            token_amount_values(provider, canonical_quote_units, state, token, amount);
        block_usd = block_usd.saturating_add(usd);
        block_sats = block_sats.saturating_add(sats);
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

fn token_amount_values(
    provider: &AmmDataProvider,
    canonical_quote_units: &HashMap<SchemaAlkaneId, CanonicalQuoteUnit>,
    state: &mut IndexState,
    token: SchemaAlkaneId,
    amount: u128,
) -> (u128, u128) {
    if amount == 0 {
        return (0, 0);
    }

    if let Some(unit) = canonical_quote_units.get(&token).copied() {
        let usd = crate::modules::ammdata::canonical_quote_amount_tvl_usd(
            amount,
            unit,
            state.btc_usd_price,
        )
        .unwrap_or(0);
        let sats = crate::modules::ammdata::canonical_quote_amount_tvl_sats(
            amount,
            unit,
            state.btc_usd_price,
        )
        .unwrap_or(0);
        return (usd, sats);
    }

    let price_usd = token_price_usd(provider, state, token);
    if price_usd == 0 {
        return (0, 0);
    }

    let usd = amount.saturating_mul(price_usd).saturating_div(AMOUNT_SCALE);
    let sats = match state.btc_usd_price {
        Some(btc_price) if btc_price > 0 => {
            usd.saturating_mul(AMOUNT_SCALE).saturating_div(btc_price)
        }
        _ => 0,
    };
    (usd, sats)
}

fn token_price_usd(
    provider: &AmmDataProvider,
    state: &mut IndexState,
    token: SchemaAlkaneId,
) -> u128 {
    if let Some(metrics) = state.token_metrics_cache.get(&token) {
        return metrics.price_usd;
    }

    let metrics = provider
        .get_token_metrics(GetTokenMetricsParams {
            blockhash: crate::runtime::state_at::StateAt::Latest,
            token,
        })
        .map(|res| res.metrics)
        .unwrap_or_default();
    let price = metrics.price_usd;
    state.token_metrics_cache.insert(token, metrics);
    price
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::ammdata::consts::{FRBTC_ALKANE_ID, PRICE_SCALE};
    use crate::modules::ammdata::schemas::{SchemaMarketDefs, SchemaTokenMetricsV1};
    use crate::modules::ammdata::storage::SetBatchParams;
    use crate::modules::essentials::storage::EssentialsProvider;
    use crate::runtime::mdb::Mdb;
    use crate::runtime::state_at::StateAt;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_provider() -> (TempDir, AmmDataProvider) {
        let dir = TempDir::new().expect("temp dir");
        let amm_mdb = Arc::new(Mdb::open(dir.path(), b"ammdata:").expect("open ammdata mdb"));
        let essentials =
            Arc::new(EssentialsProvider::new(Arc::new(amm_mdb.clone_with_prefix(b"essentials:"))));
        (dir, AmmDataProvider::new(amm_mdb, essentials))
    }

    #[test]
    fn total_volume_amm_matches_ohlcv_canonical_volume_leg() {
        let (_dir, provider) = test_provider();
        let diesel = SchemaAlkaneId { block: 2, tx: 0 };
        let pool = SchemaAlkaneId { block: 100, tx: 1 };
        let btc_price = 50_000u128.saturating_mul(PRICE_SCALE);
        let diesel_price = 5_000u128.saturating_mul(PRICE_SCALE);

        let mut pools = HashMap::new();
        pools.insert(
            pool,
            SchemaMarketDefs {
                base_alkane_id: diesel,
                quote_alkane_id: FRBTC_ALKANE_ID,
                pool_alkane_id: pool,
            },
        );
        let mut state = IndexState::new(HashMap::new(), pools);
        state.btc_usd_price = Some(btc_price);
        state.in_block_trade_volumes.insert(
            pool,
            (10u128.saturating_mul(AMOUNT_SCALE), 1u128.saturating_mul(AMOUNT_SCALE)),
        );
        state
            .token_metrics_cache
            .insert(diesel, SchemaTokenMetricsV1 { price_usd: diesel_price, ..Default::default() });

        let mut canonical = HashMap::new();
        canonical.insert(FRBTC_ALKANE_ID, CanonicalQuoteUnit::Btc);
        prepare_total_volume_amm(10, &provider, &canonical, &mut state).expect("prepare total");

        provider
            .set_batch(SetBatchParams {
                blockhash: StateAt::Latest,
                puts: state.total_volume_amm_writes,
                deletes: Vec::new(),
            })
            .expect("write total volume");

        let expected_usd = 50_000u128.saturating_mul(PRICE_SCALE);
        let expected_sats = AMOUNT_SCALE;
        assert_eq!(
            provider
                .get_latest_total_volume_amm(TotalVolumeAmmUnit::Usd)
                .expect("latest usd")
                .map(|(_, value)| value),
            Some(expected_usd)
        );
        assert_eq!(
            provider
                .get_latest_total_volume_amm(TotalVolumeAmmUnit::Sats)
                .expect("latest sats")
                .map(|(_, value)| value),
            Some(expected_sats)
        );
    }
}
