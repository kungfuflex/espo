use bitcoin::{Block, BlockHash};
use std::collections::HashMap;

/// Mock Bitcoin node for testing
/// Simulates a Bitcoin node by storing blocks and providing query methods
pub struct MockBitcoinNode {
    blocks: HashMap<BlockHash, Block>,
    height_to_hash: HashMap<u32, BlockHash>,
    hash_to_height: HashMap<BlockHash, u32>,
    tip: u32,
}

impl MockBitcoinNode {
    /// Create a new empty mock node
    pub fn new() -> Self {
        Self {
            blocks: HashMap::new(),
            height_to_hash: HashMap::new(),
            hash_to_height: HashMap::new(),
            tip: 0,
        }
    }

    /// Add a single block at a specific height
    pub fn add_block(&mut self, height: u32, block: Block) {
        let hash = block.block_hash();

        self.blocks.insert(hash, block);
        self.height_to_hash.insert(height, hash);
        self.hash_to_height.insert(hash, height);

        if height > self.tip {
            self.tip = height;
        }
    }

    /// Load a chain of blocks starting from height 0
    pub fn set_chain(&mut self, blocks: Vec<Block>) {
        self.blocks.clear();
        self.height_to_hash.clear();
        self.hash_to_height.clear();

        for (height, block) in blocks.into_iter().enumerate() {
            self.add_block(height as u32, block);
        }
    }

    /// Get block hash at a specific height
    pub fn get_block_hash(&self, height: u32) -> Option<BlockHash> {
        self.height_to_hash.get(&height).copied()
    }

    /// Get block by hash
    pub fn get_block(&self, hash: &BlockHash) -> Option<&Block> {
        self.blocks.get(hash)
    }

    /// Get block by height
    pub fn get_block_by_height(&self, height: u32) -> Option<&Block> {
        self.get_block_hash(height).and_then(|hash| self.get_block(&hash))
    }

    /// Get height of a block by hash
    pub fn get_height(&self, hash: &BlockHash) -> Option<u32> {
        self.hash_to_height.get(hash).copied()
    }

    /// Get the current tip height
    pub fn get_tip_height(&self) -> u32 {
        self.tip
    }

    /// Get the tip block hash
    pub fn get_tip_hash(&self) -> Option<BlockHash> {
        self.get_block_hash(self.tip)
    }

    /// Get the tip block
    pub fn get_tip_block(&self) -> Option<&Block> {
        self.get_block_by_height(self.tip)
    }

    /// Simulate a reorg by replacing blocks from fork_height onwards
    pub fn apply_reorg(&mut self, fork_height: u32, new_blocks: Vec<Block>) {
        // Remove blocks from fork_height onwards
        let heights_to_remove: Vec<u32> =
            self.height_to_hash.keys().filter(|&&h| h >= fork_height).copied().collect();

        for height in heights_to_remove {
            if let Some(hash) = self.height_to_hash.remove(&height) {
                self.blocks.remove(&hash);
                self.hash_to_height.remove(&hash);
            }
        }

        // Add new blocks starting from fork_height
        for (i, block) in new_blocks.into_iter().enumerate() {
            let height = fork_height + i as u32;
            self.add_block(height, block);
        }
    }

    /// Get the number of blocks in the node
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Check if a block exists by hash
    pub fn has_block(&self, hash: &BlockHash) -> bool {
        self.blocks.contains_key(hash)
    }

    /// Clear all blocks
    pub fn clear(&mut self) {
        self.blocks.clear();
        self.height_to_hash.clear();
        self.hash_to_height.clear();
        self.tip = 0;
    }
}

impl Default for MockBitcoinNode {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::ChainBuilder;

    #[test]
    fn test_mock_node_basic() {
        let mut node = MockBitcoinNode::new();
        let blocks = ChainBuilder::new().add_blocks(10).build();

        node.set_chain(blocks.clone());

        assert_eq!(node.block_count(), 11); // Genesis + 10
        assert_eq!(node.get_tip_height(), 10);

        // Verify we can retrieve blocks by height
        for (i, expected_block) in blocks.iter().enumerate() {
            let block = node.get_block_by_height(i as u32).unwrap();
            assert_eq!(block.block_hash(), expected_block.block_hash());
        }
    }

    #[test]
    fn test_mock_node_reorg() {
        let mut node = MockBitcoinNode::new();

        // Set up initial chain (11 blocks: 0-10)
        let initial_chain = ChainBuilder::new().add_blocks(10).build();
        node.set_chain(initial_chain.clone());

        let old_tip_hash = node.get_tip_hash().unwrap();

        // Create alternative chain that forks at height 5
        // Build up to height 5, then fork back 1 block to height 4, then build 6 more
        let fork_blocks = ChainBuilder::new()
            .add_blocks(5) // Heights 0-5 (6 blocks)
            .fork(1) // Go back to height 4
            .with_salt(1) // Different hashes
            .add_blocks(6) // Add 6 more blocks: heights 5-10
            .build();

        // Extract blocks from height 5 onwards
        let new_blocks: Vec<Block> = fork_blocks.into_iter().skip(5).collect();

        // Apply reorg at height 5
        node.apply_reorg(5, new_blocks.clone());

        // Tip should be at height 5 + (number of new blocks - 1)
        assert_eq!(node.get_tip_height(), 5 + new_blocks.len() as u32 - 1);
        assert_ne!(node.get_tip_hash().unwrap(), old_tip_hash);

        // Blocks 0-4 should remain unchanged
        for i in 0..5 {
            let hash = node.get_block_hash(i).unwrap();
            assert_eq!(hash, initial_chain[i as usize].block_hash());
        }
    }

    #[test]
    fn test_block_lookup() {
        let mut node = MockBitcoinNode::new();
        let blocks = ChainBuilder::new().add_blocks(5).build();

        node.set_chain(blocks.clone());

        // Test hash to height lookup
        for (i, block) in blocks.iter().enumerate() {
            let hash = block.block_hash();
            assert_eq!(node.get_height(&hash), Some(i as u32));
        }

        // Test height to hash lookup
        for i in 0..=5 {
            let hash = node.get_block_hash(i).unwrap();
            assert!(node.has_block(&hash));
        }
    }
}
