/// Full integration test harness for ESPO
///
/// This module provides a unified test harness that combines:
/// - Bitcoin chain simulation (ChainBuilder, MockBitcoinNode)
/// - ESPO module indexing
/// - Reorg simulation
///
/// Based on the implementation plan from AMM_TEST_HARNESS_SUMMARY.md
///
/// Note: AMM-specific functionality is available via test_utils::amm_helpers
/// directly when the test-utils feature is enabled.
use anyhow::Result;
use bitcoin::Block;
use espo::config::AppConfig;
use espo::test_utils::{ChainBuilder, MockBitcoinNode, TestConfigBuilder};
use rocksdb::{DB, Options};
use std::sync::Arc;
use tempfile::TempDir;

/// Comprehensive test harness integrating ESPO components
///
/// This harness provides:
/// - Simulated Bitcoin blockchain with ChainBuilder
/// - MockBitcoinNode for block storage and retrieval
/// - Reorg simulation capabilities
#[allow(dead_code)]
pub struct EspoTestHarness {
    // Bitcoin simulation
    pub mock_node: MockBitcoinNode,

    // ESPO components
    pub config: AppConfig,
    pub db: Arc<DB>,

    // State tracking
    pub current_height: u32,
    blocks: Vec<Block>,

    // Resource cleanup
    _temp_dirs: Vec<TempDir>,
}

#[allow(dead_code)]
impl EspoTestHarness {
    /// Create a new test harness
    ///
    /// Initializes all components with temporary directories that are
    /// automatically cleaned up when the harness is dropped.
    pub fn new() -> Result<Self> {
        let (config, temp_dirs) =
            TestConfigBuilder::new().with_network(bitcoin::Network::Regtest).build();

        // Open the DB
        let mut opts = Options::default();
        opts.create_if_missing(true);
        let db = Arc::new(DB::open(&opts, &config.db_path)?);

        let mock_node = MockBitcoinNode::new();

        // Build initial chain (just genesis)
        let blocks = ChainBuilder::new().build();

        Ok(Self { mock_node, config, db, current_height: 0, blocks, _temp_dirs: temp_dirs })
    }

    /// Mine a specified number of empty blocks
    ///
    /// Generates blocks using ChainBuilder and adds them to MockBitcoinNode.
    pub fn mine_blocks(&mut self, count: u32) -> Result<Vec<Block>> {
        let start_height = self.current_height;

        // Build new chain with additional blocks
        let total_blocks = self.current_height + count;
        let new_chain = ChainBuilder::new().add_blocks(total_blocks).build();

        // Extract only the newly mined blocks
        let new_blocks: Vec<Block> = new_chain.iter().skip(self.blocks.len()).cloned().collect();

        // Update internal state
        self.blocks = new_chain;
        self.current_height += count;

        // Add new blocks to mock node
        for (i, block) in new_blocks.iter().enumerate() {
            let height = start_height + i as u32;
            self.mock_node.add_block(height, block.clone());
        }

        Ok(new_blocks)
    }

    /// Simulate a blockchain reorg
    ///
    /// Creates an alternative chain from `fork_height` with `new_blocks` blocks.
    pub fn simulate_reorg(&mut self, fork_height: u32, new_blocks: u32) -> Result<()> {
        // Build alternative chain
        let fork_chain = ChainBuilder::new()
            .add_blocks(fork_height)
            .fork(fork_height)
            .with_salt(1)
            .add_blocks(new_blocks)
            .build();

        // Apply to mock node
        self.mock_node.apply_reorg(fork_height, fork_chain.clone());

        // Update internal state
        self.blocks = fork_chain;
        self.current_height = fork_height + new_blocks - 1;

        Ok(())
    }

    /// Get the current tip height
    pub fn tip_height(&self) -> u32 {
        self.current_height
    }

    /// Get a reference to a block at a specific height
    pub fn get_block(&self, height: u32) -> Option<&Block> {
        self.blocks.get(height as usize)
    }

    /// Get a reference to the config
    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    /// Get a mutable reference to the mock node
    pub fn mock_node_mut(&mut self) -> &mut MockBitcoinNode {
        &mut self.mock_node
    }
}

impl Drop for EspoTestHarness {
    fn drop(&mut self) {
        // TempDirs will automatically clean up
    }
}

// Tests are in integration test files
