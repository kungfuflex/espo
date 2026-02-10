# Oylapi Implementation Plan (Step 6)

This document implements Step 6 of the migration plan by mapping every oylapi endpoint (from `agent-context/oyl-api.ts`) to current espo indices, identifying which modules and getters to use, and calling out missing getters and indices. It also notes where we can piggyback on existing storage in `src/modules/essentials/storage.rs` and `src/modules/ammdata/storage.rs`.

## Source inventory (what we can piggyback on)

### Essentials module storage and helpers

Existing indices in `src/modules/essentials/storage.rs`:
- `/index_height`: module index height.
- `/balances/<address>/<outpoint>`: balances per outpoint, keyed by address and outpoint; spent status is encoded via `EspoOutpoint.tx_spent` in the key. Helpers: `get_balance_for_address` in `src/modules/essentials/utils/balances/lib.rs`.
- `/outpoint_balances/<outpoint>`: balances per outpoint, with spent info; helpers: `get_outpoint_balances`, `get_outpoint_balances_with_spent`, `get_outpoint_balances_with_spent_batch`.
- `/outpoint_addr/<outpoint>`: outpoint -> address mapping.
- `/utxo_spk/<outpoint>` and `/addr_spk/<address>`: ScriptPubKey mapping.
- `/holders/<alkane>` and `/holders/count/<alkane>`: holders list and count; helper: `get_holders_for_alkane`.
- `/alkanes/holders/ordered/`: holders count ordered index (ranking support).
- `/alkane_balances/<owner>`: alkane-to-alkane balances; helper: `get_alkane_balances`.
- `/alkane_balance_txs/*`: balance tx lists (by alkane, by token, by height).
- `/alkane_info/<alkane>`: creation metadata (txid/height/timestamp).
- `/alkanes/creation/id/<alkane>`: `AlkaneCreationRecord` with name/symbol history and inspection metadata.
- `/alkanes/creation/ordered/*` and `/alkanes/creation/count`: ordered creation list and count.
- `/alkanes/name/<name>/<alkane>`: normalized name prefix index (uses `normalize_alkane_name`).
- `/alkane_tx_summary/<txid>`: trace + outflow summary for a tx.
- `/alkane_block/<height>/<idx>` and `/alkane_addr/<address>/<idx>`: tx lists by block and by address.

Useful existing functions:
- `get_balance_for_address`, `get_outpoint_balances*`, `get_holders_for_alkane`, `get_alkane_balances` in `src/modules/essentials/utils/balances/lib.rs`.
- `get_creation_record`, `get_creation_records_by_id`, `get_creation_count` in `src/modules/essentials/storage.rs`.
- `rpc_get_all_alkanes`, `rpc_get_alkane_info`, `rpc_get_address_outpoints` in `src/modules/essentials/storage.rs` (good reference for future getters).

### Ammdata module storage and helpers

Existing indices in `src/modules/ammdata/storage.rs`:
- `/index_height`: module index height.
- `/reserves_snapshot_v1`: `SchemaReservesSnapshot` (all pools) with base/quote reserves and token IDs.
- `/pools/<pool>`: per-pool data (market defs).
- `fc1:<pool>:<tf>:<bucket>`: candles for pools (via `read_candles_v1`).
- `activity:v1:<pool>:<ts>:<seq>`: activity log per pool; activity kinds: `TradeBuy`, `TradeSell`, `LiquidityAdd`, `LiquidityRemove`, `PoolCreate`.
- `activity:idx:v1:*`: secondary indexes for activity (ts, abs volume, side, etc) and optional per-pool counts.

Useful existing functions:
- `get_reserves_snapshot` in `src/modules/ammdata/storage.rs`.
- `fetch_all_pools` and `fetch_latest_reserves_for_pools` in `src/modules/ammdata/utils/live_reserves.rs`.
- `read_activity_for_pool`, `read_activity_for_pool_sorted` in `src/modules/ammdata/utils/activity.rs`.
- `read_candles_v1` in `src/modules/ammdata/utils/candles.rs`.
- Swap path helpers in `src/modules/ammdata/utils/pathfinder.rs`.

## Cross-cutting gaps that affect many endpoints

These are recurring needs across endpoints; each endpoint section references them explicitly.

