use bitcoin::script::ScriptBuf;
use bitcoin::{
    Block, BlockHash, CompactTarget, Transaction, TxIn, TxMerkleNode, TxOut, Witness,
    block::{Header, Version},
    hashes::{Hash, sha256d},
};

/// Builder for creating test blockchain sequences
pub struct ChainBuilder {
    blocks: Vec<Block>,
    salt: u64,
}

impl ChainBuilder {
    /// Create a new chain builder starting from a regtest genesis block
    pub fn new() -> Self {
        let genesis = Self::create_regtest_genesis();

        Self { blocks: vec![genesis], salt: 0 }
    }

    /// Add a specified number of blocks to the chain
    pub fn add_blocks(mut self, count: u32) -> Self {
        for _ in 0..count {
            let next_block = self.create_next_block();
            self.blocks.push(next_block);
        }
        self
    }

    /// Fork from a specific height (count blocks back from current tip)
    /// Creates an alternative chain from that ancestor
    pub fn fork(mut self, back: u32) -> Self {
        let fork_point = self.blocks.len().saturating_sub(back as usize + 1);
        self.blocks.truncate(fork_point + 1);
        self
    }

    /// Change the salt to generate different block hashes (useful for reorg testing)
    pub fn with_salt(mut self, salt: u64) -> Self {
        self.salt = salt;
        self
    }

    /// Add a custom block to the chain
    pub fn add_custom_block(mut self, block: Block) -> Self {
        self.blocks.push(block);
        self
    }

    /// Build and return the chain of blocks
    pub fn build(self) -> Vec<Block> {
        self.blocks
    }

    /// Get the current tip height
    pub fn height(&self) -> u32 {
        self.blocks.len().saturating_sub(1) as u32
    }

    /// Create a regtest genesis block
    fn create_regtest_genesis() -> Block {
        // Regtest genesis block
        let coinbase_tx = Self::create_coinbase_tx(0, &[]);
        let merkle_root = Self::compute_merkle_root(&[coinbase_tx.clone()]);

        let header = Header {
            version: Version::from_consensus(1),
            prev_blockhash: BlockHash::all_zeros(),
            merkle_root,
            time: 1296688602,
            bits: CompactTarget::from_consensus(0x207fffff),
            nonce: 2,
        };

        Block { header, txdata: vec![coinbase_tx] }
    }

    /// Create the next block in the chain
    fn create_next_block(&self) -> Block {
        let prev_block = self.blocks.last().expect("chain should not be empty");
        let prev_hash = prev_block.block_hash();
        let height = self.blocks.len() as u32;

        // Create coinbase transaction
        let coinbase_tx = Self::create_coinbase_tx(height, &self.salt.to_le_bytes());

        // Compute merkle root
        let merkle_root = Self::compute_merkle_root(&[coinbase_tx.clone()]);

        let header = Header {
            version: Version::from_consensus(1),
            prev_blockhash: prev_hash,
            merkle_root,
            time: prev_block.header.time + 600, // 10 minutes per block
            bits: CompactTarget::from_consensus(0x207fffff), // Regtest difficulty
            nonce: height + self.salt as u32,
        };

        Block { header, txdata: vec![coinbase_tx] }
    }

    /// Create a coinbase transaction
    fn create_coinbase_tx(height: u32, extra_data: &[u8]) -> Transaction {
        // Create height script (BIP34)
        let mut height_script = vec![0x03]; // OP_3 (push 3 bytes)
        height_script.extend_from_slice(&height.to_le_bytes()[..3]);
        height_script.extend_from_slice(extra_data);

        let coinbase_input = TxIn {
            previous_output: bitcoin::OutPoint::null(),
            script_sig: ScriptBuf::from_bytes(height_script),
            sequence: bitcoin::Sequence::MAX,
            witness: Witness::new(),
        };

        let coinbase_output = TxOut {
            value: bitcoin::Amount::from_sat(50 * 100_000_000), // 50 BTC reward
            script_pubkey: ScriptBuf::new(),
        };

        Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::locktime::absolute::LockTime::ZERO,
            input: vec![coinbase_input],
            output: vec![coinbase_output],
        }
    }

    /// Compute merkle root from transactions
    fn compute_merkle_root(txs: &[Transaction]) -> TxMerkleNode {
        if txs.is_empty() {
            return TxMerkleNode::all_zeros();
        }

        let mut hashes: Vec<TxMerkleNode> = txs
            .iter()
            .map(|tx| {
                let txid = tx.compute_txid();
                TxMerkleNode::from_byte_array(txid.to_byte_array())
            })
            .collect();

        while hashes.len() > 1 {
            let mut next_level = Vec::new();

            for chunk in hashes.chunks(2) {
                let hash = if chunk.len() == 2 {
                    Self::hash_pair(chunk[0], chunk[1])
                } else {
                    // Duplicate last hash if odd number
                    Self::hash_pair(chunk[0], chunk[0])
                };
                next_level.push(hash);
            }

            hashes = next_level;
        }

        hashes[0]
    }

    /// Hash a pair of merkle nodes for merkle tree computation
    fn hash_pair(a: TxMerkleNode, b: TxMerkleNode) -> TxMerkleNode {
        let mut data = Vec::with_capacity(64);
        data.extend_from_slice(&a.to_byte_array());
        data.extend_from_slice(&b.to_byte_array());
        TxMerkleNode::from_slice(&sha256d::Hash::hash(&data).to_byte_array()).unwrap()
    }
}

impl Default for ChainBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chain_builder_basic() {
        let chain = ChainBuilder::new().add_blocks(10).build();

        assert_eq!(chain.len(), 11); // Genesis + 10 blocks

        // Verify chain linkage
        for i in 1..chain.len() {
            assert_eq!(chain[i].header.prev_blockhash, chain[i - 1].block_hash());
        }
    }

    #[test]
    fn test_chain_fork() {
        let main_chain = ChainBuilder::new().add_blocks(10).build();

        let fork_chain = ChainBuilder::new()
            .add_blocks(10)
            .fork(5) // Go back 5 blocks
            .with_salt(1) // Different hashes
            .add_blocks(7) // Build alternative chain
            .build();

        // Fork chain should have same blocks up to fork point
        for i in 0..=5 {
            assert_eq!(main_chain[i].block_hash(), fork_chain[i].block_hash());
        }

        // After fork point, blocks should be different
        if fork_chain.len() > 6 {
            assert_ne!(main_chain[6].block_hash(), fork_chain[6].block_hash());
        }
    }

    #[test]
    fn test_genesis_block() {
        let chain = ChainBuilder::new().build();
        assert_eq!(chain.len(), 1);

        let genesis = &chain[0];
        assert_eq!(genesis.header.prev_blockhash, BlockHash::all_zeros());
        assert_eq!(genesis.txdata.len(), 1); // Coinbase
    }
}
