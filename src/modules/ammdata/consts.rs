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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busd_is_canonical_before_fork_height() {
        let quotes =
            canonical_quotes_at_height(Network::Bitcoin, BUSD_CANONICAL_QUOTE_FORK_HEIGHT - 1);
        assert!(quotes.iter().any(|q| q.id == BUSD_ALKANE_ID));
        assert!(quotes.iter().any(|q| q.id == FRBTC_ALKANE_ID));
    }

    #[test]
    fn busd_is_not_canonical_at_fork_height() {
        let quotes = canonical_quotes_at_height(Network::Bitcoin, BUSD_CANONICAL_QUOTE_FORK_HEIGHT);
        assert!(!quotes.iter().any(|q| q.id == BUSD_ALKANE_ID));
        assert!(quotes.iter().any(|q| q.id == FRBTC_ALKANE_ID));
    }
}
