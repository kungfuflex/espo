use crate::schemas::SchemaAlkaneId;
use anyhow::{Result, anyhow};
use bitcoin::Network;

pub fn ammdata_genesis_block(network: Network) -> u32 {
    crate::consts::genesis_with_override(match network {
        Network::Bitcoin => 904_647,
        _ => 0,
    })
}

pub fn get_amm_contract(network: Network) -> Result<SchemaAlkaneId> {
    match network {
        Network::Bitcoin => Ok(SchemaAlkaneId { block: 4u32, tx: 65522u64 }),
        _ => Err(anyhow!("AMMDATA ERROR: Amm contract not defined for this network")),
    }
}

pub const KEY_INDEX_HEIGHT: &[u8] = b"/index_height";
pub const GET_RESERVES_OPCODE: u8 = 0x61;
pub const DEPLOY_AMM_OPCODE: u8 = 0x01;
pub const PRICE_SCALE_DECIMALS: u32 = 16;
pub const PRICE_SCALE: u128 = 10_000_000_000_000_000; // 1e16
pub const AMOUNT_SCALE: u128 = 100_000_000; // on-chain token amount precision (1e8)
pub const SATS_PER_BTC: u128 = AMOUNT_SCALE;

/// Decimals every token is priced against: ammdata stores and serves a token's
/// price as quote per `AMOUNT_SCALE` (1e8) RAW units, whatever the token's own
/// decimals. Its USD aggregates (TVL, volume, market cap) are right for any
/// token, because they multiply that price by `raw / AMOUNT_SCALE`; only the
/// per-token PRICE is off for a token that is not 8 decimals, by 10^(d - 8).
pub const AMOUNT_SCALE_DECIMALS: u32 = 8;

/// Mainnet tokens whose decimals are not 8. The one table: the SUBZERO L0 peg
/// tokens carry their native chain's base unit and cannot declare it on chain
/// (their `__meta` has no decimals method; opcode 102 reverts, probed
/// 2026-10-14 at 971500). Evidence: the get_limits mint caps are 4,000 SOL /
/// 500,000 XRP / 100,000 ADA only at these scales, and the SUBFROST AMM pools
/// 2:103202..2:103204 were seeded at market only at these scales.
/// Same table as subvh apps/explorer lib/alkanes/decimals-known.ts.
pub const MAINNET_NONSTANDARD_DECIMALS: &[(SchemaAlkaneId, u32)] = &[
    (SchemaAlkaneId { block: 4, tx: 50050 }, 9), // frSOL (lamports)
    (SchemaAlkaneId { block: 4, tx: 50051 }, 6), // frXRP (drops)
    (SchemaAlkaneId { block: 4, tx: 50052 }, 6), // frADA (lovelace)
];

/// A token's own decimals, `AMOUNT_SCALE_DECIMALS` unless the table says otherwise.
pub fn token_decimals(network: Network, id: &SchemaAlkaneId) -> u32 {
    if network != Network::Bitcoin {
        return AMOUNT_SCALE_DECIMALS;
    }
    MAINNET_NONSTANDARD_DECIMALS
        .iter()
        .find(|(t, _)| t == id)
        .map(|(_, d)| *d)
        .unwrap_or(AMOUNT_SCALE_DECIMALS)
}

