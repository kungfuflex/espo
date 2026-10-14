//! Curve **CryptoSwap v2** pools, indexed alongside the OYL AMM.
//!
//! The BTCUSD market (frBTC/frUSD) is not an OYL pool. It was deployed directly
//! rather than through factory `4:65522`, its Exchange is opcode 5 rather than
//! 3, and it has no `0x61` GetReserves. So none of `index_pools`' discovery
//! paths can see it, and before this module the flagship market produced no
//! candles, no trade history, no volume and no TVL.
//!
//! Almost none of that needed new machinery. `index_activity` derives trades
//! from the pool alkane's own balance deltas and `index_pool_metrics` reads
//! reserves as balances and LP supply as circulating supply, so every one of
//! those is pool-shape agnostic already. Registering the pool into
//! `reserves_snapshot` and `pool_defs` is enough to light all of it up.
//!
//! Exactly one thing is genuinely CryptoSwap-specific, and it is the reason
//! this file is not just a static table:
//!
//! **Price is not the reserve ratio.** On a constant-product pool `quote/base`
//! *is* the spot price, which is why [`super::candles`] computes it that way.
//! On a Curve v2 pool the balances sit wherever the invariant puts them, and
//! the price lives in `price_scale`/`price_oracle`. The two agree only when the
//! pool is exactly at equilibrium, and they diverge by more the tighter the
//! curve is parameterised. Charting the ratio would print a price that is
//! directionally right and quantitatively wrong, and it would drift visibly
//! after any sizeable trade. So the state is read out of the contract's own
//! storage and the ratio is overridden.
//!
//! Reads go through the essentials KV rows rather than a contract call, because
//! espo has no simulate: it is a storage reader. That is fine here, because
//! `store.rs` writes every slot this module wants under a stable literal key.
//!
//! Source of truth for the slot names and scales:
//! `subfrost-alkanes/alkanes/cryptoswap-pool/src/store.rs`.

use crate::modules::ammdata::consts::{CanonicalQuoteUnit, PRICE_SCALE};
use crate::modules::ammdata::schemas::{
    SchemaCanonicalPoolEntry, SchemaMarketDefs, SchemaPoolSnapshot,
};
use crate::modules::ammdata::storage::AmmDataProvider;
use crate::modules::ammdata::utils::index_state::IndexState;
use crate::modules::essentials::storage::{EssentialsProvider, GetMultiValuesParams};
use crate::modules::essentials::utils::balances::get_alkane_balances;
use crate::runtime::state_at::StateAt;
use crate::schemas::SchemaAlkaneId;
use anyhow::Result;
use bitcoin::Network;
use std::collections::HashMap;

/// `price_scale`, `price_oracle`, `last_prices` and `virtual_price` are all
/// 1e18-scaled, matching upstream Curve. Note this is NOT espo's
/// [`PRICE_SCALE`], which is 1e16.
pub const WAD: u128 = 1_000_000_000_000_000_000;

/// A CryptoSwap pool espo indexes.
///
/// These are seeded from a static table rather than discovered. There is no
/// factory to watch: the pool is deployed by `ops/btcusd-pool-deploy.sh` and
/// its id is known ahead of time. A pool missing from this table is simply not
/// indexed, which is the safe direction to fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CryptoSwapPoolDef {
    pub pool: SchemaAlkaneId,
    /// coin0. Becomes the market's `base`, so candles are quoted quote-per-base.
    pub coin0: SchemaAlkaneId,
    /// coin1. Becomes the market's `quote`.
    pub coin1: SchemaAlkaneId,
    /// Name used for the pool name index when the alkane itself has no label.
    pub label: &'static str,
}

/// frBTC. coin0 of the BTCUSD pool.
const FRBTC: SchemaAlkaneId = SchemaAlkaneId { block: 32, tx: 0 };
/// frUSD, canonical deploy. coin1 of the BTCUSD pool.
///
/// `4:1785` also answers name/symbol `frUSD` and is NOT it.
const FRUSD: SchemaAlkaneId = SchemaAlkaneId { block: 4, tx: 1776 };

