#![cfg(not(target_arch = "wasm32"))]

// Basic test harness demonstrating ESPO infrastructure testing
// This tests the components without requiring full metashrew runtime

mod common;

use espo::test_utils::{ChainBuilder, MockBitcoinNode, TestConfigBuilder};
use rocksdb::{DB, Options};
use std::sync::Arc;

#[test]
fn test_chain_builder_basic() {
    // Build a chain with blocks (add_blocks adds to genesis, so 10 -> 11 total)
    let blocks = ChainBuilder::new().add_blocks(10).build();

    assert_eq!(blocks.len(), 11); // genesis + 10 blocks

    // Each block should have a valid hash
    for block in &blocks {
        let _ = block.block_hash();
    }
}

#[test]
fn test_mock_node_basic() {
    let mut node = MockBitcoinNode::new();

    // Build initial chain (genesis + 20 = 21 blocks, heights 0-20)
    let blocks = ChainBuilder::new().add_blocks(20).build();

    node.set_chain(blocks.clone());

    // Verify tip height (21 blocks means heights 0-20, tip is 20)
    assert_eq!(node.get_tip_height(), 20);

    // Verify blocks are retrievable
    for (height, block) in blocks.iter().enumerate() {
        let retrieved = node.get_block_by_height(height as u32).unwrap();
        assert_eq!(retrieved.block_hash(), block.block_hash());
    }
}

#[test]
fn test_mock_node_reorg() {
    let mut node = MockBitcoinNode::new();

    // Build initial chain (genesis + 10 = 11 blocks, heights 0-10)
    let blocks = ChainBuilder::new().add_blocks(10).build();

    node.set_chain(blocks.clone());

    let old_tip_hash = node.get_tip_hash().unwrap();

    // Create alternative chain that forks at height 5
    // Build to height 5 (6 blocks), then fork back 1 block to height 4, then build 6 more
    let fork_blocks = ChainBuilder::new()
        .add_blocks(5) // Heights 0-5 (6 blocks)
        .fork(1) // Go back to height 4 (truncate to 5 blocks)
        .with_salt(1) // Different hashes for subsequent blocks
        .add_blocks(6) // Add 6 more blocks: heights 5-10
        .build();

    // Extract blocks from height 5 onwards (the forked portion)
    let new_blocks: Vec<_> = fork_blocks.into_iter().skip(5).collect();

    // Apply reorg at height 5
    node.apply_reorg(5, new_blocks.clone());

    // Tip should be updated
    assert_eq!(node.get_tip_height(), 5 + new_blocks.len() as u32 - 1);
    assert_ne!(node.get_tip_hash().unwrap(), old_tip_hash);

    // Blocks 0-4 should remain unchanged
    for i in 0..5 {
        let hash = node.get_block_hash(i).unwrap();
        assert_eq!(hash, blocks[i as usize].block_hash());
    }
}

#[test]
fn test_config_builder_basic() {
    let (config, _temp_dirs) = TestConfigBuilder::new()
        .with_network(bitcoin::Network::Regtest)
        .with_strict_mode(true)
        .build();

    assert_eq!(config.network, bitcoin::Network::Regtest);
    assert!(config.strict_mode.is_some());

    // Verify temp directories exist
    assert!(std::path::Path::new(&config.readonly_metashrew_db_dir).exists());
    assert!(std::path::Path::new(&config.db_path).exists());
}

#[test]
fn test_db_with_config() {
    let (config, _temp_dirs) = TestConfigBuilder::new().build();

    let test_path = std::path::Path::new(&config.db_path).join("test_db");
    let mut opts = Options::default();
    opts.create_if_missing(true);
    let db = Arc::new(DB::open(&opts, test_path).unwrap());

    // Basic DB operations
    db.put(b"key1", b"value1").unwrap();
    let value = db.get(b"key1").unwrap().unwrap();
    assert_eq!(value, b"value1");
}
