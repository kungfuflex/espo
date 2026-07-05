use bitcoin::Network;

/// Apply the `ESPO_GENESIS_HEIGHT` env override (if set + parseable) over a
/// per-network genesis default. EVERY genesis-height source in espo — the
/// per-module `*_genesis_block` fns (which drive the indexer start via
/// `module_resume_start_height`) AND `alkanes_genesis_block` (the reorg genesis)
/// — routes through this, so the whole indexer agrees on one genesis.
///
/// This keeps espo aligned with a rockshrew started at a non-genesis
/// `--start-block`: on signet/testnet the alkanes wasm's baked GENESIS_BLOCK is 0,
/// but a rockshrew launched with `--start-block 240000` effectively anchors
/// genesis at 240000, so espo must use the same height or it stalls trying to load
/// blocks the indexer never wrote. Set `ESPO_GENESIS_HEIGHT=240000` to match.
pub fn genesis_with_override(default: u32) -> u32 {
    if let Ok(raw) = std::env::var("ESPO_GENESIS_HEIGHT") {
        if let Ok(height) = raw.trim().parse::<u32>() {
            return height;
        }
    }
    default
}

pub fn alkanes_genesis_block(network: Network) -> u32 {
    genesis_with_override(match network {
        Network::Bitcoin => 880_000,
        _ => 0,
    })
}