1) Token metadata beyond name/symbol
- Existing: name/symbol are available via `AlkaneCreationRecord` and `/alkanes/name` index.
- Missing: decimals, totalSupply, cap, mint status, mintAmount, image, and other metadata fields in `AlkaneToken`/`AlkaneDetails`.
- Plan: add a `TokenMetadata` index in `essentials` (or a dedicated `metadata` module) that stores normalized metadata fields extracted from storage writes (`/name`, `/symbol`, `/decimals`, `/image`, `/cap`, etc) plus inspection metadata. Populate on index via traces and storage changes. Add getters for batch metadata lookup.

2) Pricing and USD conversions
- Existing: ammdata has pool reserves; no direct price cache.
- Missing: token price in sats and USD, FDV, marketcap, pool TVL/volume in USD, and price change windows.
- Plan: add an `ammdata` price cache keyed by token id and quote token (frBTC and bUSD). Populate from latest reserves snapshot and selected canonical pools. Add time-window aggregates for 1d/7d/30d/all-time token and pool volume in both raw and USD units.

3) Token-to-pool and pool-to-token reverse indexes
- Existing: `SchemaReservesSnapshot` can be scanned, but that is O(n) for token queries.
- Missing: fast token->pools mapping and pool name search.
- Plan: add `ammdata` indices:
  - `/token_pools/<token_id>/<pool_id>` for adjacency.
  - `/pool_tokens/<pool_id>` for token0/token1 (if not already in `/pools/`).
  - Optional `/pool_name_index/<normalized>` for search.

4) Address-aware AMM activity
- Existing: `SchemaActivityV1` lacks address and LP token amount fields.
- Missing: swap/mint/burn/creation history by address, LP token amounts, and transaction type in a single history stream.
- Plan: expand activity schema or create a parallel `ammdata` activity event index that includes:
  - address (caller / initiator)
  - token0Amount, token1Amount, lpTokenAmount for mint/burn/creation
  - sold/bought token ids for swaps
  - pool id
  - txid, timestamp
  - event type (swap/mint/burn/creation)
  - success flag
  Add secondary indexes by address, by token, and by pool.

5) Wrap/unwrap events
- Existing: not indexed in essentials or ammdata.
- Missing: all wrap/unwrap endpoints.
- Plan: add a new index (likely in `essentials`, since it already processes traces) that detects wrap/unwrap events by contract id and writes:
  - `wrap_events_by_address/<address>/<ts>/<seq>`
  - `wrap_events_all/<ts>/<seq>`
  - `unwrap_events_by_address/<address>/<ts>/<seq>`
  - `unwrap_events_all/<ts>/<seq>`
  - `unwrap_total_by_height` (or a rolling counter) for `/get-total-unwrap-amount`.

## Endpoint-by-endpoint implementation plan

Each endpoint includes: coverage status, modules/getters, missing getters, and missing indices plus implementation path.

### /get-alkanes-by-address
Overview: Return all alkane balances for a BTC address, enriched with name/symbol, pricing, and images.

Coverage:
- Partially supported. Core balances per address are indexed. Metadata and pricing are partially missing.

Modules and existing getters:
- Essentials:
  - `get_balance_for_address` (balances per address via `/balances/...`).
  - `get_creation_records_by_id` (batch name/symbol, creation data).
- Ammdata:
  - `get_reserves_snapshot` (for pricing via pool reserves).

New oylapi getters needed:
- `oyl_get_address_balances(address) -> Vec<(alkane_id, balance)>` wrapping `get_balance_for_address`.
- `oyl_get_alkane_metadata_batch(ids) -> TokenMetadata` (needs new metadata index if required fields beyond name/symbol).
- `oyl_get_token_price_batch(ids) -> {price_in_sats, price_in_usd}` using ammdata price cache.

Missing indices / implementation path:
- Add `token_metadata` index in essentials for decimals, images, cap, mint info.
- Add `token_price` cache in ammdata (or oylapi) computed from canonical frBTC/bUSD pools.
- Add `token_market_stats` index (holders, marketcap, fdv) to avoid O(n) holder scans.

### /get-alkanes-utxo
Overview: Return UTXOs for an address, with alkanes balances per outpoint and BTC metadata.

Coverage:
- Partially supported. Outpoints and alkane balances are indexed. Inscriptions/runes and confirmations are not.

Modules and existing getters:
- Essentials:
  - `rpc_get_address_outpoints` or `get_outpoint_balances_with_spent_batch` for per-outpoint balances.
  - `/outpoint_addr`, `/utxo_spk`, `/addr_spk` to build `FormattedUtxo` fields.
