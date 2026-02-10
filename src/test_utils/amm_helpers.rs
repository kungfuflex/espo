/// AMM deployment helpers for testing
///
/// This module provides utilities to deploy and initialize the OYL AMM
/// following the patterns from reference/subfrost-alkanes.
use crate::test_utils::{TestMetashrewRuntime, fixtures};
use alkanes_support::cellpack::Cellpack;
use alkanes_support::id::AlkaneId;
use anyhow::Result;
use bitcoin::hashes::Hash;
use bitcoin::{Block, OutPoint};
use metashrew_support::index_pointer::KeyValuePointer;

// Constants from OYL protocol
pub const AMM_FACTORY_ID: u128 = 0xffef; // 65519
pub const AUTH_TOKEN_FACTORY_ID: u128 = 0xffed; // 65517
pub const AMM_FACTORY_LOGIC_IMPL_TX: u128 = 2;
pub const POOL_BEACON_PROXY_TX: u128 = 0xbeac1;
pub const POOL_UPGRADEABLE_BEACON_TX: u128 = 0xbeac0;
pub const AMM_FACTORY_PROXY_TX: u128 = 1;

/// Information about deployed AMM contracts
#[derive(Debug, Clone)]
pub struct AmmDeployment {
    pub factory_proxy_id: AlkaneId,
    pub factory_logic_id: AlkaneId,
    pub pool_template_id: AlkaneId,
    pub pool_beacon_proxy_id: AlkaneId,
    pub pool_upgradeable_beacon_id: AlkaneId,
    pub auth_token_factory_id: AlkaneId,
    pub factory_auth_token_id: AlkaneId,
    /// All blocks created during AMM deployment, indexed by height
    pub blocks: std::collections::HashMap<u32, Block>,
}

/// A binary and its cellpack for deployment
#[derive(Debug, Clone)]
pub struct BinaryAndCellpack {
    pub binary: Vec<u8>,
    pub cellpack: Cellpack,
}

impl BinaryAndCellpack {
    pub fn new(binary: Vec<u8>, cellpack: Cellpack) -> Self {
        Self { binary, cellpack }
    }

    /// Creates a BinaryAndCellpack with an empty binary (cellpack-only operation)
    pub fn cellpack_only(cellpack: Cellpack) -> Self {
        Self { binary: Vec::new(), cellpack }
    }
}

