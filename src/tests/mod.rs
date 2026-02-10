/// Test modules for ESPO
///
/// WASM tests require running with wasm-pack:
/// ```bash
/// wasm-pack test --node --features test-utils
/// ```

#[cfg(all(test, target_arch = "wasm32"))]
pub mod wasm_amm_tests;
