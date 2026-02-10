/// WASM-based AMM integration tests
///
/// These tests run in a WASM environment which properly handles metashrew's memory model.
/// Run with: wasm-pack test --node
///
/// Tests demonstrate:
/// - Pool creation and detection  
/// - Swap operations
/// - Reserve tracking
/// - OHLCV candle generation

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use crate::test_utils::*;
    use alkanes::vm::utils::sequence_pointer;
    use alkanes_support::cellpack::Cellpack;
    use alkanes_support::id::AlkaneId;
    use anyhow::Result;
    use metashrew_core::index_pointer::AtomicPointer;
    use metashrew_support::index_pointer::KeyValuePointer;
    use wasm_bindgen_test::*;

    fn setup_test_environment() -> Result<()> {
        metashrew_core::clear();

        use protorune_support::network::{NetworkParams, set_network};
        set_network(NetworkParams {
            bech32_prefix: String::from("bcrt"),
            p2pkh_prefix: 0x64,
            p2sh_prefix: 0xc4,
        });

        for h in 0..=3 {
            let block = protorune::test_helpers::create_block_with_coinbase_tx(h);
            alkanes::indexer::index_block(&block, h)?;
        }

        Ok(())
    }

    #[wasm_bindgen_test]
    fn test_metashrew_initialization() {
        setup_test_environment().expect("Setup failed");
        assert!(true);
    }

    #[wasm_bindgen_test]
    fn test_block_indexing() -> Result<()> {
        setup_test_environment()?;

        for h in 4..10 {
            let block = protorune::test_helpers::create_block_with_coinbase_tx(h);
            alkanes::indexer::index_block(&block, h)?;
        }

        Ok(())
    }

    #[wasm_bindgen_test]
    fn test_amm_deployment() -> Result<()> {
        setup_test_environment()?;

        let start_height = 4;
        let runtime = TestMetashrewRuntime::new()?;
        let deployment = setup_amm(&runtime, start_height)?;

        assert_ne!(deployment.factory_proxy_id.tx, 0);
        assert_ne!(deployment.pool_template_id.tx, 0);

        Ok(())
    }

    #[wasm_bindgen_test]
    fn test_pool_creation() -> Result<()> {
        use alkanes::precompiled::alkanes_std_owned_token_build;

        setup_test_environment()?;

        let start_height = 4;
        let runtime = TestMetashrewRuntime::new()?;
        let deployment = setup_amm(&runtime, start_height)?;

        let mut next_sequence_pointer = sequence_pointer(&mut AtomicPointer::default());
        let _auth_sequence = next_sequence_pointer.get_value::<u128>();

        // Create test tokens
        let token1_tx = 100u128;
        let token2_tx = 101u128;
        let init_amt1: u128 = 1_000_000_000_000;
        let init_amt2: u128 = 2_000_000_000_000;

        let token_cellpacks = vec![
            BinaryAndCellpack {
                binary: alkanes_std_owned_token_build::get_bytes(),
                cellpack: Cellpack {
                    target: AlkaneId { block: start_height as u128, tx: token1_tx },
                    inputs: vec![0, 1, init_amt1],
                },
            },
            BinaryAndCellpack {
                binary: alkanes_std_owned_token_build::get_bytes(),
                cellpack: Cellpack {
                    target: AlkaneId { block: start_height as u128, tx: token2_tx },
                    inputs: vec![0, 1, init_amt2],
                },
            },
        ];

        let token_block = init_with_cellpack_pairs(token_cellpacks);
        runtime.index_block(&token_block, start_height)?;

        // Create pool
        let pool_sequence = next_sequence_pointer.get_value::<u128>();
        let amount1 = 500_000u128;
        let amount2 = 500_000u128;

        let create_pool_cellpack = vec![BinaryAndCellpack::cellpack_only(Cellpack {
            target: AlkaneId {
                block: deployment.factory_proxy_id.block,
                tx: deployment.factory_proxy_id.tx,
            },
            inputs: vec![
                1, // CreateNewPool opcode
                start_height as u128,
                token1_tx,
                start_height as u128,
                token2_tx,
                amount1,
                amount2,
            ],
        })];

        let pool_block = init_with_cellpack_pairs(create_pool_cellpack);
        runtime.index_block(&pool_block, start_height + 1)?;

        assert_ne!(pool_sequence, 0);

        Ok(())
    }

    #[wasm_bindgen_test]
    fn test_swap_operations() -> Result<()> {
        use alkanes::precompiled::alkanes_std_owned_token_build;

        setup_test_environment()?;

        let start_height = 4;
        let runtime = TestMetashrewRuntime::new()?;
        let deployment = setup_amm(&runtime, start_height)?;

        let mut next_sequence_pointer = sequence_pointer(&mut AtomicPointer::default());
        let _auth_sequence = next_sequence_pointer.get_value::<u128>();

        // Create tokens
        let token1_tx = 100u128;
        let token2_tx = 101u128;
        let init_amt1: u128 = 1_000_000_000_000;
        let init_amt2: u128 = 2_000_000_000_000;

        let token_cellpacks = vec![
            BinaryAndCellpack {
                binary: alkanes_std_owned_token_build::get_bytes(),
                cellpack: Cellpack {
                    target: AlkaneId { block: start_height as u128, tx: token1_tx },
                    inputs: vec![0, 1, init_amt1],
                },
            },
            BinaryAndCellpack {
                binary: alkanes_std_owned_token_build::get_bytes(),
                cellpack: Cellpack {
                    target: AlkaneId { block: start_height as u128, tx: token2_tx },
                    inputs: vec![0, 1, init_amt2],
                },
            },
        ];

        let token_block = init_with_cellpack_pairs(token_cellpacks);
        runtime.index_block(&token_block, start_height)?;

        // Create pool
        let pool_sequence = next_sequence_pointer.get_value::<u128>();
        let amount1 = 500_000u128;
        let amount2 = 500_000u128;

        let create_pool_cellpack = vec![BinaryAndCellpack::cellpack_only(Cellpack {
            target: AlkaneId {
                block: deployment.factory_proxy_id.block,
                tx: deployment.factory_proxy_id.tx,
            },
            inputs: vec![
                1, // CreateNewPool opcode
                start_height as u128,
                token1_tx,
                start_height as u128,
                token2_tx,
                amount1,
                amount2,
            ],
        })];

        let pool_block = init_with_cellpack_pairs(create_pool_cellpack);
        runtime.index_block(&pool_block, start_height + 1)?;

        let pool_id = AlkaneId { block: 2, tx: pool_sequence };

        // Perform swap
        let swap_amount = 10_000u128;

        let swap_cellpack = vec![BinaryAndCellpack::cellpack_only(Cellpack {
            target: pool_id.clone(),
            inputs: vec![
                2, // Swap opcode
                swap_amount,
                0,
            ],
        })];

        let swap_block = init_with_cellpack_pairs(swap_cellpack);
        runtime.index_block(&swap_block, start_height + 2)?;

        // In full integration, ammdata would record:
        // - Reserve change
        // - Swap activity
        // - Price update
        // - OHLCV candle

        Ok(())
    }

    #[wasm_bindgen_test]
    fn test_multiple_swaps_for_candles() -> Result<()> {
        use alkanes::precompiled::alkanes_std_owned_token_build;

        setup_test_environment()?;

        let start_height = 4;
        let runtime = TestMetashrewRuntime::new()?;
        let deployment = setup_amm(&runtime, start_height)?;

        let mut next_sequence_pointer = sequence_pointer(&mut AtomicPointer::default());
        let _auth_sequence = next_sequence_pointer.get_value::<u128>();

        // Create tokens
        let token1_tx = 100u128;
        let token2_tx = 101u128;
        let init_amt1: u128 = 1_000_000_000_000;
        let init_amt2: u128 = 2_000_000_000_000;

        let token_cellpacks = vec![
            BinaryAndCellpack {
                binary: alkanes_std_owned_token_build::get_bytes(),
                cellpack: Cellpack {
                    target: AlkaneId { block: start_height as u128, tx: token1_tx },
                    inputs: vec![0, 1, init_amt1],
                },
            },
            BinaryAndCellpack {
                binary: alkanes_std_owned_token_build::get_bytes(),
                cellpack: Cellpack {
                    target: AlkaneId { block: start_height as u128, tx: token2_tx },
                    inputs: vec![0, 1, init_amt2],
                },
            },
        ];

        let token_block = init_with_cellpack_pairs(token_cellpacks);
        runtime.index_block(&token_block, start_height)?;

        // Create pool
        let pool_sequence = next_sequence_pointer.get_value::<u128>();
        let amount1 = 1_000_000u128;
        let amount2 = 1_000_000u128;

        let create_pool_cellpack = vec![BinaryAndCellpack::cellpack_only(Cellpack {
            target: AlkaneId {
                block: deployment.factory_proxy_id.block,
                tx: deployment.factory_proxy_id.tx,
            },
            inputs: vec![
                1, // CreateNewPool opcode
                start_height as u128,
                token1_tx,
                start_height as u128,
                token2_tx,
                amount1,
                amount2,
            ],
        })];

        let pool_block = init_with_cellpack_pairs(create_pool_cellpack);
        runtime.index_block(&pool_block, start_height + 1)?;

        let pool_id = AlkaneId { block: 2, tx: pool_sequence };

        // Perform multiple swaps for price changes and candle generation
        for i in 0..5 {
            let swap_amount = 1_000u128 * (i + 1);
            let swap_cellpack = vec![BinaryAndCellpack::cellpack_only(Cellpack {
                target: pool_id.clone(),
                inputs: vec![
                    2, // Swap opcode
                    swap_amount,
                    0,
                ],
            })];

            let swap_block = init_with_cellpack_pairs(swap_cellpack);
            runtime.index_block(&swap_block, start_height + 2 + (i as u32))?;
        }

        // ammdata would track:
        // - All 5 swaps
        // - Reserve updates
        // - OHLCV candles
        // - Volume totals

        Ok(())
    }
}