/// Deploy AMM infrastructure contracts (5 blocks)
///
/// This deploys:
/// 1. Pool template
/// 2. Auth token factory
/// 3. Factory logic implementation
/// 4. Beacon proxy
/// 5. Upgradeable beacon
pub fn deploy_amm_infrastructure(
    runtime: &TestMetashrewRuntime,
    start_height: u32,
) -> Result<AmmDeployment> {
    println!("[AMM] Deploying AMM infrastructure contracts");

    let mut height = start_height;
    let mut blocks = std::collections::HashMap::new();

    // 1. Deploy pool template to factory space
    let pool_cellpack = vec![BinaryAndCellpack {
        binary: fixtures::get_pool_wasm().to_vec(),
        cellpack: Cellpack {
            target: AlkaneId { block: 3, tx: AMM_FACTORY_ID },
            inputs: vec![50], // Deployment marker
        },
    }];
    let pool_block = init_with_cellpack_pairs(pool_cellpack);
    runtime.index_block(&pool_block, height)?;
    blocks.insert(height, pool_block);
    println!("[AMM] Pool template deployed at height {}", height);
    height += 1;

    // 2. Deploy auth token factory

    let auth_cellpack = vec![BinaryAndCellpack {
        binary: fixtures::get_auth_token_wasm().to_vec(),
        cellpack: Cellpack {
            target: AlkaneId { block: 3, tx: AUTH_TOKEN_FACTORY_ID },
            inputs: vec![100], // Deployment marker
        },
    }];
    let auth_block = init_with_cellpack_pairs(auth_cellpack);
    runtime.index_block(&auth_block, height)?;
    blocks.insert(height, auth_block);
    println!("[AMM] Auth token factory deployed at height {}", height);
    height += 1;

    // 3. Deploy AMM factory logic implementation
    let factory_cellpack = vec![BinaryAndCellpack {
        binary: fixtures::get_factory_wasm().to_vec(),
        cellpack: Cellpack {
            target: AlkaneId { block: 3, tx: AMM_FACTORY_LOGIC_IMPL_TX },
            inputs: vec![50], // Deployment marker
        },
    }];
    let factory_block = init_with_cellpack_pairs(factory_cellpack);
    runtime.index_block(&factory_block, height)?;
    blocks.insert(height, factory_block);
    println!("[AMM] Factory logic deployed at height {}", height);
    height += 1;

    // 4. Deploy beacon proxy for pools

    let beacon_proxy_cellpack = vec![BinaryAndCellpack {
        binary: fixtures::get_beacon_proxy_wasm().to_vec(),
        cellpack: Cellpack {
            target: AlkaneId { block: 3, tx: POOL_BEACON_PROXY_TX },
            inputs: vec![0x8fff], // Beacon proxy marker
        },
    }];
    let beacon_proxy_block = init_with_cellpack_pairs(beacon_proxy_cellpack);
    runtime.index_block(&beacon_proxy_block, height)?;
    blocks.insert(height, beacon_proxy_block);
    println!("[AMM] Beacon proxy deployed at height {}", height);
    height += 1;

    // 5. Deploy upgradeable beacon (points to pool template)

    let upgradeable_beacon_cellpack = vec![BinaryAndCellpack {
        binary: fixtures::get_upgradeable_beacon_wasm().to_vec(),
        cellpack: Cellpack {
            target: AlkaneId { block: 3, tx: POOL_UPGRADEABLE_BEACON_TX },
            inputs: vec![
                0x7fff,         // Upgradeable beacon marker
                4,              // pool_template.block (deployed at start_height)
                AMM_FACTORY_ID, // pool_template.tx
                1,              // version
            ],
        },
    }];
    let upgradeable_beacon_block = init_with_cellpack_pairs(upgradeable_beacon_cellpack);
    runtime.index_block(&upgradeable_beacon_block, height)?;
    blocks.insert(height, upgradeable_beacon_block);
    println!("[AMM] Upgradeable beacon deployed at height {}", height);

    println!("[AMM] Infrastructure contracts deployed successfully");

    let deployment = AmmDeployment {
        factory_proxy_id: AlkaneId { block: 0, tx: 0 }, // Will be set after proxy deployment
        factory_logic_id: AlkaneId { block: start_height as u128, tx: AMM_FACTORY_LOGIC_IMPL_TX },
        pool_template_id: AlkaneId { block: start_height as u128, tx: AMM_FACTORY_ID },
        pool_beacon_proxy_id: AlkaneId { block: start_height as u128, tx: POOL_BEACON_PROXY_TX },
        pool_upgradeable_beacon_id: AlkaneId {
            block: start_height as u128,
            tx: POOL_UPGRADEABLE_BEACON_TX,
        },
        auth_token_factory_id: AlkaneId { block: start_height as u128, tx: AUTH_TOKEN_FACTORY_ID },
        factory_auth_token_id: AlkaneId { block: 0, tx: 0 }, // Will be determined after initialization
        blocks,
    };

    Ok(deployment)
}

/// Deploy and initialize the factory proxy (MUST be in same block!)

pub fn deploy_factory_proxy(
    runtime: &TestMetashrewRuntime,
    block_height: u32,
    deployment: &AmmDeployment,
) -> Result<(Block, AlkaneId, AlkaneId)> {
    println!("[AMM] Deploying factory proxy and initializing");

    // Calculate auth token sequence
    // In alkanes, sequences start at 0 for block 2 (first user block after genesis)
    // For auth tokens in factory setup, we expect sequence 0
    let auth_sequence = 0u128;

    // Deploy proxy AND initialize in SAME block (critical!)
    let cellpack_pairs: Vec<BinaryAndCellpack> = vec![
        // 1. Deploy upgradeable proxy for factory
        BinaryAndCellpack {
            binary: fixtures::get_upgradeable_proxy_wasm().to_vec(),
            cellpack: Cellpack {
                target: AlkaneId { block: 3, tx: AMM_FACTORY_PROXY_TX },
                inputs: vec![
                    0x7fff, // Upgradeable proxy marker
                    deployment.factory_logic_id.block,
                    deployment.factory_logic_id.tx,
                    1, // version
                ],
            },
        },
        // 2. Initialize the factory (via proxy) - SAME transaction batch
        BinaryAndCellpack::cellpack_only(Cellpack {
            target: AlkaneId { block: block_height as u128, tx: AMM_FACTORY_PROXY_TX },
            inputs: vec![
                0,                                           // InitFactory opcode
                POOL_BEACON_PROXY_TX,                        // pool_factory_id for cloning
                deployment.pool_upgradeable_beacon_id.block, // beacon_id.block
                deployment.pool_upgradeable_beacon_id.tx,    // beacon_id.tx
            ],
        }),
    ];

    // Get a dummy outpoint for chaining
    let dummy_block = protorune::test_helpers::create_block_with_coinbase_tx(block_height);
    let input_outpoint = OutPoint { txid: dummy_block.txdata[0].compute_txid(), vout: 0 };

    let test_block = init_with_cellpack_pairs_w_input(cellpack_pairs, input_outpoint);
    runtime.index_block(&test_block, block_height)?;

    let factory_proxy_id = AlkaneId { block: block_height as u128, tx: AMM_FACTORY_PROXY_TX };

    let factory_auth_token_id = AlkaneId { block: 2, tx: auth_sequence };

    println!("[AMM] Factory proxy deployed and initialized: {:?}", factory_proxy_id);
    println!("[AMM] Factory auth token: {:?}", factory_auth_token_id);

    Ok((test_block, factory_proxy_id, factory_auth_token_id))
}

