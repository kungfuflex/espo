use crate::schemas::SchemaAlkaneId;
use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::BTreeMap;

#[derive(BorshSerialize, BorshDeserialize, PartialEq, Debug, Clone, Copy)]
pub struct SchemaCandleV1 {
    pub open: u128,
    pub high: u128,
    pub low: u128,
    pub close: u128,
    pub volume: u128,
}

#[derive(BorshSerialize, BorshDeserialize, PartialEq, Debug, Clone, Copy)]
pub struct SchemaFullCandleV1 {
    pub base_candle: SchemaCandleV1,
    pub quote_candle: SchemaCandleV1,
}

#[derive(BorshSerialize, BorshDeserialize, PartialEq, Debug, Clone, Copy)]
pub struct SchemaCanonicalPoolEntry {
    pub pool_id: SchemaAlkaneId,
    pub quote_id: SchemaAlkaneId,
}

#[derive(BorshSerialize, BorshDeserialize, PartialEq, Debug, Clone, Default)]
pub struct SchemaTokenMetricsV1 {
    pub price_usd: u128,
    pub fdv_usd: u128,
    pub marketcap_usd: u128,
    pub volume_all_time: u128,
    pub volume_1d: u128,
    pub volume_7d: u128,
    pub volume_30d: u128,
    pub change_1d: String,
    pub change_7d: String,
    pub change_30d: String,
    pub change_all_time: String,
}

#[derive(BorshSerialize, BorshDeserialize, PartialEq, Debug, Clone, Default)]
pub struct SchemaPoolMetricsV1 {
    pub token0_volume_1d: u128,
    pub token1_volume_1d: u128,
    pub token0_volume_30d: u128,
    pub token1_volume_30d: u128,
    pub pool_volume_1d_usd: u128,
    pub pool_volume_30d_usd: u128,
    pub pool_volume_1d_sats: u128,
    pub pool_volume_30d_sats: u128,
    pub pool_tvl_usd: u128,
    pub pool_tvl_sats: u128,
    pub tvl_change_24h: String,
    pub tvl_change_7d: String,
    pub pool_apr: String,
}

#[derive(BorshSerialize, BorshDeserialize, PartialEq, Debug, Clone, Default)]
pub struct SchemaPoolMetricsV2 {
    pub token0_volume_1d: u128,
    pub token1_volume_1d: u128,
    pub token0_volume_30d: u128,
    pub token1_volume_30d: u128,
    pub pool_volume_1d_usd: u128,
    pub pool_volume_30d_usd: u128,
    pub pool_volume_1d_sats: u128,
    pub pool_volume_30d_sats: u128,
    pub pool_volume_7d_usd: u128,
    pub pool_volume_all_time_usd: u128,
    pub pool_volume_7d_sats: u128,
    pub pool_volume_all_time_sats: u128,
    pub pool_tvl_usd: u128,
    pub pool_tvl_sats: u128,
    pub tvl_change_24h: String,
    pub tvl_change_7d: String,
    pub pool_apr: String,
}

#[derive(BorshSerialize, BorshDeserialize, PartialEq, Debug, Clone, Default)]
pub struct SchemaPoolDetailsSnapshot {
    pub value_json: Vec<u8>,
    pub token0_tvl_usd: u128,
    pub token1_tvl_usd: u128,
    pub token0_tvl_sats: u128,
    pub token1_tvl_sats: u128,
    pub pool_tvl_usd: u128,
    pub pool_volume_1d_usd: u128,
    pub pool_volume_30d_usd: u128,
    pub pool_apr: f64,
    pub tvl_change_24h: f64,
    pub lp_supply: u128,
}

#[derive(BorshSerialize, BorshDeserialize, PartialEq, Debug, Clone, Default)]
pub struct SchemaPoolCreationInfoV1 {
    pub creator_spk: Vec<u8>,
    pub creation_height: u32,
    pub initial_token0_amount: u128,
    pub initial_token1_amount: u128,
    pub initial_lp_supply: u128,
}

#[derive(BorshSerialize, BorshDeserialize, PartialEq, Debug, Clone, Copy)]
pub struct SchemaMarketDefs {
    pub base_alkane_id: SchemaAlkaneId,
    pub quote_alkane_id: SchemaAlkaneId,
    pub pool_alkane_id: SchemaAlkaneId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum ActivityKind {
    TradeBuy,
    TradeSell,
    LiquidityAdd,
    LiquidityRemove,
    PoolCreate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum ActivityDirection {
    BaseIn,
    QuoteIn,
}

#[derive(Clone, BorshSerialize, BorshDeserialize, Debug)]
pub struct SchemaActivityV1 {
    pub timestamp: u64,
    pub txid: [u8; 32],
    pub kind: ActivityKind,
    pub direction: Option<ActivityDirection>,
    pub base_delta: i128,
    pub quote_delta: i128,
    pub address_spk: Vec<u8>,
    pub success: bool,
}

impl SchemaCandleV1 {
    pub fn from_price_and_vol(
        open: u128,
        high: u128,
        low: u128,
        close: u128,
        volume: u128,
    ) -> Self {
        Self { open, high, low, close, volume }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Timeframe {
    M10,
    H1,
    H4,
    D1,
    W1,
    M1,
}

impl Timeframe {
    #[inline]
    pub fn duration_secs(&self) -> u64 {
        match self {
            Timeframe::M10 => 10 * 60,
            Timeframe::H1 => 60 * 60,
            Timeframe::H4 => 4 * 60 * 60,
            Timeframe::D1 => 24 * 60 * 60,
            Timeframe::W1 => 7 * 24 * 60 * 60,
            Timeframe::M1 => 30 * 24 * 60 * 60, // simple month bucket (30d)
        }
    }
    /// Short ASCII code used in keys (keeps keys compact & lexicographically nice)
    #[inline]
    pub fn code(&self) -> &'static str {
        match self {
            Timeframe::M10 => "10m",
            Timeframe::H1 => "1h",
            Timeframe::H4 => "4h",
            Timeframe::D1 => "1d",
            Timeframe::W1 => "1w",
            Timeframe::M1 => "1M",
        }
    }
}
pub fn active_timeframes() -> Vec<Timeframe> {
    vec![Timeframe::M10, Timeframe::H1, Timeframe::H4, Timeframe::D1, Timeframe::W1, Timeframe::M1]
}
/// One entry per pool: latest reserves + the token IDs,
/// so callers never need to hit /pools to learn base/quote.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default)]
pub struct SchemaPoolSnapshot {
    pub base_reserve: u128,
    pub quote_reserve: u128,
    pub base_id: SchemaAlkaneId,
    pub quote_id: SchemaAlkaneId,
}

/// Entire “all pools” snapshot in a single key.
/// BTreeMap gives deterministic ordering for stable encoding.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default)]
pub struct SchemaReservesSnapshot {
    pub entries: BTreeMap<SchemaAlkaneId, SchemaPoolSnapshot>,
}
