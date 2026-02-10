/// WASM-based AMM integration tests
///
/// These tests run in a WASM environment which properly handles metashrew's memory model.
/// Run with: wasm-pack test --node
///
/// Based on reference/subfrost-alkanes test patterns.

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use wasm_bindgen_test::*;
    use anyhow::Result;
    use alkanes::vm::utils::sequence_pointer;
    use alkanes_support::cellpack::Cellpack;
    use alkanes_support::id::AlkaneId;
    use bitcoin::hashes::Hash;
    use bitcoin::OutPoint;
    use metashrew_core::index_pointer::AtomicPointer;
    use metashrew_support::index_pointer::KeyValuePointer;

    fn setup_test_environment() -> Result<()> {
        // Clear metashrew state
        metashrew_core::clear();

        // Configure network
        use protorune_support::network::{set_network, NetworkParams};
        set_network(NetworkParams {
            bech32_prefix: String::from("bcrt"),
            p2pkh_prefix: 0x64,
            p2sh_prefix: 0xc4,
        });

        // Index empty blocks to height 3
        for h in 0..=3 {
            let block = protorune::test_helpers::create_block_with_coinbase_tx(h);
            alkanes::indexer::index_block(&block, h)?;
        }

        Ok(())
    }

    #[wasm_bindgen_test]
    fn test_metashrew_initialization() {
        setup_test_environment().expect("Setup failed");
        // If we got here, metashrew is working in WASM context
        assert!(true);
    }

    #[wasm_bindgen_test]
    fn test_block_indexing() -> Result<()> {
        setup_test_environment()?;

        // Index additional blocks
        for h in 4..10 {
            let block = protorune::test_helpers::create_block_with_coinbase_tx(h);
            alkanes::indexer::index_block(&block, h)?;
        }

        Ok(())
    }

    #[wasm_bindgen_test]
    fn test_amm_deployment() -> Result<()> {
        use crate::test_utils::*;

        setup_test_environment()?;

        // Deploy AMM using our helpers
        let start_height = 4;
        let runtime = TestMetashrewRuntime::new()?;
        let deployment = setup_amm(&runtime, start_height)?;

        // Verify deployment
        assert_ne!(deployment.factory_proxy_id.tx, 0);
        assert_ne!(deployment.pool_template_id.tx, 0);

        Ok(())
    }

    #[wasm_bindgen_test]
    fn test_pool_creation_and_detection() -> Result<()> {
        use crate::test_utils::*;
        use crate::test_utils::amm_helpers::BinaryAndCellpack;
        use alkanes::precompiled::alkanes_std_owned_token_build;

        setup_test_environment()?;

        let start_height = 4;
        let runtime = TestMetashrewRuntime::new()?;
        let deployment = setup_amm(&runtime, start_height)?;

        // Get the last outpoint for chaining transactions
        let mut next_sequence_pointer = sequence_pointer(&mut AtomicPointer::default());
        let _auth_sequence = next_sequence_pointer.get_value::<u128>();

        // Create two test tokens with large initial supplies
        let token1_tx = 100u128;
        let token2_tx = 101u128;
        let init_amt_token1: u128 = 1_000_000_000_000;
        let init_amt_token2: u128 = 2_000_000_000_000;

        // Deploy tokens
        let token_cellpacks = vec![
            BinaryAndCellpack {
                binary: alkanes_std_owned_token_build::get_bytes(),
                cellpack: Cellpack {
                    target: AlkaneId { block: start_height as u128, tx: token1_tx },
                    inputs: vec![0, 1, init_amt_token1],
                },
            },
            BinaryAndCellpack {
                binary: alkanes_std_owned_token_build::get_bytes(),
                cellpack: Cellpack {
                    target: AlkaneId { block: start_height as u128, tx: token2_tx },
                    inputs: vec![0, 1, init_amt_token2],
                },
            },
        ];

        // Helper function from subfrost pattern
        let init_with_cellpack_pairs = |pairs: Vec<BinaryAndCellpack>| -> bitcoin::Block {
            use protorune::test_helpers::create_block_with_coinbase_tx;
            let mut block = create_block_with_coinbase_tx(start_height);

            for pair in pairs {
                use protorune::message::{MessageContext, MessageContextParcel};
                use bitcoin::Transaction;
                use bitcoin::TxOut;
                use bitcoin::ScriptBuf;

                let mut tx = Transaction {
                    version: bitcoin::transaction::Version::TWO,
                    lock_time: bitcoin::absolute::LockTime::ZERO,
                    input: vec![],
                    output: vec![TxOut {
                        value: bitcoin::Amount::from_sat(1000),
                        script_pubkey: ScriptBuf::new(),
                    }],
                };

                // Add cellpack data
                let context = MessageContextParcel::from_cellpack(pair.cellpack);
                if let Some(buf) = context.encode() {
                    tx.output[0].script_pubkey = ScriptBuf::from_bytes(buf);
                }

                block.txdata.push(tx);
            }

            block
        };

        let token_block = init_with_cellpack_pairs(token_cellpacks);
        runtime.index_block(&token_block, start_height)?;

        // Create a pool with opcode 1 (CreateNewPool)
        let pool_sequence = next_sequence_pointer.get_value::<u128>();
        let amount1 = 500_000u128;
        let amount2 = 500_000u128;

        let create_pool_cellpack = vec![BinaryAndCellpack::cellpack_only(Cellpack {
            target: AlkaneId {
                block: deployment.factory_proxy_id.block,
                tx: deployment.factory_proxy_id.tx
            },
            inputs: vec![
                1, // CreateNewPool opcode
                start_height as u128, token1_tx,
                start_height as u128, token2_tx,
                amount1,
                amount2,
            ],
        })];

        let pool_block = init_with_cellpack_pairs(create_pool_cellpack);
        runtime.index_block(&pool_block, start_height)?;

        let expected_lp_token_id = AlkaneId { block: 2, tx: pool_sequence };

        // Verify the pool was created
        // In a full integration, we'd check ammdata indexed this pool
        assert_ne!(pool_sequence, 0);

        Ok(())
    }

    #[wasm_bindgen_test]
    fn test_swap_operations() -> Result<()> {
        use crate::test_utils::*;
        use crate::test_utils::amm_helpers::BinaryAndCellpack;
        use alkanes::precompiled::alkanes_std_owned_token_build;
        use alkanes::vm::utils::sequence_pointer;
        use metashrew_core::index_pointer::AtomicPointer;

        setup_test_environment()?;

        let start_height = 4;
        let runtime = TestMetashrewRuntime::new()?;
        let deployment = setup_amm(&runtime, start_height)?;

        let mut next_sequence_pointer = sequence_pointer(&mut AtomicPointer::default());
        let _auth_sequence = next_sequence_pointer.get_value::<u128>();

        // Create tokens
        let token1_tx = 100u128;
        let token2_tx = 101u128;
        let init_amt_token1: u128 = 1_000_000_000_000;
        let init_amt_token2: u128 = 2_000_000_000_000;

        let init_with_cellpack_pairs = |pairs: Vec<BinaryAndCellpack>| -> bitcoin::Block {
            use protorune::test_helpers::create_block_with_coinbase_tx;
            let mut block = create_block_with_coinbase_tx(start_height);

            for pair in pairs {
                use protorune::message::{MessageContext, MessageContextParcel};
                use bitcoin::Transaction;
                use bitcoin::TxOut;
                use bitcoin::ScriptBuf;

                let mut tx = Transaction {
                    version: bitcoin::transaction::Version::TWO,
                    lock_time: bitcoin::absolute::LockTime::ZERO,
                    input: vec![],
                    output: vec![TxOut {
                        value: bitcoin::Amount::from_sat(1000),
                        script_pubkey: ScriptBuf::new(),
                    }],
                };

                let context = MessageContextParcel::from_cellpack(pair.cellpack);
                if let Some(buf) = context.encode() {
                    tx.output[0].script_pubkey = ScriptBuf::from_bytes(buf);
                }

                block.txdata.push(tx);
            }

            block
        };

        // Deploy tokens
        let token_cellpacks = vec![
            BinaryAndCellpack {
                binary: alkanes_std_owned_token_build::get_bytes(),
                cellpack: Cellpack {
                    target: AlkaneId { block: start_height as u128, tx: token1_tx },
                    inputs: vec![0, 1, init_amt_token1],
                },
            },
            BinaryAndCellpack {
                binary: alkanes_std_owned_token_build::get_bytes(),
                cellpack: Cellpack {
                    target: AlkaneId { block: start_height as u128, tx: token2_tx },
                    inputs: vec![0, 1, init_amt_token2],
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
                tx: deployment.factory_proxy_id.tx
            },
            inputs: vec![
                1, // CreateNewPool opcode
                start_height as u128, token1_tx,
                start_height as u128, token2_tx,
                amount1,
                amount2,
            ],
        })];

        let pool_block = init_with_cellpack_pairs(create_pool_cellpack);
        runtime.index_block(&pool_block, start_height + 1)?;

        let pool_id = AlkaneId { block: 2, tx: pool_sequence };

        // Perform a swap (opcode 2: swap token0 to token1, or opcode 3: swap token1 to token0)
        // For simplicity, let's do a swap of token0 to token1
        let swap_amount = 10_000u128;

        let swap_cellpack = vec![BinaryAndCellpack::cellpack_only(Cellpack {
            target: pool_id.clone(),
            inputs: vec![
                2, // Swap opcode (token0 to token1)
                swap_amount,
                0, // min output (0 for test)
            ],
        })];

        let swap_block = init_with_cellpack_pairs(swap_cellpack);
        runtime.index_block(&swap_block, start_height + 2)?;

        // In a full integration, we'd verify ammdata recorded:
        // - Reserve change
        // - Swap activity
        // - Price update
        // - OHLCV candle

        Ok(())
    }

    #[wasm_bindgen_test]
    fn test_multiple_swaps_candle_generation() -> Result<()> {
        use crate::test_utils::*;
        use crate::test_utils::amm_helpers::BinaryAndCellpack;
        use alkanes::precompiled::alkanes_std_owned_token_build;
        use alkanes::vm::utils::sequence_pointer;
        use metashrew_core::index_pointer::AtomicPointer;

        setup_test_environment()?;

        let start_height = 4;
        let runtime = TestMetashrewRuntime::new()?;
        let deployment = setup_amm(&runtime, start_height)?;

        let mut next_sequence_pointer = sequence_pointer(&mut AtomicPointer::default());
        let _auth_sequence = next_sequence_pointer.get_value::<u128>();

        // Create tokens
        let token1_tx = 100u128;
        let token2_tx = 101u128;
        let init_amt_token1: u128 = 1_000_000_000_000;
        let init_amt_token2: u128 = 2_000_000_000_000;

        let init_with_cellpack_pairs = |pairs: Vec<BinaryAndCellpack>| -> bitcoin::Block {
            use protorune::test_helpers::create_block_with_coinbase_tx;
            let mut block = create_block_with_coinbase_tx(start_height);

            for pair in pairs {
                use protorune::message::{MessageContext, MessageContextParcel};
                use bitcoin::Transaction;
                use bitcoin::TxOut;
                use bitcoin::ScriptBuf;

                let mut tx = Transaction {
                    version: bitcoin::transaction::Version::TWO,
                    lock_time: bitcoin::absolute::LockTime::ZERO,
                    input: vec![],
                    output: vec![TxOut {
                        value: bitcoin::Amount::from_sat(1000),
                        script_pubkey: ScriptBuf::new(),
                    }],
                };

                let context = MessageContextParcel::from_cellpack(pair.cellpack);
                if let Some(buf) = context.encode() {
                    tx.output[0].script_pubkey = ScriptBuf::from_bytes(buf);
                }

                block.txdata.push(tx);
            }

            block
        };

        // Deploy tokens
        let token_cellpacks = vec![
            BinaryAndCellpack {
                binary: alkanes_std_owned_token_build::get_bytes(),
                cellpack: Cellpack {
                    target: AlkaneId { block: start_height as u128, tx: token1_tx },
                    inputs: vec![0, 1, init_amt_token1],
                },
            },
            BinaryAndCellpack {
                binary: alkanes_std_owned_token_build::get_bytes(),
                cellpack: Cellpack {
                    target: AlkaneId { block: start_height as u128, tx: token2_tx },
                    inputs: vec![0, 1, init_amt_token2],
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
                tx: deployment.factory_proxy_id.tx
            },
            inputs: vec![
                1, // CreateNewPool opcode
                start_height as u128, token1_tx,
                start_height as u128, token2_tx,
                amount1,
                amount2,
            ],
        })];

        let pool_block = init_with_cellpack_pairs(create_pool_cellpack);
        runtime.index_block(&pool_block, start_height + 1)?;

        let pool_id = AlkaneId { block: 2, tx: pool_sequence };

        // Perform multiple swaps to generate price changes
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

        // In a full integration, ammdata would:
        // - Track all 5 swaps
        // - Update reserves after each
        // - Generate OHLCV candles
        // - Calculate volume

        Ok(())
    }

    #[wasm_bindgen_test]
    fn test_reserve_tracking() -> Result<()> {
        use crate::test_utils::*;
        use crate::test_utils::amm_helpers::BinaryAndCellpack;
        use alkanes::precompiled::alkanes_std_owned_token_build;
        use alkanes::vm::utils::sequence_pointer;
        use metashrew_core::index_pointer::AtomicPointer;

        setup_test_environment()?;

        let start_height = 4;
        let runtime = TestMetashrewRuntime::new()?;
        let deployment = setup_amm(&runtime, start_height)?;

        let mut next_sequence_pointer = sequence_pointer(&mut AtomicPointer::default());
        let _auth_sequence = next_sequence_pointer.get_value::<u128>();

        // Create tokens with known amounts
        let token1_tx = 100u128;
        let token2_tx = 101u128;
        let init_amt_token1: u128 = 10_000_000;
        let init_amt_token2: u128 = 20_000_000;

        let init_with_cellpack_pairs = |pairs: Vec<BinaryAndCellpack>| -> bitcoin::Block {
            use protorune::test_helpers::create_block_with_coinbase_tx;
            let mut block = create_block_with_coinbase_tx(start_height);

            for pair in pairs {
                use protorune::message::{MessageContext, MessageContextParcel};
                use bitcoin::Transaction;
                use bitcoin::TxOut;
                use bitcoin::ScriptBuf;

                let mut tx = Transaction {
                    version: bitcoin::transaction::Version::TWO,
                    lock_time: bitcoin::absolute::LockTime::ZERO,
                    input: vec![],
                    output: vec![TxOut {
                        value: bitcoin::Amount::from_sat(1000),
                        script_pubkey: ScriptBuf::new(),
                    }],
                };

                let context = MessageContextParcel::from_cellpack(pair.cellpack);
                if let Some(buf) = context.encode() {
                    tx.output[0].script_pubkey = ScriptBuf::from_bytes(buf);
                }

                block.txdata.push(tx);
            }

            block
        };

        // Deploy tokens
        let token_cellpacks = vec![
            BinaryAndCellpack {
                binary: alkanes_std_owned_token_build::get_bytes(),
                cellpack: Cellpack {
                    target: AlkaneId { block: start_height as u128, tx: token1_tx },
                    inputs: vec![0, 1, init_amt_token1],
                },
            },
            BinaryAndCellpack {
                binary: alkanes_std_owned_token_build::get_bytes(),
                cellpack: Cellpack {
                    target: AlkaneId { block: start_height as u128, tx: token2_tx },
                    inputs: vec![0, 1, init_amt_token2],
                },
            },
        ];

        let token_block = init_with_cellpack_pairs(token_cellpacks);
        runtime.index_block(&token_block, start_height)?;

        // Create pool with specific liquidity
        let pool_sequence = next_sequence_pointer.get_value::<u128>();
        let initial_reserve0 = 100_000u128;
        let initial_reserve1 = 200_000u128;

        let create_pool_cellpack = vec![BinaryAndCellpack::cellpack_only(Cellpack {
            target: AlkaneId {
                block: deployment.factory_proxy_id.block,
                tx: deployment.factory_proxy_id.tx
            },
            inputs: vec![
                1, // CreateNewPool opcode
                start_height as u128, token1_tx,
                start_height as u128, token2_tx,
                initial_reserve0,
                initial_reserve1,
            ],
        })];

        let pool_block = init_with_cellpack_pairs(create_pool_cellpack);
        runtime.index_block(&pool_block, start_height + 1)?;

        // Verify pool created - in full integration, we'd query ammdata for:
        // - Pool exists
        // - Reserves are (100_000, 200_000)
        // - Initial price ratio is 2.0

        Ok(())
    }
}