- Runtime/Electrum (not in storage): needed for confirmations and satoshis per outpoint.

New oylapi getters needed:
- `oyl_get_address_outpoints(address) -> Vec<Outpoint>` (wrapper around essentials outpoint indices).
- `oyl_get_outpoint_balances_batch(outpoints) -> Map<outpoint, balances>`.
- `oyl_get_outpoint_scriptpubkey_batch(outpoints)` (read `/utxo_spk`).

Missing indices / implementation path:
- Inscriptions/runes require a new ordinals/runes index or metashrew integration.
- Confirmations and satoshis require electrum/bitcoind lookups; consider caching `utxo_value` and `utxo_height` in essentials for cheap access.

### /get-amm-utxos
Overview: Return UTXOs for an address, but filtered/structured for AMM usage.

Coverage:
- Same as `/get-alkanes-utxo` plus AMM-specific filtering. Core indices exist, but AMM filter logic is missing.

Modules and existing getters:
- Essentials outpoint indices (same as above).
- Ammdata for pool/token data if filter depends on pool membership.

New oylapi getters needed:
- `oyl_get_amm_utxos(address, spend_strategy?)` that reuses `oyl_get_address_outpoints` and filters by:
  - UTXOs holding LP token alkanes or required token alkanes.

Missing indices / implementation path:
- If AMM filter needs to know which alkanes are pool tokens, add `ammdata/token_pools` index and/or `pool_tokens` index.

### /get-alkanes
Overview: List alkanes with pagination, sorting, and search; returns `AlkaneToken` entries.

Coverage:
- Partially supported. Creation records and name/symbol indexing exist. Sorting by price/volume/marketcap and full metadata are not fully supported.

Modules and existing getters:
- Essentials:
  - `/alkanes/creation/ordered` and `get_creation_count` for base list.
  - `/alkanes/name` for prefix search by name.
- Ammdata:
  - `get_reserves_snapshot` for pool-based pricing.
  - activity/candles for volume metrics (requires aggregation).

New oylapi getters needed:
- `oyl_list_alkanes_page(offset, limit)` reading creation ordered list.
- `oyl_search_alkanes_by_name(prefix)` using `/alkanes/name`.
- `oyl_sort_alkanes(sort_by, order)` backed by new precomputed sort keys.

Missing indices / implementation path:
- Add token-level aggregates for:
  - volume windows (1d/7d/30d/all-time) in raw units and USD.
  - price change windows (24h/7d/30d/all-time).
  - holders count (already exists) and marketcap/FDV (derived).
- Add symbol index if `searchQuery` should also match symbol (currently only name prefix).
- Consider `token_sort_index:<field>` to avoid O(n) sorts.

### /global-alkanes-search
Overview: Search across tokens and pools by name or exact id.

Coverage:
- Partially supported. Token name prefix index exists. Pool name search not indexed.

Modules and existing getters:
- Essentials `/alkanes/name` for token name prefix.
- Ammdata reserves snapshot for pool list (O(n) scan).

New oylapi getters needed:
- `oyl_search_tokens(query)` using name index + exact id match.
- `oyl_search_pools(query)` using a new pool name index or token-pool index.

Missing indices / implementation path:
- Add `ammdata/pool_name_index` (normalized `TOKEN0 / TOKEN1`), plus reverse lookup for pool ids.
- Add `ammdata/token_pools` to match exact `block:tx` in search for pools containing token.

### /get-alkane-details
Overview: Full details for one alkane, including pricing and metadata fields.

Coverage:
- Partially supported. Creation record exists. Additional metadata, supply, decimals, and pricing need new indices.

Modules and existing getters:
- Essentials:
  - `get_creation_record` for name/symbol and creation info.
  - `get_holders_for_alkane` for holders count and supply (but can be O(n) if many holders).
- Ammdata:
  - price cache (to add) and pool volume aggregates.

New oylapi getters needed:
- `oyl_get_token_details(alkane_id)` that joins creation record + metadata + price + aggregates.

Missing indices / implementation path:
- Add `token_metadata` index (decimals, image, cap, mint info).
- Add `token_supply` index to avoid scanning holders on each request.
- Add `token_price` and `token_volume` aggregates in ammdata.

### /get-pools
Overview: List pool ids for a factory.

Coverage:
- Not fully supported. Pools are known in ammdata, but no factory -> pool index.

Modules and existing getters:
- Ammdata: `get_reserves_snapshot` (contains all pools, but no factory map).