/// Mainnet BTCUSD, deployed and initialised 2026-08-06.
///
/// The id is the pool proxy, which is also the LP alkane: `mint_lp` transfers
/// `ctx.myself`. Storage therefore lives under this id, not under the
/// implementation, which is what makes the KV reads below work.
///
/// There is deliberately no deploy height here. A height would have to be
/// hand-maintained and would silently mis-register the pool if it were wrong,
/// whereas [`read_pool_state`] already answers `None` for a pool that does not
/// exist yet. Registration is gated on that instead, so the table stays
/// truthful about the one thing it knows: which alkane is which coin.
const MAINNET_POOLS: &[CryptoSwapPoolDef] = &[CryptoSwapPoolDef {
    pool: SchemaAlkaneId { block: 4, tx: 1778 },
    coin0: FRBTC,
    coin1: FRUSD,
    label: "BTCUSD",
}];

pub fn cryptoswap_pools(network: Network) -> &'static [CryptoSwapPoolDef] {
    match network {
        Network::Bitcoin => MAINNET_POOLS,
        _ => &[],
    }
}

pub fn cryptoswap_pool_def(
    network: Network,
    pool: &SchemaAlkaneId,
) -> Option<&'static CryptoSwapPoolDef> {
    cryptoswap_pools(network).iter().find(|def| &def.pool == pool)
}

pub fn is_cryptoswap_pool(network: Network, pool: &SchemaAlkaneId) -> bool {
    cryptoswap_pool_def(network, pool).is_some()
}

/// The slots read per block, in the order [`decode_state`] expects them.
const STATE_KEYS: [&[u8]; 6] = [
    b"/price_oracle",
    b"/price_scale",
    b"/last_prices",
    b"/balances/0",
    b"/balances/1",
    b"/virtual_price",
];

/// A CryptoSwap pool's priced state at one block.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CryptoSwapState {
    /// EMA price, coin0 per coin1, 1e18. The manipulation-resistant one.
    pub price_oracle: u128,
    /// Repeg target, coin0 per coin1, 1e18.
    pub price_scale: u128,
    /// Spot price of the last trade, coin0 per coin1, 1e18.
    pub last_prices: u128,
    /// Raw coin balances, in each coin's own units (both 8-decimal here).
    pub balance0: u128,
    pub balance1: u128,
    /// LP virtual price, 1e18.
    pub virtual_price: u128,
}

impl CryptoSwapState {
    /// The price to chart, coin0 per coin1 at 1e18.
    ///
    /// Prefers the EMA oracle. A pool that has been initialised but not yet
    /// traded has `price_oracle == 0` while `price_scale` already holds the
    /// initial price, so fall back rather than charting a zero. `last_prices`
    /// is the final fallback and is the least smoothed of the three.
    pub fn chart_price_wad(&self) -> Option<u128> {
        [self.price_oracle, self.price_scale, self.last_prices]
            .into_iter()
            .find(|p| *p != 0)
    }
}

/// Strip the `[last_txid(32 bytes)] + raw_value` framing essentials stores rows
/// in. A row shorter than the prefix is a raw value already.
fn row_value(raw: &[u8]) -> &[u8] {
    if raw.len() >= 32 { &raw[32..] } else { raw }
}

/// Decode a little-endian slot as `u128`.
///
/// `set_u256` writes 32 LE bytes and `set_u128` writes 16, so both are read by
/// taking the low 16 and requiring the rest to be zero. Every value this module
/// reads is a WAD price or an 8-decimal balance, so a set high half means the
/// slot is not what we think it is and `None` is the honest answer.
fn decode_le_u128(raw: &[u8]) -> Option<u128> {
    let value = row_value(raw);
    if value.len() < 16 {
        return None;
    }
    if value[16..].iter().any(|b| *b != 0) {
        return None;
    }
    let mut buf = [0u8; 16];
    buf.copy_from_slice(&value[..16]);
    Some(u128::from_le_bytes(buf))
}

