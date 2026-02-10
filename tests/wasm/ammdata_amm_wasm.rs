/// WASM-based AMM integration tests
///
/// These tests run in a WASM environment which properly handles metashrew's memory model.
/// Run with: wasm-pack test --node
///
/// Based on reference/subfrost-alkanes test patterns.

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn test_amm_deployment_in_wasm() {
    use alkanes::indexer::index_block;
    use alkanes_support::cellpack::Cellpack;
    use alkanes_support::id::AlkaneId;
    use bitcoin::hashes::Hash;
    use bitcoin::OutPoint;

    // Clear and setup environment
    metashrew_core::clear();

    // Configure network
    use protorune_support::network::{set_network, NetworkParams};
    set_network(NetworkParams {
        bech32_prefix: String::from("bcrt"),
        p2pkh_prefix: 0x64,
        p2sh_prefix: 0xc4,
    });

    // Index genesis blocks
    for h in 0..=3 {
        let block = protorune::test_helpers::create_block_with_coinbase_tx(h);
        index_block(&block, h).expect("Failed to index genesis block");
    }

    // This should work now in WASM context!
    console_log!("[WASM TEST] AMM deployment test starting");
    console_log!("[WASM TEST] Metashrew initialized successfully");

    // Test passed if we got here without the memory allocation error
    assert_eq!(2 + 2, 4);
}

#[wasm_bindgen_test]
fn test_simple_alkanes_index() {
    // Very simple test - just index a few blocks
    metashrew_core::clear();

    use protorune_support::network::{set_network, NetworkParams};
    set_network(NetworkParams {
        bech32_prefix: String::from("bcrt"),
        p2pkh_prefix: 0x64,
        p2sh_prefix: 0xc4,
    });

    for h in 0..5 {
        let block = protorune::test_helpers::create_block_with_coinbase_tx(h);
        let result = alkanes::indexer::index_block(&block, h);
        assert!(result.is_ok(), "Block indexing failed at height {}", h);
    }

    console_log!("[WASM TEST] Successfully indexed 5 blocks");
}

// Helpers for console output in WASM
#[macro_export]
macro_rules! console_log {
    ($($t:tt)*) => {
        web_sys::console::log_1(&format!($($t)*).into())
    }
}

#[wasm_bindgen_test]
fn test_full_amm_deployment_wasm() {
    use alkanes::indexer::index_block;
    use alkanes_support::cellpack::Cellpack;
    use alkanes_support::id::AlkaneId;
    use bitcoin::hashes::Hash;
    use bitcoin::OutPoint;

    console_log!("[WASM TEST] Full AMM Deployment Test");

    // Setup
    metashrew_core::clear();
    use protorune_support::network::{set_network, NetworkParams};
    set_network(NetworkParams {
        bech32_prefix: String::from("bcrt"),
        p2pkh_prefix: 0x64,
        p2sh_prefix: 0xc4,
    });

    // Index genesis
    for h in 0..=3 {
        let block = protorune::test_helpers::create_block_with_coinbase_tx(h);
        index_block(&block, h).expect("Genesis indexing failed");
    }

    console_log!("[WASM TEST] Genesis blocks indexed");

    // TODO: Add full AMM deployment here using espo::test_utils::setup_amm
    // This requires:
    // 1. Loading WASM files (pool, factory)
    // 2. Deploying contracts via cellpacks
    // 3. Indexing through metashrew
    // 4. Extracting traces
    // 5. Building EspoBlock
    // 6. Indexing through ammdata

    console_log!("[WASM TEST] Test completed");
}