/// A price per `AMOUNT_SCALE` raw units → the price per WHOLE token of
/// `decimals`. Render-time only: nothing stored changes.
pub fn price_per_whole_token(price: u128, decimals: u32) -> u128 {
    if decimals >= AMOUNT_SCALE_DECIMALS {
        price.saturating_mul(10u128.pow(decimals - AMOUNT_SCALE_DECIMALS))
    } else {
        price / 10u128.pow(AMOUNT_SCALE_DECIMALS - decimals)
    }
}
pub const K_TOLERANCE_BPS: u128 = 10; // 0.1%
pub const FRBTC_ALKANE_ID: SchemaAlkaneId = SchemaAlkaneId { block: 32, tx: 0 };
pub const BUSD_ALKANE_ID: SchemaAlkaneId = SchemaAlkaneId { block: 2, tx: 56801 };
pub const MAINNET_FIRE_ALKANE_ID: SchemaAlkaneId = SchemaAlkaneId { block: 2, tx: 77623 };
pub const MAINNET_FIRE_USD_CHART_START_TS: u64 = 1_780_875_600;
// BUSD stops contributing as a canonical USD quote at this height and is treated as a normal token.
pub const BUSD_CANONICAL_QUOTE_FORK_HEIGHT: u32 = 946_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalQuoteUnit {
    Btc,
    Usd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalQuote {
    pub id: SchemaAlkaneId,
    pub unit: CanonicalQuoteUnit,
}

pub fn busd_is_canonical_quote_at_height(height: u32) -> bool {
    height < BUSD_CANONICAL_QUOTE_FORK_HEIGHT
}

pub fn canonical_quotes(network: Network) -> Vec<CanonicalQuote> {
    canonical_quotes_at_height(network, 0)
}

pub fn canonical_quotes_at_height(network: Network, height: u32) -> Vec<CanonicalQuote> {
    let mut mainnet = vec![CanonicalQuote { id: FRBTC_ALKANE_ID, unit: CanonicalQuoteUnit::Btc }];
    if busd_is_canonical_quote_at_height(height) {
        mainnet.push(CanonicalQuote { id: BUSD_ALKANE_ID, unit: CanonicalQuoteUnit::Usd });
    }
    match network {
        Network::Bitcoin => mainnet,
        _ => mainnet,
    }
}

/// AMM factories whose pools may set a token's price on mainnet.
///
/// Espo indexes pools from any contract that looks like an AMM factory - several
/// hundred do - and a factory only guarantees one pool per pair among its own
/// pools. So anyone can deploy a factory and open a second pool for an existing
/// pair with a few dollars in it. Pools from factories not listed here are still
/// indexed, charted and counted; they just cannot move a token's price.
const MAINNET_PRICING_FACTORIES: &[SchemaAlkaneId] = &[SchemaAlkaneId { block: 4, tx: 65522 }];

/// From this height only `MAINNET_PRICING_FACTORIES` price tokens. Before it every
/// recognised factory did, and that history is left as it was indexed.
pub const MAINNET_PRICING_FACTORY_FORK_HEIGHT: u32 = 970_000;

/// The factories allowed to price tokens at `height`, or `None` when any
/// recognised factory may (other networks, and mainnet before the fork height).
pub fn pricing_factories_at_height(
    network: Network,
    height: u32,
) -> Option<&'static [SchemaAlkaneId]> {
    match network {
        Network::Bitcoin if height >= MAINNET_PRICING_FACTORY_FORK_HEIGHT => {
            Some(MAINNET_PRICING_FACTORIES)
        }
        _ => None,
    }
}

/// A (token, derived quote) pair that detaches from its derived pool at a height.
///
/// Before the height the token's `-derived_<quote>-usd` chart merges the token's
/// canonical (frBTC) pricing with its price through the quote's pool. From the
/// height on, the pool is no longer registered as the pair's derived pool, so the
/// quote's price moves stop being written into new candles and the chart follows
/// the token's direct USD pricing only - as if `-usd` had been requested. The
/// chart's history before the height is untouched.
#[derive(Debug, Clone, Copy)]
pub struct DerivedQuoteFork {
    pub token: SchemaAlkaneId,
    pub quote: SchemaAlkaneId,
    pub height: u32,
}

/// The height upstream compiled in for TORTILLA's detach from DIESEL. Kept as a
/// reference value only: this fork does NOT apply it from a constant. The detach
/// is an operator decision and is expressed in config as
/// `derive_sources: [{ "alkane": "2:68479", "drop_forks": [{ "alkane": "2:0",
/// "height": 969393 }] }]`, which `DerivedIndexPlan::pair_dropped` applies with
/// exactly the same effect.
pub const MAINNET_TORTILLA_DIESEL_DETACH_HEIGHT: u32 = 969_393;

