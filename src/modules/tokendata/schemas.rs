use crate::schemas::SchemaAlkaneId;
use borsh::{BorshDeserialize, BorshSerialize};

#[derive(
    BorshSerialize, BorshDeserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub enum TokenActivityKind {
    Buy,
    Sell,
    LiquidityAdd,
    LiquidityRemove,
    PoolCreate,
    Mint,
}

#[derive(
    BorshSerialize, BorshDeserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub enum TokenActivitySource {
    Market,
    Mint,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct SchemaTokenActivityV1 {
    pub timestamp: u64,
    pub txid: [u8; 32],
    pub token: SchemaAlkaneId,
    pub kind: TokenActivityKind,
    pub source: TokenActivitySource,
    pub pool: Option<SchemaAlkaneId>,
    pub counter_token: Option<SchemaAlkaneId>,
    pub token_delta: i128,
    pub counter_delta: i128,
    pub address_spk: Vec<u8>,
    pub success: bool,
}
