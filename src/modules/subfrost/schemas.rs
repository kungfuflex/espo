use borsh::{BorshDeserialize, BorshSerialize};

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SchemaWrapEventV1 {
    pub timestamp: u64,
    pub txid: [u8; 32],
    pub amount: u128,
    pub address_spk: Vec<u8>,
    pub success: bool,
}