/// Compiled-in derived-quote forks. Deliberately empty: every fork is config.
const MAINNET_DERIVED_QUOTE_FORKS: &[DerivedQuoteFork] = &[];

/// True when `token`'s derived chart against `quote` must ignore the quote's pool
/// at `height`.
pub fn derived_quote_detached_at_height(
    network: Network,
    token: &SchemaAlkaneId,
    quote: &SchemaAlkaneId,
    height: u32,
) -> bool {
    let table: &[DerivedQuoteFork] = match network {
        Network::Bitcoin => MAINNET_DERIVED_QUOTE_FORKS,
        _ => &[],
    };
    table
        .iter()
        .any(|f| f.token == *token && f.quote == *quote && height >= f.height)
}

/// Holders whose balance of `alkane` is treated as non-circulating by the
/// `cmcap` chart: vesting contracts, treasuries, LP pools the team seeded, burn
/// sinks. A network fact, so it lives here rather than in config.
#[derive(Debug, Clone, Copy)]
pub struct NonCirculatingHolders {
    pub alkane: SchemaAlkaneId,
    pub addresses: &'static [&'static str],
    pub alkanes: &'static [SchemaAlkaneId],
}

const MAINNET_NON_CIRCULATING: &[NonCirculatingHolders] = &[
    // DIESEL
    NonCirculatingHolders {
        alkane: SchemaAlkaneId { block: 2, tx: 0 },
        addresses: &["bc1phqvgwn7wn5e4s8g0999rtgafd07jpuuy59rkdrk4s5thw9jafkasg8umr8"],
        alkanes: &[],
    },
    // TORTILLA: the deployer contract and the burn address. The TORT/frBTC LP pool
    // is deliberately not here - pooled liquidity is circulating.
    NonCirculatingHolders {
        alkane: SchemaAlkaneId { block: 2, tx: 68479 },
        addresses: &["1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"],
        alkanes: &[SchemaAlkaneId { block: 2, tx: 68478 }],
    },
    // SLICE: four lockers and the treasury address.
    NonCirculatingHolders {
        alkane: SchemaAlkaneId { block: 4, tx: 8888 },
        addresses: &["bc1pz860hkyek9584tx5l84pyax5puu7crerspaksz0jen2svww3ce2smzyjer"],
        alkanes: &[
            SchemaAlkaneId { block: 4, tx: 55 },
            SchemaAlkaneId { block: 4, tx: 53 },
            SchemaAlkaneId { block: 4, tx: 54 },
            SchemaAlkaneId { block: 4, tx: 56 },
        ],
    },
];

pub fn non_circulating_holders(
    network: Network,
    token: &SchemaAlkaneId,
) -> Option<&'static NonCirculatingHolders> {
    let table: &[NonCirculatingHolders] = match network {
        Network::Bitcoin => MAINNET_NON_CIRCULATING,
        _ => &[],
    };
    table.iter().find(|h| h.alkane == *token)
}

#[cfg(test)]
mod tests {

    #[test]
    fn nonstandard_decimals_table_and_per_token_price() {
        let frsol = SchemaAlkaneId { block: 4, tx: 50050 };
        let frxrp = SchemaAlkaneId { block: 4, tx: 50051 };
        let frada = SchemaAlkaneId { block: 4, tx: 50052 };
        assert_eq!(token_decimals(Network::Bitcoin, &frsol), 9);
        assert_eq!(token_decimals(Network::Bitcoin, &frxrp), 6);
        assert_eq!(token_decimals(Network::Bitcoin, &frada), 6);
        assert_eq!(token_decimals(Network::Bitcoin, &FRBTC_ALKANE_ID), 8);
        // Mainnet ids only: 4:50050 is something else on another network.
        assert_eq!(token_decimals(Network::Regtest, &frsol), 8);
        // Live espo closes (2026-10-14) -> per whole token.
        assert_eq!(price_per_whole_token(103_192_000_000_000_000, 9), 1_031_920_000_000_000_000);
        assert_eq!(price_per_whole_token(226_765_320_000_000_000, 6), 2_267_653_200_000_000);
        assert_eq!(price_per_whole_token(37 * PRICE_SCALE, 8), 37 * PRICE_SCALE);
    }
    use super::*;