New oylapi getters needed:
- `oyl_get_pools_by_factory(factory_id)`.

Missing indices / implementation path:
- Add `ammdata/factory_pools/<factory_id>/<pool_id>` index when pools are discovered in `ammdata/main.rs` (use `NewPoolInfo.factory_id`).
- Store `factory_id` in `/pools/<pool>` to allow reverse checks.

### /get-pool-details
Overview: Details for one pool: reserves, TVL, volumes, APR, etc.

Coverage:
- Partially supported. Reserves and token ids exist. TVL/volume/pricing/APR requires aggregates and price cache.

Modules and existing getters:
- Ammdata:
  - `get_reserves_snapshot` for base/quote reserves and token ids.
  - candles/activity for volume (needs aggregation).
- Essentials:
  - token metadata for name/symbol and decimals.

New oylapi getters needed:
- `oyl_get_pool_snapshot(pool_id)` from reserves snapshot.
- `oyl_get_pool_metrics(pool_id)` from ammdata aggregates.
- `oyl_get_token_metadata_batch(token_ids)` for pool name.

Missing indices / implementation path:
- Add `ammdata/pool_metrics` index (tvl, volume1d/7d/30d/all-time, apr, tvlChange).
- Add `ammdata/pool_creation_info` (creator address, creation height, initial amounts) during pool creation processing.

### /address-positions
Overview: LP positions for an address with per-pool valuation.

Coverage:
- Partially supported. Address balances exist; pool reserves exist. Need LP total supply, pricing, and pool metadata.

Modules and existing getters:
- Essentials:
  - `get_balance_for_address` for LP token balances.
- Ammdata:
  - `get_reserves_snapshot` for reserves and token ids.

New oylapi getters needed:
- `oyl_get_address_lp_balances(address)` (filter balances to LP token alkanes).
- `oyl_get_pool_supply(pool_id)` (needs supply index).
- `oyl_get_pool_value(pool_id)` (from pool metrics + token price cache).

Missing indices / implementation path:
- Add `pool_lp_supply` index (total LP supply per pool).
- Add `token_price` and `pool_metrics` indexes for valuation.
- Add `token_pools` index to quickly map LP alkanes to pools if not 1:1 by id.

### /get-all-pools-details
Overview: Paginated pool list with aggregates, trending pools, largest pool, and totals.

Coverage:
- Partially supported for raw pool list. Aggregations and trending require new indices.

Modules and existing getters:
- Ammdata:
  - `get_reserves_snapshot` for pool list.
- Essentials:
  - token metadata for pool name.

New oylapi getters needed:
- `oyl_list_pools(page, limit, sort_by, order, searchQuery, address)`.
- `oyl_get_pool_aggregates()` to fill total TVL, volume, trending, largest pool.

Missing indices / implementation path:
- Add `ammdata/pool_metrics` (tvl/volume/apr/tvlChange) and `ammdata/pool_sort_index` for fast sorting.
- Add `ammdata/pool_trending` (24h volume or price change) precomputed per pool.
- Add `ammdata/pool_search_index` for pool name/token match.

### /get-pool-swap-history
Overview: Swap history for a pool, includes swaps and total counts.

Coverage:
- Partially supported. Activity log exists per pool but lacks full swap fields and address.

Modules and existing getters:
- Ammdata:
  - `read_activity_for_pool` / `read_activity_for_pool_sorted` for per-pool activity.

New oylapi getters needed:
- `oyl_get_pool_swaps(pool_id, count, offset, include_total)` built on a swap-specific activity index.

Missing indices / implementation path:
- Extend activity schema to store sold/bought token ids and amounts, and address.
- Add `ammdata/pool_swaps` index (by ts/seq) with total count for O(1) totals.

### /get-token-swap-history
Overview: Swap history for a token across all pools.

Coverage:
- Not supported without O(n) pool scans.

Modules and existing getters:
- Ammdata activity per pool (too expensive to scan all pools each query).

New oylapi getters needed:
- `oyl_get_token_swaps(token_id, count, offset)`.

Missing indices / implementation path:
- Add `ammdata/token_swaps/<token_id>/<ts>/<seq>` index populated when indexing activity. Derive token involvement from pool defs.
- Maintain a token swap count index for totals.

### /get-pool-mint-history
Overview: Liquidity add (mint) events for a pool.