fn decode_state(values: &[Option<Vec<u8>>]) -> Option<CryptoSwapState> {
    if values.len() != STATE_KEYS.len() {
        return None;
    }
    let slot = |i: usize| -> u128 {
        values[i].as_deref().and_then(decode_le_u128).unwrap_or(0)
    };
    let state = CryptoSwapState {
        price_oracle: slot(0),
        price_scale: slot(1),
        last_prices: slot(2),
        balance0: slot(3),
        balance1: slot(4),
        virtual_price: slot(5),
    };
    // An uninitialised pool reads back as all zeroes. Registering that would
    // pin the chart to zero, so treat it as "no state yet".
    if state.chart_price_wad().is_none() {
        return None;
    }
    Some(state)
}

/// Read a CryptoSwap pool's state out of its own contract storage at `blockhash`.
///
/// Returns `None` when the pool has not been initialised yet, or when a slot
/// decodes to something that is not a plain little-endian integer.
pub fn read_pool_state(
    blockhash: StateAt,
    essentials: &EssentialsProvider,
    pool: &SchemaAlkaneId,
) -> Option<CryptoSwapState> {
    let table = essentials.table();
    let keys = STATE_KEYS.iter().map(|k| table.kv_row_key(pool, k)).collect::<Vec<_>>();
    let values = essentials.get_multi_values(GetMultiValuesParams { blockhash, keys }).ok()?.values;
    decode_state(&values)
}

/// Convert a 1e18 coin0-per-coin1 price into the `(quote_per_base, base_per_quote)`
/// pair espo's candles carry, both at [`PRICE_SCALE`] (1e16).
///
/// With base = coin0 and quote = coin1, the stored price is base-per-quote, so
/// the chart price is its reciprocal:
///
/// - `quote_per_base = 1e18 / p`, scaled to 1e16 => `1e34 / p`
/// - `base_per_quote = p / 1e18`, scaled to 1e16 => `p / 100`
///
/// Measured against the live pool on 2026-08-06: `p = 15_585_063_275_356`
/// gives `quote_per_base = 64_164` USD per BTC at 1e16 scale.
///
/// `1e34` is well inside `u128` (max ~3.4e38), so the reciprocal cannot
/// overflow; a `p` small enough to matter is rejected as a zero price upstream.
pub fn prices_from_wad(price_wad: u128) -> Option<(u128, u128)> {
    if price_wad == 0 {
        return None;
    }
    let quote_per_base = WAD.checked_mul(PRICE_SCALE)? / price_wad;
    let base_per_quote = price_wad / (WAD / PRICE_SCALE);
    if quote_per_base == 0 || base_per_quote == 0 {
        return None;
    }
    Some((quote_per_base, base_per_quote))
}

