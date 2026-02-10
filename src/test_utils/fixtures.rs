/// Shared test fixtures and data

/// Get the alkanes.wasm binary for testing
///
/// This function loads the alkanes.wasm file (v2.1.6 regtest) from the test_data directory.
/// The WASM file is embedded at compile time using include_bytes!
pub fn get_alkanes_wasm() -> &'static [u8] {
    include_bytes!("../../test_data/alkanes.wasm")
}

/// Get the OYL AMM factory.wasm binary for testing
///
/// This is the OYL AMM factory contract that manages pool creation and protocol configuration.
pub fn get_factory_wasm() -> &'static [u8] {
    include_bytes!("../../test_data/factory.wasm")
}

/// Get the OYL AMM pool.wasm binary for testing
///
/// This is the OYL AMM pool contract template that implements the automated market maker logic.
pub fn get_pool_wasm() -> &'static [u8] {
    include_bytes!("../../test_data/pool.wasm")
}

/// Check if alkanes.wasm is available
pub fn has_alkanes_wasm() -> bool {
    !get_alkanes_wasm().is_empty()
}

/// Get the alkanes_std_auth_token.wasm binary
pub fn get_auth_token_wasm() -> &'static [u8] {
    include_bytes!("../../test_data/alkanes_std_auth_token.wasm")
}

/// Get the alkanes_std_beacon_proxy.wasm binary
pub fn get_beacon_proxy_wasm() -> &'static [u8] {
    include_bytes!("../../test_data/alkanes_std_beacon_proxy.wasm")
}

/// Get the alkanes_std_upgradeable_beacon.wasm binary
pub fn get_upgradeable_beacon_wasm() -> &'static [u8] {
    include_bytes!("../../test_data/alkanes_std_upgradeable_beacon.wasm")
}

/// Get the alkanes_std_upgradeable_proxy.wasm binary
pub fn get_upgradeable_proxy_wasm() -> &'static [u8] {
    include_bytes!("../../test_data/alkanes_std_upgradeable_proxy.wasm")
}

/// Check if all AMM WASM files are available
pub fn has_amm_wasms() -> bool {
    !get_factory_wasm().is_empty()
        && !get_pool_wasm().is_empty()
        && !get_auth_token_wasm().is_empty()
        && !get_beacon_proxy_wasm().is_empty()
        && !get_upgradeable_beacon_wasm().is_empty()
        && !get_upgradeable_proxy_wasm().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alkanes_wasm_check() {
        let wasm = get_alkanes_wasm();
        println!("alkanes.wasm size: {} bytes", wasm.len());

        assert!(has_alkanes_wasm(), "alkanes.wasm should be available");
        assert!(wasm.len() > 1_000_000, "alkanes.wasm should be > 1MB");
    }

    #[test]
    fn test_amm_wasms_check() {
        let factory = get_factory_wasm();
        let pool = get_pool_wasm();

        println!("factory.wasm size: {} bytes", factory.len());
        println!("pool.wasm size: {} bytes", pool.len());

        assert!(has_amm_wasms(), "AMM WASM files should be available");
        assert!(factory.len() > 100_000, "factory.wasm should be > 100KB");
        assert!(pool.len() > 100_000, "pool.wasm should be > 100KB");
    }
}
