use bitcoin::Network;

/// The block at which alkanes activate — espo's index/start/reorg genesis.
///
/// `ESPO_GENESIS_HEIGHT` (env) overrides the per-network default when set. This
/// keeps espo aligned with a rockshrew started at a non-genesis `--start-block`:
/// on signet/testnet the alkanes wasm's baked GENESIS_BLOCK is 0, but a rockshrew
/// launched with `--start-block 240000` effectively anchors genesis at 240000, so
/// espo must use the same height or it stalls trying to load blocks the indexer
/// never wrote. Set `ESPO_GENESIS_HEIGHT=240000` to match.
pub fn alkanes_genesis_block(network: Network) -> u32 {
    if let Ok(raw) = std::env::var("ESPO_GENESIS_HEIGHT") {
        if let Ok(height) = raw.trim().parse::<u32>() {
            return height;
        }
    }
    match network {
        Network::Bitcoin => 880_000,
        _ => 0,
    }
}