/// Register every configured CryptoSwap pool and publish its price for the block.
///
/// Runs before `process_balance_deltas`, because that is what consumes both the
/// `pools_map` entry and the price override.
///
/// Registration is idempotent and self-gating: a pool already in `pools_map`
/// only has its price refreshed, and a pool whose storage does not read back is
/// skipped entirely, so this is a no-op on every block before the deploy and on
/// every network where the table is empty.
///
/// Returns the number of pools newly registered.
pub fn register_cryptoswap_pools(
    blockhash: StateAt,
    network: Network,
    provider: &AmmDataProvider,
    essentials: &EssentialsProvider,
    canonical_quote_units: &HashMap<SchemaAlkaneId, CanonicalQuoteUnit>,
    state: &mut IndexState,
) -> Result<usize> {
    let pools = cryptoswap_pools(network);
    if pools.is_empty() {
        return Ok(0);
    }
    let table = provider.table();
    let mut registered = 0usize;

    for def in pools {
        let Some(pool_state) = read_pool_state(blockhash.clone(), essentials, &def.pool) else {
            // Not deployed yet, or not initialised. Nothing to index.
            continue;
        };

        // Publish the price first: an already-registered pool still needs this
        // refreshed every block, and it must land even if registration is a
        // no-op.
        if let Some(prices) = pool_state.chart_price_wad().and_then(prices_from_wad) {
            state.pool_price_overrides.insert(def.pool, prices);
        }

        if state.pools_map.contains_key(&def.pool) {
            continue;
        }

        let defs = SchemaMarketDefs {
            pool_alkane_id: def.pool,
            base_alkane_id: def.coin0,
            quote_alkane_id: def.coin1,
        };
        state.pools_map.insert(def.pool, defs);
        if let Ok(encoded) = borsh::to_vec(&defs) {
            state.pool_defs_writes.push((table.pools_key(&def.pool), encoded));
        }
        state
            .token_pools_writes
            .push((table.token_pools_key(&defs.base_alkane_id, &def.pool), Vec::new()));
        state
            .token_pools_writes
            .push((table.token_pools_key(&defs.quote_alkane_id, &def.pool), Vec::new()));

        // Reserves are read as balances held by the pool alkane, exactly as the
        // OYL path does. `/balances/0` and `/balances/1` are the contract's own
        // accounting and would double as a cross-check, but the balance view is
        // what the rest of the pipeline applies its deltas to, so it has to be
        // the one seeded here or the first trade would skew the snapshot.
        let mut pool_balances =
            get_alkane_balances(blockhash.clone(), essentials, &def.pool).unwrap_or_default();
        let base_reserve = pool_balances.remove(&defs.base_alkane_id).unwrap_or(0);
        let quote_reserve = pool_balances.remove(&defs.quote_alkane_id).unwrap_or(0);
        state.reserves_snapshot.insert(
            def.pool,
            SchemaPoolSnapshot {
                base_reserve,
                quote_reserve,
                base_id: defs.base_alkane_id,
                quote_id: defs.quote_alkane_id,
            },
        );

        // Canonical-pool wiring is what lets a token price itself off this
        // market. frBTC is a canonical BTC quote, so the frUSD side gets a BTC
        // price from here for free.
        if canonical_quote_units.contains_key(&defs.quote_alkane_id) {
            state
                .canonical_pool_updates
                .entry(defs.base_alkane_id)
                .or_default()
                .push(SchemaCanonicalPoolEntry {
                    pool_id: def.pool,
                    quote_id: defs.quote_alkane_id,
                });
        }
        if canonical_quote_units.contains_key(&defs.base_alkane_id) {
            state
                .canonical_pool_updates
                .entry(defs.quote_alkane_id)
                .or_default()
                .push(SchemaCanonicalPoolEntry {
                    pool_id: def.pool,
                    quote_id: defs.base_alkane_id,
                });
        }

        let label = crate::modules::ammdata::utils::index_pools::get_alkane_label(
            blockhash.clone(),
            essentials,
            &mut state.alkane_label_cache,
            &def.pool,
        );
        let name = crate::modules::ammdata::strip_lp_suffix(&label);
        let name_norm = if name.trim().is_empty() {
            def.label.to_ascii_lowercase()
        } else {
            name.trim().to_ascii_lowercase()
        };
        if !name_norm.is_empty() {
            state
                .pool_name_index_writes
                .push((table.pool_name_index_key(&name_norm, &def.pool), Vec::new()));
        }

        registered = registered.saturating_add(1);
        eprintln!(
            "[AMMDATA] registered cryptoswap pool {}/{} ({}) base={}/{} quote={}/{}",
            def.pool.block,
            def.pool.tx,
            def.label,
            defs.base_alkane_id.block,
            defs.base_alkane_id.tx,
            defs.quote_alkane_id.block,
            defs.quote_alkane_id.tx,
        );
    }

    Ok(registered)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wad_row(v: u128) -> Vec<u8> {
        let mut row = vec![0u8; 32];
        row.extend_from_slice(&v.to_le_bytes());
        row.extend_from_slice(&[0u8; 16]);
        row
    }

    #[test]
    fn mainnet_btcusd_is_registered_and_devnet_is_not() {
        let pool = SchemaAlkaneId { block: 4, tx: 1778 };
        assert!(is_cryptoswap_pool(Network::Bitcoin, &pool));
        assert!(!is_cryptoswap_pool(Network::Regtest, &pool));

        let def = cryptoswap_pool_def(Network::Bitcoin, &pool).expect("registered");
        assert_eq!(def.coin0, FRBTC);
        assert_eq!(def.coin1, FRUSD);
    }

    #[test]
    fn the_live_price_scale_reads_back_as_the_measured_btc_price() {
        // Read off the live pool on 2026-08-06, when it quoted 64,164 USD/BTC.
        let price_wad = 15_585_063_275_356;
        let (quote_per_base, base_per_quote) = prices_from_wad(price_wad).expect("nonzero");

        // USD per BTC, at espo's 1e16 scale.
        assert_eq!(quote_per_base / PRICE_SCALE, 64_164);
        // BTC per USD is the stored price rescaled 1e18 -> 1e16, exactly.
        assert_eq!(base_per_quote, price_wad / 100);

        // The two are reciprocal to within the truncation of a 1e16 fixed point,
        // which at this price is ~1 part in 6e20. Asserting exact equality would
        // be asserting that integer division is lossless, which it is not.
        let round_tripped = WAD * PRICE_SCALE / (base_per_quote * (WAD / PRICE_SCALE));
        let drift = round_tripped.abs_diff(quote_per_base);
        assert!(
            drift * 1_000_000_000 < quote_per_base,
            "reciprocal drifted by more than a billionth: {drift} on {quote_per_base}"
        );
    }

    #[test]
    fn a_zero_price_is_rejected_rather_than_charted() {
        assert!(prices_from_wad(0).is_none());
    }

    #[test]
    fn the_oracle_is_preferred_but_an_untraded_pool_falls_back_to_the_repeg_target() {
        let traded = CryptoSwapState {
            price_oracle: 10,
            price_scale: 20,
            last_prices: 30,
            ..Default::default()
        };
        assert_eq!(traded.chart_price_wad(), Some(10));

        let fresh = CryptoSwapState { price_scale: 20, last_prices: 30, ..Default::default() };
        assert_eq!(fresh.chart_price_wad(), Some(20));

        assert_eq!(CryptoSwapState::default().chart_price_wad(), None);
    }

    #[test]
    fn state_decodes_past_the_row_txid_prefix() {
        let values = vec![
            Some(wad_row(15_585_063_275_356)),
            Some(wad_row(15_500_000_000_000)),
            Some(wad_row(15_600_000_000_000)),
            Some(wad_row(10_000_000)),
            Some(wad_row(6_416_400_000_000)),
            Some(wad_row(WAD)),
        ];
        let state = decode_state(&values).expect("decodes");
        assert_eq!(state.price_oracle, 15_585_063_275_356);
        assert_eq!(state.balance0, 10_000_000);
        assert_eq!(state.virtual_price, WAD);
    }

    #[test]
    fn an_uninitialised_pool_decodes_to_no_state_rather_than_a_zero_price() {
        let values = vec![None, None, None, None, None, None];
        assert!(decode_state(&values).is_none());
    }

    #[test]
    fn a_slot_wider_than_u128_is_refused_rather_than_truncated() {
        let mut row = vec![0u8; 32];
        row.extend_from_slice(&u128::MAX.to_le_bytes());
        row.extend_from_slice(&[1u8; 16]); // high half set
        assert!(decode_le_u128(&row).is_none());
    }
}
