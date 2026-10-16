//! Depth-aware selection of a token's canonical pools when building its USD candle.
//!
//! A token can trade against the same canonical quote (frBTC) in several pools. The
//! USD candle used to take whichever of those pools came last in a `HashMap`
//! iteration, regardless of depth, so a dust pool could set the token's price: on
//! 2026-10-15 a 2 DIESEL sell into DIESEL/frBTC 2:96884 (~0.005 BTC deep) printed
//! DIESEL at $27.76 while the 93 BTC pool 2:77087 sat at $36.7 (block 971505), and a
//! 1 DIESEL buy printed $41.08 (block 971584).
//!
//! Rule, per token and per canonical unit:
//! - depth = the pool's canonical-side reserve after the block;
//! - the deepest pool (ties: lowest pool id) always survives, so a token whose only
//!   pool is shallow keeps being priced exactly as before;
//! - every other pool survives only at depth >= max(10% of the deepest, the unit floor);
//! - the survivors that have a candle in the bucket are combined with a
//!   depth-weighted OHLC; a single one is returned untouched (byte-identical);
//! - if no survivor has a candle in the bucket, there is no update, so the token's
//!   previous close carries forward.

use crate::modules::ammdata::consts::CanonicalQuoteUnit;
use crate::modules::ammdata::schemas::{SchemaCandleV1, SchemaPoolSnapshot};
use crate::schemas::SchemaAlkaneId;

/// A non-deepest pool must hold at least this share of the deepest pool's
/// canonical-side reserve (in basis points) to contribute.
pub const MIN_RELATIVE_DEPTH_BPS: u128 = 1_000; // 10%

/// Absolute depth floor for a non-deepest pool against a BTC-unit quote (frBTC sats).
pub const MIN_ABS_DEPTH_BTC: u128 = 1_000_000; // 0.01 BTC

/// The absolute floor per canonical unit, in that quote's base units. The USD unit
/// (bUSD, canonical only below its fork height) has no absolute floor; the relative
/// rule still applies to it.
pub fn min_abs_depth(unit: CanonicalQuoteUnit) -> u128 {
    match unit {
        CanonicalQuoteUnit::Btc => MIN_ABS_DEPTH_BTC,
        CanonicalQuoteUnit::Usd => 0,
    }
}

