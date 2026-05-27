use borsh::{BorshDeserialize, BorshSerialize};

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SchemaWrapEventV1 {
    pub timestamp: u64,
    pub txid: [u8; 32],
    pub amount: u128,
    pub address_spk: Vec<u8>,
    pub success: bool,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SchemaUnwrapRequestV1 {
    pub timestamp: u64,
    pub txid: [u8; 32],
    pub vout: u32,
    pub amount: u128,
    pub address_spk: Vec<u8>,
    pub fulfillment_tx: Option<[u8; 32]>,
}

impl SchemaUnwrapRequestV1 {
    pub fn fulfilled(&self) -> bool {
        self.fulfillment_tx.is_some()
    }
}
