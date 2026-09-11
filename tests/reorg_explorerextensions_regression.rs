#![cfg(not(target_arch = "wasm32"))]

//! Regression test for the 2026-09-11 espo-e wedge.
//!
//! espo-e stopped indexing at height 966500 and stayed `Running` but unready
//! for 36 minutes, taking explorer.subfrost.io with it. The last log line was:
//!
//! ```text
//! [reorg] failed to switch indexer to height 966500: module explorerextensions
//!         still reports index height 966500 after reorg to next_height 966500
//! ```
//!
//! Every other module rolled back. `explorerextensions` implemented
//! `get_index_height` but not `handle_reorg`, so it inherited the trait's
//! silent `Ok(())` default, never refreshed the height it caches in an
//! `RwLock`, and tripped `handle_reorg_switch`'s (correct, strict)
//! verification pass forever.
//!
//! This test reproduces that end to end against the real module, the real
//! versioned tree and the real `handle_reorg_switch`. Revert
//! `ExplorerExtensions::handle_reorg` and it goes red with that exact message.
//!
//! Single `#[test]` on purpose: `init_global_tree_db` is a process-wide
//! `OnceLock` and the tree's active root is global mutable state, so the
//! scenario cannot be split across parallel tests in one binary. The
//! module-facing contract is covered separately, and in parallel, in
//! `tests/reorg_module_contract.rs`.

use bitcoin::Block;
use espo::alkanes::trace::EspoBlock;
use espo::modules::defs::EspoModule;
use espo::modules::explorerextensions::main::ExplorerExtensions;
use espo::modules::reorg::handle_reorg_switch;
use espo::runtime::mdb::Mdb;
use espo::runtime::tree_db::{get_global_tree_db, init_global_tree_db};
use espo::test_utils::ChainBuilder;
use rocksdb::{DB, Options};
use std::sync::Arc;
use tempfile::TempDir;

/// An `EspoBlock` with no transactions. `explorerextensions::index_block`
/// derives its rows from alkanes traces, so an empty block writes no rows —
/// but it still persists `/index_height` and updates the cached copy, which is
/// the only state this test is about.
fn empty_espo_block(height: u32, block: &Block) -> EspoBlock {
    EspoBlock {
        is_latest: true,
        height,
        block_header: block.header,
        host_function_values: Default::default(),
        fee_summary: None,
        tx_count: 0,
        transactions: Vec::new(),
    }
}

#[test]
fn explorerextensions_rolls_its_index_height_back_on_reorg() {
    let dir = TempDir::new().expect("tempdir");
    let mut opts = Options::default();
    opts.create_if_missing(true);
    let db = Arc::new(DB::open(&opts, dir.path()).expect("open rocksdb"));
    init_global_tree_db(Arc::clone(&db)).expect("init global tree db");
    let tree = get_global_tree_db().expect("global tree db");

    // `explorerextensions:` is one of the versioned namespaces, so its writes
    // land in the tree and are what a rewind discards.
    let mut module = ExplorerExtensions::new();
    module.set_mdb(Arc::new(Mdb::from_db(Arc::clone(&db), b"explorerextensions:")));
    let module: Arc<dyn EspoModule> = Arc::new(module);

    let chain = ChainBuilder::new().add_blocks(2).build();

    for height in 1..=2u32 {
        let block = &chain[height as usize];
        tree.begin_block(height, &block.block_hash(), &chain[height as usize - 1].block_hash())
            .expect("begin block");
        module.index_block(empty_espo_block(height, block)).expect("index block");
        tree.finish_block().expect("finish block");
    }

    assert_eq!(
        module.get_index_height(),
        Some(2),
        "precondition: the module must be at the tip before the reorg"
    );

    // Blocks 2 gets orphaned: the indexer switches back to next_height = 2,
    // i.e. "block 1 is the new tip, re-index from 2".
    let mods = vec![Arc::clone(&module)];
    handle_reorg_switch(&mods, 2).unwrap_or_else(|e| {
        panic!(
            "handle_reorg_switch wedged the indexer, exactly as it did on espo-e: {:#}\n\
             ExplorerExtensions::handle_reorg must re-read the index height from storage \
             after the versioned tree has been rewound.",
            e
        )
    });

    assert_eq!(
        module.get_index_height(),
        Some(1),
        "explorerextensions must report the rolled-back height, not its stale cached one"
    );

    // And the rollback is real, not just a cache poke: a fresh module reading
    // the same storage sees the same height.
    let mut reopened = ExplorerExtensions::new();
    reopened.set_mdb(Arc::new(Mdb::from_db(Arc::clone(&db), b"explorerextensions:")));
    assert_eq!(
        reopened.get_index_height(),
        Some(1),
        "the persisted index height must have been rewound too"
    );

    // The indexer can now make forward progress again — the thing espo-e could
    // not do until it was restarted by hand.
    let replacement = ChainBuilder::new().add_blocks(2).with_salt(7).fork(1).add_blocks(1).build();
    let new_block = &replacement[2];
    assert_ne!(new_block.block_hash(), chain[2].block_hash(), "replacement must be a real fork");
    tree.begin_block(2, &new_block.block_hash(), &chain[1].block_hash())
        .expect("begin replacement block");
    module
        .index_block(empty_espo_block(2, new_block))
        .expect("index replacement block");
    tree.finish_block().expect("finish replacement block");
    assert_eq!(module.get_index_height(), Some(2), "indexing must resume after the reorg");
}
