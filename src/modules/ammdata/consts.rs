use crate::schemas::SchemaAlkaneId;
use anyhow::{Result, anyhow};
use bitcoin::Network;

pub fn ammdata_genesis_block(network: Network) -> u32 {
    match network {
        Network::Bitcoin => 904_647,
        _ => 0,
    }
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

pub fn canonical_quotes(network: Network) -> Vec<CanonicalQuote> {
    let mainnet = vec![
        CanonicalQuote { id: SchemaAlkaneId { block: 32, tx: 0 }, unit: CanonicalQuoteUnit::Btc },
        CanonicalQuote {
            id: SchemaAlkaneId { block: 2, tx: 56801 },
            unit: CanonicalQuoteUnit::Usd,
        },
    ];
    match network {
        Network::Bitcoin => mainnet,
        _ => mainnet,
    }
}
