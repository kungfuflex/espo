/// Metashrew runtime wrapper for testing
///
/// This module provides a test-friendly wrapper around the metashrew/alkanes runtime
/// for integration testing of ESPO with AMM operations using actual RocksDB storage.
use anyhow::{Context, Result};
use bitcoin::Block;
use metashrew_runtime::MetashrewRuntime;
use metashrew_support::utils;
use rocksdb::DB;
use rockshrew_runtime::adapter::RocksDBRuntimeAdapter;
use std::sync::Arc;
use tempfile::TempDir;

/// The alkanes.wasm indexer binary (v2.1.6 regtest)
const ALKANES_WASM: &[u8] = include_bytes!("../../test_data/alkanes.wasm");

/// Test-friendly wrapper around the metashrew runtime
///
/// This wrapper:
/// - Initializes MetashrewRuntime with RocksDB storage
/// - Loads the actual alkanes.wasm indexer
/// - Provides methods to index blocks
/// - Offers query capabilities for testing
/// - Manages cleanup via TempDir
pub struct TestMetashrewRuntime {
    runtime: Arc<tokio::sync::Mutex<MetashrewRuntime<RocksDBRuntimeAdapter>>>,
    db: Arc<DB>,
    _temp_dir: TempDir,
}

impl TestMetashrewRuntime {
    /// Create a new test runtime with RocksDB storage
    ///
    /// This:
    /// - Creates a temporary RocksDB database
    /// - Initializes wasmtime engine with async support
    /// - Loads alkanes.wasm into the runtime
    /// - Configures network for regtest
    pub fn new() -> Result<Self> {
        Self::configure_network();

        // Create temporary directory for RocksDB
        let temp_dir = TempDir::new().context("Failed to create temp directory")?;

        // Open RocksDB with optimized options
        let opts = RocksDBRuntimeAdapter::get_optimized_options();
        let db = Arc::new(DB::open(&opts, temp_dir.path()).context("Failed to open RocksDB")?);

        // Create RocksDB adapter
        let adapter = RocksDBRuntimeAdapter::new(db.clone());

        // Create wasmtime engine with async support
        let mut config = wasmtime::Config::default();
        config.async_support(true);
        let engine = wasmtime::Engine::new(&config).context("Failed to create wasmtime engine")?;

        // Create MetashrewRuntime
        // Note: This is blocking, but only called during test setup
        let runtime = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                MetashrewRuntime::new(
                    ALKANES_WASM,
                    adapter,
                    engine,
                    None,  // intercept_logging
                    false, // enable_smt
                )
                .await
            })
        })?;

        Ok(Self { runtime: Arc::new(tokio::sync::Mutex::new(runtime)), db, _temp_dir: temp_dir })
    }

    /// Configure network parameters for regtest
    fn configure_network() {
        use protorune_support::network::{NetworkParams, set_network};

        set_network(NetworkParams {
            bech32_prefix: String::from("bcrt"),
            p2pkh_prefix: 0x64,
            p2sh_prefix: 0xc4,
        });
    }

    /// Index a block through the alkanes runtime
    ///
    /// This processes the block through metashrew, generating traces
    /// that ESPO can then index for its modules.
    pub fn index_block(&self, block: &Block, height: u32) -> Result<()> {
        // Serialize the block
        let block_bytes = utils::consensus_encode(block)
            .with_context(|| format!("Failed to serialize block at height {}", height))?;

        // Index the block through MetashrewRuntime
        // This is blocking, but it's a test so that's acceptable
        let index_result: Result<()> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let mut runtime = self.runtime.lock().await;

                // Set context
                {
                    let mut context = runtime.context.lock().await;
                    context.block = block_bytes;
                    context.height = height;
                }

                // Run the indexer
                runtime
                    .run()
                    .await
                    .with_context(|| format!("Failed to index block at height {}", height))?;

                // Refresh memory
                runtime
                    .refresh_memory()
                    .await
                    .with_context(|| format!("Failed to refresh memory at height {}", height))?;

                Ok::<(), anyhow::Error>(())
            })
        });

        index_result
    }

    /// Get all traces for a block after indexing
    ///
    /// This extracts all traces generated when indexing a block through metashrew.
    /// Returns PartialEspoTrace structures that can be converted to EspoBlock.
    ///
    /// # Example
    ///
    /// ```no_run
    /// let runtime = TestMetashrewRuntime::new()?;
    /// runtime.index_block(&block, height)?;
    ///
    /// let traces = runtime.get_traces_for_block(height)?;
    /// let espo_block = build_espo_block(height, &block, traces)?;
    /// ```
    pub fn get_traces_for_block(
        &self,
        height: u32,
    ) -> Result<Vec<crate::alkanes::trace::PartialEspoTrace>> {
        use crate::alkanes::metashrew::decode_trace_blob;

        // Scan for all trace keys that match /trace/{outpoint} pattern
        // We need to iterate through RocksDB and find traces for transactions in this block
        let mut traces = Vec::new();

        // The key pattern is: /trace/{outpoint_bytes}
        // where outpoint_bytes is 36 bytes (32 txid + 4 vout)

        // Iterate through all keys starting with /trace/
        let prefix = b"/trace/";
        let iter = self.db.prefix_iterator(prefix);

        for item in iter {
            if let Ok((key, value)) = item {
                // Skip if not long enough to be a trace key (/trace/ + 36 bytes minimum)
                if key.len() < (prefix.len() + 36) {
                    continue;
                }

                // Check if this is a leaf trace key ending in /0
                // Traces are stored at: /trace/{outpoint_36_bytes}/0
                let key_suffix = &key[prefix.len()..];

                // We want keys that are: 36 bytes (outpoint) + "/" + "0" = 38 bytes total
                if key_suffix.len() < 38 {
                    continue;
                }

                // Check if it ends with "/0"
                if &key_suffix[36..38] != b"/0" {
                    continue;
                }

                // Extract the outpoint (first 36 bytes after /trace/)
                let outpoint_bytes = key_suffix[..36].to_vec();

                // Decode the trace using ESPO's decode_trace_blob
                // which handles both "height:HEX" format and raw protobuf bytes
                if let Some(trace) = decode_trace_blob(&value) {
                    traces.push(crate::alkanes::trace::PartialEspoTrace {
                        protobuf_trace: trace,
                        outpoint: outpoint_bytes,
                    });
                }
            }
        }

        println!("[RUNTIME] Found {} traces for block {}", traces.len(), height);
        Ok(traces)
    }

    /// Get the DB handle for advanced queries
    pub fn db(&self) -> &Arc<DB> {
        &self.db
    }
}

impl Default for TestMetashrewRuntime {
    fn default() -> Self {
        Self::new().expect("Failed to create default TestMetashrewRuntime")
    }
}

// TempDir automatically cleans up the RocksDB when dropped

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_runtime_initialization() {
        let runtime = TestMetashrewRuntime::new().unwrap();
        assert!(runtime.db().get(b"test").is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_runtime_index_empty_blocks() {
        let runtime = TestMetashrewRuntime::new().unwrap();

        // Index a few empty blocks
        for height in 0..3 {
            let block = protorune::test_helpers::create_block_with_coinbase_tx(height);
            runtime.index_block(&block, height).unwrap();
        }
    }
}
