use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotTokenStatus {
    Bonding,
    Migrating,
    Bonded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct BondedSnapshotMetaV1 {
    pub root_hash: [u8; 32],
    pub height: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct PizzafunChainMetadataV1 {
    pub series_id: String,
    pub icon_url: String,
    pub name: String,
    pub description: String,
    pub protocol_id: String,
    pub timestamp: u64,
    pub symbol: String,
    pub telegram_link: Option<String>,
    pub x_link: Option<String>,
    pub website_link: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct BondedSnapshotRowV1 {
    pub metaprotocol: String,
    pub series_id: String,
    pub protocol_id: String,
    pub created_at: u64,
    pub name: String,
    pub symbol: String,
    pub description: String,
    pub icon_url: String,
    pub status: SnapshotTokenStatus,
    pub last_traded_at: u64,
    pub price_usd: u128,
    pub market_cap_usd: u128,
    pub volume_1d: u128,
    pub volume_7d: u128,
    pub volume_30d: u128,
    pub volume_all_time: u128,
    pub change_1d_bps: i64,
    pub change_7d_bps: i64,
    pub change_30d_bps: i64,
    pub change_all_time_bps: i64,
    pub holders: u64,
    pub telegram_link: Option<String>,
    pub x_link: Option<String>,
    pub website_link: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct BondedSnapshotPageV1 {
    pub root_hash: [u8; 32],
    pub offset: u64,
    pub limit: u64,
    pub total: u64,
    pub entries: Vec<BondedSnapshotRowV1>,
}