/// Complete AMM setup: deploy infrastructure and initialize factory

pub fn setup_amm(runtime: &TestMetashrewRuntime, start_height: u32) -> Result<AmmDeployment> {
    let mut deployment = deploy_amm_infrastructure(runtime, start_height)?;

    let (proxy_block, factory_proxy_id, factory_auth_token_id) =
        deploy_factory_proxy(runtime, start_height + 5, &deployment)?;

    deployment.factory_proxy_id = factory_proxy_id;
    deployment.factory_auth_token_id = factory_auth_token_id;
    deployment.blocks.insert(start_height + 5, proxy_block);

    println!("[AMM] AMM setup complete");
    Ok(deployment)
}

// Helper functions for creating blocks with cellpacks

/// Create a block with multiple cellpacks
pub fn init_with_cellpack_pairs(cellpack_pairs: Vec<BinaryAndCellpack>) -> Block {
    init_with_cellpack_pairs_w_input(cellpack_pairs, OutPoint::null())
}

/// Create a block with multiple cellpacks and an input outpoint

fn init_with_cellpack_pairs_w_input(
    cellpack_pairs: Vec<BinaryAndCellpack>,
    input_outpoint: OutPoint,
) -> Block {
    use alkanes_support::envelope::RawEnvelope;
    use bitcoin::{Amount, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness};

    let mut transactions = Vec::new();

    // Create coinbase
    let coinbase = Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::locktime::absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_00000_000),
            script_pubkey: ScriptBuf::new(),
        }],
    };
    transactions.push(coinbase);

    // Create transaction for each cellpack pair
    for pair in cellpack_pairs {
        let mut tx_inputs = vec![];

        // Add input if not null
        if !input_outpoint.is_null() {
            tx_inputs.push(TxIn {
                previous_output: input_outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            });
        }

        // Add binary witness if present (for deployments)
        let witness = if !pair.binary.is_empty() {
            RawEnvelope::from(pair.binary).to_witness(true)
        } else {
            Witness::new()
        };

        tx_inputs.push(TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness,
        });

        // Create a Runestone with a Protostone containing the cellpack
        use ordinals::Runestone;
        use protorune_support::protostone::{Protostone, Protostones};

        let protostone = Protostone {
            burn: None,
            message: pair.cellpack.encipher(),
            edicts: vec![],
            refund: Some(0),  // Refund to output 0 (the regular output) on error
            pointer: Some(0), // Pointer to output 0 (where alkanes/runes go on success)
            from: None,
            protocol_tag: 1, // Alkanes protocol tag
        };

        // Encode protostones using the Protostones trait
        let protostones_vec = vec![protostone];
        let protocol_field = protostones_vec.encipher().expect("Failed to encode protostones");

        // Create runestone with protostone in the protocol field
        let runestone = Runestone {
            edicts: vec![],
            etching: None,
            mint: None,
            pointer: Some(0), // Point to output 0
            protocol: Some(protocol_field),
        };

        // Encode runestone into OP_RETURN script
        let runestone_script = runestone.encipher();

        let tx = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::locktime::absolute::LockTime::ZERO,
            input: tx_inputs,
            output: vec![
                // Regular output FIRST (index 0)
                TxOut {
                    value: Amount::from_sat(100_000_000), // 1 BTC
                    script_pubkey: ScriptBuf::new(),
                },
                // OP_RETURN output SECOND (index 1)
                TxOut { value: Amount::ZERO, script_pubkey: runestone_script },
            ],
        };

        transactions.push(tx);
    }

    // Create block header
    let header = bitcoin::block::Header {
        version: bitcoin::block::Version::from_consensus(1),
        prev_blockhash: bitcoin::BlockHash::from_byte_array([0u8; 32]),
        merkle_root: bitcoin::TxMerkleNode::from_byte_array([0u8; 32]),
        time: 0,
        bits: bitcoin::CompactTarget::from_consensus(0x207fffff),
        nonce: 0,
    };

    Block { header, txdata: transactions }
}

// WASM getters have been moved to fixtures.rs for better organization
// They are now loaded via include_bytes! from test_data/ directory
