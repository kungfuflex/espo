# Oylapi Endpoints 26-Remaining Implementation Plan

This file covers the remaining endpoints after 21-25 from `agent-context/oyl-api.ts`.
It follows the same structure and assumptions as:
- `context/oylapi-migration-endpoints/1-5.md`
- `context/oylapi-migration-endpoints/6-10.md`
- `context/oylapi-migration-endpoints/11-15.md`
- `context/oylapi-migration-endpoints/16-20.md`
- `context/oylapi-migration-endpoints/21-25.md`

Existing indices we can already piggyback on:
- Ammdata:
  - `activity:v1` (with `address_spk` + `success`)
  - `pool_creations`, `token_swaps`, `address_pool_swaps`, `address_token_swaps`
  - `address_pool_creations`, `address_pool_mints`, `address_pool_burns`
  - `pool_metrics`, `pools`, `factory_pools`, `pool_name_index`
- Essentials:
  - creation records for token metadata (name/symbol/decimals).
- Subfrost:
  - `wrap_events_all`, `unwrap_events_all`
  - `wrap_events_by_address`, `unwrap_events_by_address`
  - `unwrap_total_latest`, `unwrap_total_by_height`

---

### /get-all-address-amm-tx-history

#### Goal
Return unified AMM tx history for a single address across swaps, mints, burns, creations, wraps, and unwraps.

#### Coverage
- Partially supported: address-specific indices exist for swaps/mints/burns/creations and wrap/unwrap.
- Missing: unified ordering across all event types.

#### Inputs and validation
- `address` required.
- `type` optional (swap/mint/burn/creation/wrap/unwrap).
- `count` defaults to 50, clamp 1..200.
- `offset` defaults to 0.
- `successful` defaults to false.
- `includeTotal` defaults to true.

#### Primary data sources
- Ammdata:
  - `address_pool_swaps`, `address_pool_mints`, `address_pool_burns`, `address_pool_creations`
  - `activity:v1` for details per `<pool>/<ts>/<seq>`
- Subfrost:
  - `wrap_events_by_address`, `unwrap_events_by_address`

#### New indices required
- Ammdata: `address_amm_history/v1/<address_spk>/<ts>/<seq>/<pool>/<kind>` -> empty
  - Written once per activity event (swap/mint/burn/create).

#### Implementation plan (write path)
1. When writing activity entries, also write `address_amm_history` keyed by address + timestamp.
2. Leave wrap/unwrap in subfrost; oylapi will merge with ammdata stream.

#### Implementation plan (read path)
1. Convert address to SPK bytes.
2. If `type` is provided:
   - Use the corresponding index only (e.g., swaps -> `address_pool_swaps`).
3. If `type` is not provided:
   - Page `address_amm_history` and fetch activity entries.
4. Merge with subfrost wrap/unwrap events (same k-way merge by timestamp).
5. Map to `AmmTxHistoryItem` union:
   - Swap -> `AmmSwapHistoryItem`
   - Mint -> `AmmMintHistoryItem`
   - Burn -> `AmmBurnHistoryItem`
   - Creation -> `AmmCreationHistoryItem`
   - Wrap/Unwrap -> `WrapHistoryItem`
6. Filter by `successful` if provided.

#### Missing getters to add (oylapi)
- `get_all_address_amm_tx_history(address, type?, count, offset, include_total, successful?)`.

---

### /get-all-amm-tx-history

#### Goal
Return unified AMM tx history across all addresses.

#### Coverage
- Partially supported: pool activity exists but no global activity index.
- Wrap/unwrap are in subfrost only.

#### Inputs and validation
- `type` optional (swap/mint/burn/creation/wrap/unwrap).
- `count` defaults to 50, clamp 1..200.
- `offset` defaults to 0.
- `successful` defaults to false.
- `includeTotal` defaults to true.

#### Primary data sources
- Ammdata:
  - `activity:v1` per pool (no global index).
- Subfrost:
  - `wrap_events_all`, `unwrap_events_all`.

#### New indices required
- Ammdata: `amm_history_all/v1/<ts>/<seq>/<pool>/<kind>` -> empty.
  - Written once per activity event (swap/mint/burn/create).

#### Implementation plan (write path)
1. When writing activity entries, also write `amm_history_all` keyed by timestamp.

#### Implementation plan (read path)
1. If `type` provided:
   - Filter by `kind` in `amm_history_all` (or use per-type prefix).
