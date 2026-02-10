// Test utilities for ESPO integration testing

// Non-WASM test utilities (require tempfile, rocksdb, etc.)
#[cfg(not(target_arch = "wasm32"))]
pub use tempfile::TempDir;

#[cfg(not(target_arch = "wasm32"))]
pub mod chain_builder;
#[cfg(not(target_arch = "wasm32"))]
pub mod config_builder;
#[cfg(not(target_arch = "wasm32"))]
pub mod mock_node;

pub mod fixtures;

// Test utilities that require the test-utils feature (metashrew-runtime, rockshrew-runtime, wasmtime)
#[cfg(feature = "test-utils")]
pub mod amm_helpers;
#[cfg(feature = "test-utils")]
pub mod metashrew_runtime;
#[cfg(feature = "test-utils")]
pub mod trace_helpers;

// Re-export commonly used items
#[cfg(not(target_arch = "wasm32"))]
pub use chain_builder::ChainBuilder;
#[cfg(not(target_arch = "wasm32"))]
pub use config_builder::TestConfigBuilder;
#[cfg(not(target_arch = "wasm32"))]
pub use mock_node::MockBitcoinNode;

#[cfg(feature = "test-utils")]
pub use metashrew_runtime::TestMetashrewRuntime;

// Re-export AMM helpers
#[cfg(feature = "test-utils")]
pub use amm_helpers::{
    AMM_FACTORY_ID, AMM_FACTORY_LOGIC_IMPL_TX, AMM_FACTORY_PROXY_TX, AUTH_TOKEN_FACTORY_ID,
    AmmDeployment, BinaryAndCellpack, POOL_BEACON_PROXY_TX, POOL_UPGRADEABLE_BEACON_TX,
    deploy_amm_infrastructure, deploy_factory_proxy, init_with_cellpack_pairs, setup_amm,
};

// Re-export trace helpers
#[cfg(feature = "test-utils")]
pub use trace_helpers::build_espo_block;