Coverage:
- Partially supported. Activity has `LiquidityAdd` but lacks token0/token1 amounts, lpTokenAmount, and address.

Modules and existing getters:
- Ammdata activity log (needs extension).

New oylapi getters needed:
- `oyl_get_pool_mints(pool_id, count, offset)` reading mint event index.

Missing indices / implementation path:
- Extend activity event schema to include `token0Amount`, `token1Amount`, `lpTokenAmount`, `address`.
- Add `ammdata/pool_mints` index by pool.

### /get-pool-burn-history
Overview: Liquidity remove (burn) events for a pool.

Coverage:
- Partially supported with same gaps as mint history.

Modules and existing getters:
- Ammdata activity log (needs extension).

New oylapi getters needed:
- `oyl_get_pool_burns(pool_id, count, offset)`.

Missing indices / implementation path:
- Extend activity schema and add `ammdata/pool_burns` index.

### /get-pool-creation-history
Overview: Pool creation events (global or per pool).

Coverage:
- Partially supported. `PoolCreate` events exist in activity log but lack amounts, supply, and address.

Modules and existing getters:
- Ammdata activity (per pool).

New oylapi getters needed:
- `oyl_get_pool_creations(count, offset)`.

Missing indices / implementation path:
- Add `ammdata/pool_creations` index (global stream) with `token0Amount`, `token1Amount`, `tokenSupply`, `creatorAddress` fields.
- Capture these fields when detecting `NewPoolInfo` in `ammdata/main.rs`.

### /get-address-swap-history-for-pool
Overview: Swaps by address within a pool.

Coverage:
- Not supported. Activity lacks address and address index.

Modules and existing getters:
- None sufficient; requires new address-aware activity index.

New oylapi getters needed:
- `oyl_get_address_pool_swaps(address, pool_id, count, offset)`.

Missing indices / implementation path:
- Add `ammdata/address_pool_swaps/<address>/<pool>/<ts>/<seq>` index at index time with address extraction.
- Add totals index for paging.

### /get-address-swap-history-for-token
Overview: Swaps by address for a token across pools.

Coverage:
- Not supported without new address and token indexes.

Modules and existing getters:
- None sufficient.

New oylapi getters needed:
- `oyl_get_address_token_swaps(address, token_id, count, offset)`.

Missing indices / implementation path:
- Add `ammdata/address_token_swaps/<address>/<token>/<ts>/<seq>` derived from activity.

### /get-address-wrap-history
Overview: Wrap events for an address.

Coverage:
- Not supported; no wrap/unwrap index exists.

Modules and existing getters:
- None in essentials/ammdata.

New oylapi getters needed:
- `oyl_get_address_wraps(address, count, offset)`.

Missing indices / implementation path:
- Add `wrap_events_by_address` in essentials (see cross-cutting gap 5).
- Detect wraps during trace processing (based on known wrap contract id or opcode).

### /get-address-unwrap-history
Overview: Unwrap events for an address.

Coverage:
- Not supported; same as wrap.

Modules and existing getters:
- None.

New oylapi getters needed:
- `oyl_get_address_unwraps(address, count, offset)`.

Missing indices / implementation path:
- Add `unwrap_events_by_address` index in essentials.

### /get-all-wrap-history
Overview: Global wrap event stream.

Coverage:
- Not supported.

Modules and existing getters:
- None.

New oylapi getters needed:
- `oyl_get_all_wraps(count, offset)`.

Missing indices / implementation path:
- Add `wrap_events_all/<ts>/<seq>` index.

### /get-all-unwrap-history
Overview: Global unwrap event stream.

Coverage:
- Not supported.

Modules and existing getters:
- None.

New oylapi getters needed:
- `oyl_get_all_unwraps(count, offset)`.

Missing indices / implementation path:
- Add `unwrap_events_all/<ts>/<seq>` index.

### /get-total-unwrap-amount
Overview: Total unwrap amount (optionally filtered by height).

Coverage:
- Not supported.

Modules and existing getters:
- None.

New oylapi getters needed:
- `oyl_get_total_unwrap_amount(block_height?)`.

Missing indices / implementation path:
- Add `unwrap_total_by_height` or rolling accumulator in essentials.
- For height cutoff, store cumulative totals at block boundaries.

### /get-address-pool-creation-history
Overview: Pool creation events by address.

Coverage:
- Not supported (address missing).

Modules and existing getters:
- Ammdata activity (no address).

New oylapi getters needed:
- `oyl_get_address_pool_creations(address, count, offset)`.