2. Merge `amm_history_all` with subfrost wrap/unwrap streams by timestamp.
3. Map to `AmmTxHistoryItem` union.
4. Filter by `successful` if provided.

#### Missing getters to add (oylapi)
- `get_all_amm_tx_history(type?, count, offset, include_total, successful?)`.

---

### /get-all-token-pairs

#### Goal
List all pools as token pairs (for a factory), with optional TVL/volume fields.

#### Coverage
- Partially supported: pools + metrics exist in ammdata, metadata in essentials.

#### Inputs and validation
- `factoryId` required.

#### Primary data sources
- Ammdata:
  - `factory_pools` for pool ids.
  - `pools` for base/quote ids.
  - `pool_metrics` for TVL and volume.
- Essentials:
  - creation records for token metadata (name/symbol/decimals).

#### New indices required
- None if using `factory_pools` + `pools` + `pool_metrics`.
- If performance is an issue, add `token_pairs_all` cache index.

#### Implementation plan (read path)
1. Fetch pool ids via `factory_pools`.
2. For each pool:
   - Load `pools` (base/quote ids).
   - Load pool metrics for TVL/volume fields.
   - Load token metadata from essentials.
   - Load pool balances from essentials for `reserve0`/`reserve1`.
3. Build `TokenPair` entries with:
   - `poolId`, `poolName`, `poolTvlInUsd`, `poolVolume1dInUsd`, `reserve0`, `reserve1`
   - `token0`/`token1` with metadata.
4. Return list (no paging in spec).

#### Missing getters to add (oylapi)
- `get_all_token_pairs(factory_id)`.

---

### /get-token-pairs

#### Goal
List all pools that include a specific token (filtered by factory).

#### Coverage
- Partially supported: pools exist but token->pool index not yet defined.

#### Inputs and validation
- `factoryId` required.
- `alkaneId` required.
- `sort_by` supports only `tvl`.
- `limit` and `offset` optional.
- `searchQuery` optional.

#### Primary data sources
- Ammdata:
  - `token_pools` (new index).
  - `factory_pools`, `pools`, `pool_metrics`, `pool_name_index`.
- Essentials:
  - creation records for metadata.

#### New indices required
- Ammdata: `token_pools/v1/<token>/<pool> -> empty` (base + quote).
- Optional `token_pools_by_factory` if factory filtering is heavy.

#### Implementation plan (write path)
1. When pool defs are written, insert `token_pools` entries for base and quote tokens.

#### Implementation plan (read path)
1. Fetch pools for token via `token_pools`.
2. Filter by factory using `factory_pools`.
3. Apply `searchQuery` using `pool_name_index` or exact id match.
4. Sort by `pool_metrics.pool_tvl_usd` when `sort_by=tvl`.
5. Map to `TokenPair` with metadata and metrics, using pool balances (essentials) for reserves.

#### Missing getters to add (oylapi)
- `get_token_pairs(factory_id, token_id, sort_by, limit, offset, search_query)`.

---

### /get-alkane-swap-pair-details

#### Goal
Return swap paths between tokenA and tokenB (best routes).

#### Coverage
- Mostly supported via ammdata pathfinder.

#### Inputs and validation
- `factoryId` required.
- `tokenAId` and `tokenBId` required.

#### Primary data sources
- Ammdata:
  - `rpc_find_best_swap_path` (pathfinder).
  - pool graph built from reserves snapshot.

#### New indices required
- None if pathfinder uses live reserves snapshot.
- If pathfinder performance is slow, cache adjacency graph keyed by ammdata index height.

#### Implementation plan (read path)
1. Restrict pool graph to `factoryId` using `factory_pools`.
2. Call ammdata `rpc_find_best_swap_path` with:
   - `token_in = tokenAId`, `token_out = tokenBId`.
   - Use default fee and hop limits unless overridden.
3. Convert to `SwapPath[]` with:
   - `path` (token route).
   - `pools` (token pair list for each hop).
4. Return empty list if no route found.

#### Missing getters to add (oylapi)
- `get_alkane_swap_pair_details(factory_id, token_a, token_b)`.

---

## Index definitions summary (new in 26-remaining)

### Ammdata
- `address_amm_history/v1/<address_spk>/<ts>/<seq>/<pool>/<kind>` (optional but recommended)
- `amm_history_all/v1/<ts>/<seq>/<pool>/<kind>` (optional but recommended)
- `token_pools/v1/<token>/<pool>`

### Subfrost
- No new indices beyond those already created for unwrap totals.
