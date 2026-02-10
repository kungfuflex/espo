#![cfg(not(target_arch = "wasm32"))]

// Basic integration test demonstrating chain building and mock node usage

mod common;

use espo::test_utils::{ChainBuilder, MockBitcoinNode};

#[test]
fn test_build_simple_chain() {
    // Build a simple chain with 10 blocks
    let blocks = ChainBuilder::new().add_blocks(10).build();

    assert_eq!(blocks.len(), 11); // Genesis + 10 blocks

    // Verify chain is properly linked
    for i in 1..blocks.len() {
        assert_eq!(
            blocks[i].header.prev_blockhash,
            blocks[i - 1].block_hash(),
            "Block {} should link to block {}",
            i,
            i - 1
        );
    }
}

#[test]
fn test_mock_node_with_chain() {
    let mut node = MockBitcoinNode::new();

    // Build and load a chain
    let blocks = ChainBuilder::new().add_blocks(20).build();

    node.set_chain(blocks.clone());

    // Verify node state
    assert_eq!(node.get_tip_height(), 20);
    assert_eq!(node.block_count(), 21); // Genesis + 20

    // Verify we can retrieve blocks by height
    for (i, expected_block) in blocks.iter().enumerate() {
        let block = node
            .get_block_by_height(i as u32)
            .expect(&format!("Block at height {} should exist", i));

        assert_eq!(
            block.block_hash(),
            expected_block.block_hash(),
            "Block at height {} should match",
            i
        );
    }
}

#[test]
fn test_fork_and_reorg_simulation() {
    let mut node = MockBitcoinNode::new();

    // Build initial chain
    let initial_chain = ChainBuilder::new().add_blocks(10).build();

    node.set_chain(initial_chain.clone());
    let initial_tip = node.get_tip_hash().unwrap();

    // Create a fork at height 5
    let fork_chain = ChainBuilder::new()
        .add_blocks(5) // Build up to height 5
        .fork(1) // Go back to height 4
        .with_salt(1) // Use different salt for different hashes
        .add_blocks(8) // Build longer alternative chain
        .build();

    // Extract new blocks from height 5 onwards
    let new_blocks: Vec<_> = fork_chain.into_iter().skip(5).collect();

    // Apply reorg
    node.apply_reorg(5, new_blocks);

    // Verify reorg was applied
    assert_ne!(node.get_tip_hash().unwrap(), initial_tip);

    // Verify common blocks (0-4) remain unchanged
    for i in 0..5 {
        let hash = node.get_block_hash(i).unwrap();
        assert_eq!(
            hash,
            initial_chain[i as usize].block_hash(),
            "Block {} should remain unchanged after reorg",
            i
        );
    }
}
