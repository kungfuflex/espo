pub mod consts;
pub mod schemas;
pub mod utils;

// Modules that require non-WASM dependencies (tokio, rocksdb, etc.)
#[cfg(not(target_arch = "wasm32"))]
pub mod alkanes;
#[cfg(not(target_arch = "wasm32"))]
pub mod bitcoind_flexible;
#[cfg(not(target_arch = "wasm32"))]
pub mod config;
#[cfg(not(target_arch = "wasm32"))]
pub mod core;
#[cfg(not(target_arch = "wasm32"))]
pub mod debug;
#[cfg(not(target_arch = "wasm32"))]
pub mod explorer;
#[cfg(not(target_arch = "wasm32"))]
pub mod modules;
#[cfg(not(target_arch = "wasm32"))]
pub mod runtime;

// Test utilities available for testing
// Always compiled for non-wasm to support both unit tests and integration tests
// Heavy dependencies (metashrew-runtime, wasmtime) are gated behind test-utils feature internally
#[cfg(not(target_arch = "wasm32"))]
pub mod test_utils;

// WASM tests module
#[cfg(all(test, target_arch = "wasm32"))]
pub mod tests;

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU32;

// Shared ESPO height cell (used by config helpers and the indexer).
pub static ESPO_HEIGHT: OnceLock<Arc<AtomicU32>> = OnceLock::new();

// Last known safe tip height fetched from the block source.
pub static SAFE_TIP: OnceLock<Arc<AtomicU32>> = OnceLock::new();