Missing indices / implementation path:
- Add `ammdata/address_pool_creations/<address>/<ts>/<seq>` index.

### /get-address-pool-mint-history
Overview: Liquidity add events by address.

Coverage:
- Not supported (address and amounts missing).

Modules and existing getters:
- Ammdata activity (insufficient).

New oylapi getters needed:
- `oyl_get_address_pool_mints(address, count, offset)`.

Missing indices / implementation path:
- Add `ammdata/address_pool_mints/<address>/<ts>/<seq>` index with amounts.

### /get-address-pool-burn-history
Overview: Liquidity remove events by address.

Coverage:
- Not supported (address and amounts missing).

Modules and existing getters:
- Ammdata activity (insufficient).

New oylapi getters needed:
- `oyl_get_address_pool_burns(address, count, offset)`.

Missing indices / implementation path:
- Add `ammdata/address_pool_burns/<address>/<ts>/<seq>` index with amounts.

### /get-all-address-amm-tx-history
Overview: Unified AMM tx history for an address (swap/mint/burn/creation/wrap/unwrap).

Coverage:
- Not supported without address-aware indices in both ammdata and wrap indexes.

Modules and existing getters:
- Ammdata activity (no address) and no wrap indexes.

New oylapi getters needed:
- `oyl_get_address_amm_history(address, type?, count, offset)` that merges multiple indices.

Missing indices / implementation path:
- Add address-specific indexes for swap/mint/burn/creation in ammdata.
- Add wrap/unwrap address indexes in essentials.
- Add a unified `amm_address_history` index if ordering across types is required without merging at query time.

### /get-all-amm-tx-history
Overview: Unified AMM tx history across all addresses.

Coverage:
- Not supported without a global combined AMM stream.

Modules and existing getters:
- Ammdata activity (per-pool only).

New oylapi getters needed:
- `oyl_get_all_amm_history(type?, count, offset)`.

Missing indices / implementation path:
- Add `ammdata/amm_history_all/<ts>/<seq>` index that stores normalized event records.
- Optionally add per-type indexes for filtering without scan.

### /get-all-token-pairs
Overview: List all pools as token pairs (with optional TVL/volume if present).

Coverage:
- Partially supported. Pools exist, but token metadata and TVL/volume require aggregates.

Modules and existing getters:
- Ammdata `get_reserves_snapshot` for pool list and reserves.
- Essentials token metadata for name/symbol/decimals.

New oylapi getters needed:
- `oyl_list_all_token_pairs(factory_id)` from pool snapshot + token metadata.

Missing indices / implementation path:
- Add `pool_metrics` (TVL/volume) to fill optional fields in `TokenPair`.
- Add `factory_pools` to honor factoryId filtering.

### /get-token-pairs
Overview: Token pairs for a specific token (pool list filtered by token).

Coverage:
- Not supported without token->pool index.

Modules and existing getters:
- Ammdata reserves snapshot (O(n) scan; not ideal).

New oylapi getters needed:
- `oyl_get_token_pairs(factory_id, token_id, sort_by, limit, offset, searchQuery)`.

Missing indices / implementation path:
- Add `ammdata/token_pools/<token>/<pool>` index.
- Add `pool_name_index` for searchQuery filtering.
- Add `pool_metrics` or `token_pair_sort` indexes for sorting by TVL.

### /get-alkane-swap-pair-details
Overview: Best swap paths between two tokens.

Coverage:
- Mostly supported by ammdata pathfinder once token->pool graph exists; may need pool graph cache.

Modules and existing getters:
- Ammdata pathfinder (`plan_*` in `utils/pathfinder.rs`).
- Ammdata reserves snapshot for pool graph.

New oylapi getters needed:
- `oyl_find_swap_paths(factory_id, tokenA, tokenB)` wrapping ammdata pathfinder.

Missing indices / implementation path:
- Ensure ammdata maintains a fast in-memory or cached adjacency graph using `token_pools` index.
- Add `pool_graph_version` or cache invalidation based on ammdata index height.

## Notes on module placement for new indices

- Ammdata is the best home for pool- and swap-related aggregates, because it already processes pool events and maintains pool-level activity.
- Essentials is the best home for address balances, outpoints, and (likely) wrap/unwrap events since it has global traces and storage change access.
- The oylapi module should only expose getters that compose essentials and ammdata data; heavy indexing should remain in the source modules to avoid duplication.