    #[test]
    fn busd_is_canonical_before_fork_height() {
        let quotes =
            canonical_quotes_at_height(Network::Bitcoin, BUSD_CANONICAL_QUOTE_FORK_HEIGHT - 1);
        assert!(quotes.iter().any(|q| q.id == BUSD_ALKANE_ID));
        assert!(quotes.iter().any(|q| q.id == FRBTC_ALKANE_ID));
    }

    #[test]
    fn only_the_oyl_factory_prices_tokens_from_the_fork_height() {
        let h = MAINNET_PRICING_FACTORY_FORK_HEIGHT;
        // Before the height, and on other networks, nothing is restricted.
        assert!(pricing_factories_at_height(Network::Bitcoin, h - 1).is_none());
        assert!(pricing_factories_at_height(Network::Regtest, h + 1_000).is_none());

        let allowed = pricing_factories_at_height(Network::Bitcoin, h).expect("in force");
        assert_eq!(allowed, &[SchemaAlkaneId { block: 4, tx: 65522 }]);
        // The same contract the module already treats as the AMM.
        assert_eq!(allowed[0], get_amm_contract(Network::Bitcoin).unwrap());
        // The factory behind the duplicate dust pools is not on it.
        assert!(!allowed.contains(&SchemaAlkaneId { block: 4, tx: 235884 }));
    }

    #[test]
    fn no_derived_quote_fork_is_compiled_in() {
        // The TORTILLA/DIESEL detach is config (derive_sources.drop_forks), never
        // a constant: nothing detaches on any network unless configured.
        let tort = SchemaAlkaneId { block: 2, tx: 68479 };
        let diesel = SchemaAlkaneId { block: 2, tx: 0 };
        let h = MAINNET_TORTILLA_DIESEL_DETACH_HEIGHT;
        for height in [h - 1, h, h + 100_000] {
            assert!(!derived_quote_detached_at_height(Network::Bitcoin, &tort, &diesel, height));
        }
        assert!(MAINNET_DERIVED_QUOTE_FORKS.is_empty());
    }

    #[test]
    fn non_circulating_table_entries_all_list_holders() {
        for entry in MAINNET_NON_CIRCULATING {
            assert!(
                !entry.addresses.is_empty() || !entry.alkanes.is_empty(),
                "{}:{} lists no holders",
                entry.alkane.block,
                entry.alkane.tx
            );
            // A holder that is the token itself would make the ratio meaningless.
            assert!(!entry.alkanes.contains(&entry.alkane));
        }
    }

    #[test]
    fn non_circulating_lookup_is_mainnet_only() {
        let slice = SchemaAlkaneId { block: 4, tx: 8888 };
        let hit = non_circulating_holders(Network::Bitcoin, &slice).expect("mainnet entry");
        assert_eq!(hit.alkanes.len(), 4);
        assert_eq!(hit.addresses.len(), 1);
        assert!(non_circulating_holders(Network::Regtest, &slice).is_none());
        assert!(
            non_circulating_holders(Network::Bitcoin, &SchemaAlkaneId { block: 2, tx: 1 })
                .is_none()
        );
    }

    #[test]
    fn busd_is_not_canonical_at_fork_height() {
        let quotes = canonical_quotes_at_height(Network::Bitcoin, BUSD_CANONICAL_QUOTE_FORK_HEIGHT);
        assert!(!quotes.iter().any(|q| q.id == BUSD_ALKANE_ID));
        assert!(quotes.iter().any(|q| q.id == FRBTC_ALKANE_ID));
    }
}