/// The pool's reserve of the canonical quote `quote_id` (0 if unknown).
pub fn canonical_side_depth(
    snapshot: Option<&SchemaPoolSnapshot>,
    quote_id: &SchemaAlkaneId,
) -> u128 {
    match snapshot {
        Some(s) if s.base_id == *quote_id => s.base_reserve,
        Some(s) if s.quote_id == *quote_id => s.quote_reserve,
        _ => 0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanonicalPoolCandidate {
    pub pool_id: SchemaAlkaneId,
    /// Canonical-side reserve after the block (0 if the pool has no snapshot).
    pub depth: u128,
    /// The pool's candle for the bucket, already converted to USD; `None` if the pool
    /// did not trade in the bucket (or its price could not be converted).
    pub candle: Option<SchemaCandleV1>,
}

/// Combine one canonical unit's pools into a single USD candle for the bucket.
/// The result does not depend on the order of `candidates`.
pub fn combine_canonical_unit_candles(
    candidates: &[CanonicalPoolCandidate],
    min_abs: u128,
) -> Option<SchemaCandleV1> {
    let mut sorted: Vec<CanonicalPoolCandidate> = candidates.to_vec();
    // Deepest first; ties by pool id so the reference pool is well defined.
    sorted.sort_by(|a, b| b.depth.cmp(&a.depth).then(a.pool_id.cmp(&b.pool_id)));
    let reference = sorted.first()?;
    let relative_floor = reference.depth.saturating_mul(MIN_RELATIVE_DEPTH_BPS) / 10_000;
    let floor = relative_floor.max(min_abs);

    let survivors: Vec<(u128, SchemaCandleV1)> = sorted
        .iter()
        .enumerate()
        .filter(|(i, c)| *i == 0 || c.depth >= floor)
        .filter_map(|(_, c)| c.candle.map(|candle| (c.depth, candle)))
        .collect();

    match survivors.len() {
        0 => None,
        1 => Some(survivors[0].1),
        _ => Some(depth_weighted(&survivors)),
    }
}

fn depth_weighted(survivors: &[(u128, SchemaCandleV1)]) -> SchemaCandleV1 {
    let total: u128 = survivors.iter().fold(0u128, |acc, (d, _)| acc.saturating_add(*d));
    if total == 0 {
        // No depth information: fall back to an equal-weight mean.
        let n = survivors.len() as u128;
        let mean = |f: fn(&SchemaCandleV1) -> u128| -> u128 {
            survivors.iter().fold(0u128, |acc, (_, c)| acc.saturating_add(f(c))) / n
        };
        return SchemaCandleV1 {
            open: mean(|c| c.open),
            high: mean(|c| c.high),
            low: mean(|c| c.low),
            close: mean(|c| c.close),
            volume: survivors.iter().fold(0u128, |acc, (_, c)| acc.saturating_add(c.volume)),
        };
    }
    let weighted = |f: fn(&SchemaCandleV1) -> u128| -> u128 {
        // Σ p·w / Σ w. Prices are 1e16-scaled and depths are 1e8-scaled reserves, so
        // the products stay far inside u128 for any realistic pool; on overflow, fall
        // back to dividing first (loses a little precision, never wraps).
        let mut acc: Option<u128> = Some(0);
        for (d, c) in survivors {
            acc = acc.and_then(|a| f(c).checked_mul(*d).and_then(|x| a.checked_add(x)));
        }
        match acc {
            Some(sum) => sum / total,
            None => survivors
                .iter()
                .fold(0u128, |a, (d, c)| a.saturating_add((f(c) / total).saturating_mul(*d))),
        }
    };
    let mut out = SchemaCandleV1 {
        open: weighted(|c| c.open),
        high: weighted(|c| c.high),
        low: weighted(|c| c.low),
        close: weighted(|c| c.close),
        volume: survivors.iter().fold(0u128, |acc, (_, c)| acc.saturating_add(c.volume)),
    };
    // Keep the candle well-formed after averaging.
    out.high = out.high.max(out.open).max(out.close);
    out.low = out.low.min(out.open).min(out.close);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::ammdata::consts::PRICE_SCALE;

    const MAIN: SchemaAlkaneId = SchemaAlkaneId { block: 2, tx: 77087 };
    const THIN: SchemaAlkaneId = SchemaAlkaneId { block: 2, tx: 96884 };

    // BTC/USD implied by espo-g's 971505 print (27.755042 / 0.00034378), 1e16-scaled.
    const BTC_USD: u128 = 80_734 * PRICE_SCALE;

    /// BTC-per-DIESEL (1e16-scaled) -> USD (1e16-scaled), the same conversion
    /// derive_token_data applies.
    fn usd(btc_per_token_e16: u128) -> u128 {
        btc_per_token_e16 * BTC_USD / PRICE_SCALE
    }

    fn candle(o: u128, h: u128, l: u128, c: u128, v: u128) -> SchemaCandleV1 {
        SchemaCandleV1 { open: usd(o), high: usd(h), low: usd(l), close: usd(c), volume: v }
    }

    /// The pre-fix behaviour: the last pool (in iteration order) with a candle wins.
    fn last_wins(c: &[CanonicalPoolCandidate]) -> Option<SchemaCandleV1> {
        c.iter().filter_map(|x| x.candle).last()
    }

    // Post-block canonical-side (frBTC) reserves, in sats.
    const MAIN_DEPTH: u128 = 9_326_850_750; // ~93.27 BTC
    const THIN_DEPTH_971505: u128 = 514_800; // ~0.0051 BTC after the 2 DIESEL sell
    const THIN_DEPTH_971584: u128 = 626_320; // ~0.0063 BTC after the 1 DIESEL buy

    /// Block 971505 (03:59:48Z, 03:50 bucket): tx c2b593d7… sold 2 DIESEL into 2:96884,
    /// moving it 0.00045709 -> 0.00034378 BTC; 2:77087 did not trade in the bucket.
    #[test]
    fn block_971505_thin_pool_wick_is_ignored() {
        let thin =
            candle(4_570_900_000_000, 4_570_900_000_000, 3_437_800_000_000, 3_437_800_000_000, 2);
        let cands = [
            CanonicalPoolCandidate { pool_id: MAIN, depth: MAIN_DEPTH, candle: None },
            CanonicalPoolCandidate { pool_id: THIN, depth: THIN_DEPTH_971505, candle: Some(thin) },
        ];
        // Before: the wick (~$27.75).
        let before = last_wins(&cands).unwrap();
        assert_eq!(before.close / PRICE_SCALE, 27);
        // After: no update, so the token keeps the previous close (the main pool's
        // ~$36.67 at 03:40), which is what the chart then carries forward.
        assert_eq!(combine_canonical_unit_candles(&cands, MIN_ABS_DEPTH_BTC), None);
    }

    /// Block 971584 (14:12:17Z, 14:10 bucket): tx 4b945096… bought 1 DIESEL from
    /// 2:96884 (-> 0.00050804 BTC) while tx 408e70dd… sold 1.6 DIESEL into 2:77087
    /// (0.00045151 BTC) in the same block.
    #[test]
    fn block_971584_main_pool_sets_the_price() {
        let main =
            candle(4_515_100_000_000, 4_515_100_000_000, 4_515_100_000_000, 4_515_100_000_000, 7);
        let thin =
            candle(4_340_300_000_000, 5_080_400_000_000, 4_340_300_000_000, 5_080_400_000_000, 1);
        let cands = [
            CanonicalPoolCandidate { pool_id: MAIN, depth: MAIN_DEPTH, candle: Some(main) },
            CanonicalPoolCandidate { pool_id: THIN, depth: THIN_DEPTH_971584, candle: Some(thin) },
        ];
        let before = last_wins(&cands).unwrap();
        assert_eq!(before.close / PRICE_SCALE, 41);
        let after = combine_canonical_unit_candles(&cands, MIN_ABS_DEPTH_BTC).unwrap();
        assert_eq!(after, main);
        assert_eq!(after.close / PRICE_SCALE, 36);
    }

    #[test]
    fn single_pool_is_returned_untouched_even_when_shallow() {
        let c = candle(10, 20, 5, 15, 3);
        for depth in [0u128, 1, 500, MIN_ABS_DEPTH_BTC - 1, MAIN_DEPTH] {
            let cands = [CanonicalPoolCandidate { pool_id: THIN, depth, candle: Some(c) }];
            assert_eq!(combine_canonical_unit_candles(&cands, MIN_ABS_DEPTH_BTC), Some(c));
        }
        let none = [CanonicalPoolCandidate { pool_id: THIN, depth: 5, candle: None }];
        assert_eq!(combine_canonical_unit_candles(&none, MIN_ABS_DEPTH_BTC), None);
    }

    #[test]
    fn only_the_deepest_traded_matches_the_old_result() {
        let main = candle(4_500, 4_600, 4_400, 4_550, 9);
        let cands = [
            CanonicalPoolCandidate { pool_id: THIN, depth: THIN_DEPTH_971584, candle: None },
            CanonicalPoolCandidate { pool_id: MAIN, depth: MAIN_DEPTH, candle: Some(main) },
        ];
        assert_eq!(combine_canonical_unit_candles(&cands, MIN_ABS_DEPTH_BTC), last_wins(&cands));
    }

    #[test]
    fn deep_pools_are_depth_weighted() {
        let a = SchemaCandleV1 { open: 100, high: 120, low: 90, close: 110, volume: 1 };
        let b = SchemaCandleV1 { open: 200, high: 220, low: 190, close: 210, volume: 2 };
        let pa = SchemaAlkaneId { block: 2, tx: 1 };
        let pb = SchemaAlkaneId { block: 2, tx: 2 };
        // b is 1/3 of a's depth: above the 10% bar and the floor.
        let cands = [
            CanonicalPoolCandidate {
                pool_id: pa,
                depth: 3 * MIN_ABS_DEPTH_BTC * 10,
                candle: Some(a),
            },
            CanonicalPoolCandidate { pool_id: pb, depth: MIN_ABS_DEPTH_BTC * 10, candle: Some(b) },
        ];
        let out = combine_canonical_unit_candles(&cands, MIN_ABS_DEPTH_BTC).unwrap();
        assert_eq!(out, SchemaCandleV1 { open: 125, high: 145, low: 115, close: 135, volume: 3 });
    }

    #[test]
    fn absolute_floor_drops_a_non_deepest_pool() {
        let a = SchemaCandleV1 { open: 100, high: 100, low: 100, close: 100, volume: 1 };
        let b = SchemaCandleV1 { open: 900, high: 900, low: 900, close: 900, volume: 1 };
        let pa = SchemaAlkaneId { block: 2, tx: 1 };
        let pb = SchemaAlkaneId { block: 2, tx: 2 };
        // b is 50% of a, but under 0.01 BTC.
        let cands = [
            CanonicalPoolCandidate { pool_id: pa, depth: 800_000, candle: Some(a) },
            CanonicalPoolCandidate { pool_id: pb, depth: 400_000, candle: Some(b) },
        ];
        assert_eq!(combine_canonical_unit_candles(&cands, MIN_ABS_DEPTH_BTC), Some(a));
        // With no absolute floor (USD unit) the relative rule alone keeps b.
        let out = combine_canonical_unit_candles(&cands, 0).unwrap();
        assert_eq!(out.close, (100 * 800_000 + 900 * 400_000) / 1_200_000);
    }

    #[test]
    fn result_is_independent_of_input_order() {
        let mk = |tx: u64, depth: u128, px: u128| CanonicalPoolCandidate {
            pool_id: SchemaAlkaneId { block: 2, tx },
            depth,
            candle: Some(SchemaCandleV1 {
                open: px,
                high: px + 7,
                low: px - 3,
                close: px + 1,
                volume: tx as u128,
            }),
        };
        let base = vec![
            mk(10, 50_000_000, 1_000),
            mk(11, 20_000_000, 1_300),
            mk(12, 4_000_000, 9_000),  // under 10% of 50M -> dropped
            mk(13, 50_000_000, 1_100), // ties the deepest; lower id wins the reference
            CanonicalPoolCandidate {
                pool_id: SchemaAlkaneId { block: 2, tx: 14 },
                depth: 30_000_000,
                candle: None,
            },
        ];
        let expected = combine_canonical_unit_candles(&base, MIN_ABS_DEPTH_BTC);
        assert!(expected.is_some());
        // Every permutation of the five candidates.
        let mut idx: Vec<usize> = (0..base.len()).collect();
        let mut count = 0;
        permute(&mut idx, 0, &mut |perm| {
            let v: Vec<_> = perm.iter().map(|&i| base[i]).collect();
            assert_eq!(combine_canonical_unit_candles(&v, MIN_ABS_DEPTH_BTC), expected);
            count += 1;
        });
        assert_eq!(count, 120);
    }

    fn permute(v: &mut Vec<usize>, k: usize, f: &mut dyn FnMut(&[usize])) {
        if k == v.len() {
            f(v);
            return;
        }
        for i in k..v.len() {
            v.swap(k, i);
            permute(v, k + 1, f);
            v.swap(k, i);
        }
    }
}
