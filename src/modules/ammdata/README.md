## ESPO ammdata module

The ammdata espo module generates OHLCV data and tradehistory from traces from oylamm.

### non-oylamm pools

Curve CryptoSwap v2 pools (the BTCUSD frBTC/frUSD market) are indexed too, via
`utils/cryptoswap.rs`. They are not discovered: there is no factory to watch, so
they come from a static per-network table and are registered only once their
storage reads back, which keeps a pool from being advertised before it exists.

Everything downstream is shared with oylamm unchanged, because activity is
derived from the pool alkane's balance deltas and reserves are read as balances,
neither of which cares about the pool's shape. The single exception is price: on
a constant-product pool `quote/base` is the spot price, on a v2 curve it is not,
so those pools publish a price read from their own `price_oracle` and
`IndexState::pool_price_overrides` replaces the ratio for them.

### rocksdb schema

SchemaAlkaneId -> Borsh
pub struct SchemaAlkaneId {
    pub block: u32,
    pub tx: u64,
}



ammdata/block_index -> blockNumber(u64 LE) : The blockNumber of the last proccessed block for the ammdata indexer
ammdata/ohlcv/<alkanePoolId>/<interval>/<timestamp> -> ohlcv data
ammdata/tradehistory/<poolId>/<timestamp> -> trade data