# Overview

## What is Espo

### Espo Modules

Espo is a modular indexer for Alkanes. By "modular", it works kind of like metashrew (where people write wasm modules and metashrew indexes these and handles reorgs, and just passes in block data). On espo, this is referre to as an "EspoModule" which must define an index_block function which is passed in block data and Alkanes traces for that block during indexing.

This index_block function is called by the core indexer coordinating the espo modules. Espo Modules must take in the block data and the incoming traces, index them as per the necessities of the module require, and store results in their own keyspace on the global rocksdb database. For example, the essentials module gets and stores data with the prefix **"essentials:"**, ammdata with the prefix **"ammdata:"**, and so on.

Furthermore every module keeps track of its owned indexed height, which must be made public through the get_indexed_height function defined on the EspoModule trait. To see all the things that the EspoModule trait requires, check out **src/modules/defs.rs** .

The core indexer process of Espo runs on **main.rs** and traces are fetched from **metashrew/traces.rs** . Currently theres three modules implemented on Espo:

##### EspoModule Trait Methods

Trait definition: `src/modules/defs.rs` (`trait EspoModule`)

- `get_name(&self) -> &'static str`
- `set_mdb(&mut self, mdb: Arc<Mdb>) -> ()`
- `get_genesis_block(&self, network: Network) -> u32`
- `index_block(&self, block: EspoBlock) -> Result<()>`
- `get_index_height(&self) -> Option<u32>`
- `register_rpc(&self, reg: &RpcNsRegistrar) -> ()`

Type references:

- `EspoBlock`: `src/alkanes/trace.rs`
- `Mdb`: `src/runtime/mdb.rs`
- `RpcNsRegistrar`: `src/modules/defs.rs`
- `Network`: `bitcoin::Network`
- `Result`: `anyhow::Result`

#### MODULE: essentials (special)

This is a core module that must be bundled with espo as it indexes, as the name implies, essential data. However, this module is special because all other modules on espo depend on it (hence why its required to be bunded with espo and why it must be the first module to be initialized). The ordering of other modules does not matter, but essentials must be before all of them. All modules are initialized in **main.rs** , and as you can see essentials is initalized before all of them.

The essentials module (located in **/modules/essentials/** ) is incharge of the following indicies:

##### Indices

- AlkaneId + Storage Key -> Stored Value (contract storage)
- AlkaneId -> Storage Key Directory Listing
- AlkaneId -> Holders
- AlkaneId -> Holder Count
- Holder Count -> AlkaneId (ordered by holders)
- Address -> AlkaneUtxos (including spent) + Balances
- AlkaneUtxo -> AlkaneBalances
- AlkaneUtxo -> Address
- AlkaneUtxo -> ScriptPubKey
- Address -> ScriptPubKey (cached)
- Address -> AlkaneTransactions
- AlkaneId -> AlkaneBalances (for contracts)
- AlkaneId -> Balance-Change Transactions
- (Owner AlkaneId, Token AlkaneId) -> Balance-Change Transactions
- Block Height -> Balance-Change Transactions
- Alkane Transaction -> Summary (traces/outflows)
- Block -> AlkaneTransactions (txids + length)
- Name (prefix) -> AlkaneId
- AlkaneId -> Name/Symbol
- AlkaneId -> Contract Wasm Inspection Result
- AlkaneId -> Creation Record (txid/height/timestamp)
- Creation Order -> AlkaneId
- Creation Count -> Total Alkane Count
- Block -> Block Summary (trace count + header)
- Global -> Latest Traces

##### K/V storage format for Essentials Module

All keys below are stored under the `essentials:` prefix in RocksDB. Unless noted,
txids are stored as raw `bitcoin::Txid::to_byte_array()` bytes (little-endian);
explorers typically reverse to display.

Key notation:

- `u32be`, `u64be` = big-endian integer bytes in key
- `u32le`, `u64le` = little-endian integer bytes in value
- `borsh(T)` = Borsh-serialized type `T`

Common types (Borsh):

- Schema definitions live in:
  - `src/schemas.rs` (SchemaAlkaneId, EspoOutpoint)
  - `src/modules/essentials/storage.rs` (BalanceEntry, HolderEntry, AlkaneBalanceTxEntry, AlkaneTxSummary, BlockSummary, AlkaneInfo helpers)
  - `src/modules/essentials/utils/inspections.rs` (StoredInspectionResult, AlkaneCreationRecord)
  - `src/modules/essentials/utils/balances/defs.rs` (SignedU128)
- `SchemaAlkaneId`: `{ block: u32, tx: u64 }`
- `EspoOutpoint`: `{ txid: Vec<u8>, vout: u32, tx_spent: Option<Vec<u8>> }`
- `BalanceEntry`: `{ alkane: SchemaAlkaneId, amount: u128 }`
- `HolderEntry`: `{ holder: HolderId, amount: u128 }`
- `HolderId`: `Address(String) | Alkane(SchemaAlkaneId)`
- `SignedU128`: `{ negative: bool, amount: u128 }`
- `AlkaneBalanceTxEntry`: `{ txid: [u8;32], height: u32, outflow: BTreeMap<SchemaAlkaneId, SignedU128> }`
- `AlkaneTxSummary`: `{ txid: [u8;32], traces: Vec<EspoSandshrewLikeTrace>, outflows: Vec<AlkaneBalanceTxEntry>, height: u32 }`
- `AlkaneCreationRecord`: `{ alkane, txid, creation_height, creation_timestamp, tx_index_in_block, inspection, names, symbols }`
- `StoredInspectionResult`: `{ alkane, bytecode_length, metadata, metadata_error, factory_alkane }`
- `BlockSummary`: `{ trace_count: u32, header: Vec<u8> }`

Contract storage rows (per Alkane):

- `0x01 | block_be(4) | tx_be(8) | key_len_be(2) | key_bytes`
  -> `txid_le(32) | value_bytes` (last writer txid + raw storage value)
- `0x03 | block_be(4) | tx_be(8) | key_len_be(2) | key_bytes`
  -> empty (directory marker used by `get_keys`)

Index metadata:

- `/index_height` -> `u32le` (last indexed block height)

Balances and UTXOs:

- `/balances/{address}/{borsh(EspoOutpoint)}` -> `borsh(Vec<BalanceEntry>)`
- `/outpoint_balances/{borsh(EspoOutpoint)}` -> `borsh(Vec<BalanceEntry>)`
- `/outpoint_addr/{borsh(EspoOutpoint)}` -> UTF-8 address string
- `/utxo_spk/{borsh(EspoOutpoint)}` -> ScriptPubKey raw bytes
- `/addr_spk/{address}` -> ScriptPubKey raw bytes (cached)

Holders:

- `/holders/{alkane block_be}{alkane tx_be}` -> `borsh(Vec<HolderEntry>)`
- `/holders/count/{alkane block_be}{alkane tx_be}` -> `borsh({ count: u64 })`
- `/alkanes/holders/ordered/{count_be(8)}{alkane block_be}{alkane tx_be}` -> empty

Alkane balances + deltas:

- `/alkane_balances/{owner block_be}{owner tx_be}` -> `borsh(Vec<BalanceEntry>)`
- `/alkane_balance_txs/{alkane block_be}{alkane tx_be}` -> `borsh(Vec<AlkaneBalanceTxEntry>)`
- `/alkane_balance_txs_by_token/{owner block_be}{owner tx_be}/{token block_be}{token tx_be}`
  -> `borsh(Vec<AlkaneBalanceTxEntry>)`
- `/alkane_balance_txs_by_height/{height_be}` -> `borsh(Vec<AlkaneBalanceTxEntry>)`

Transactions + address/block indexes:

- `/alkane_tx_summary/{txid_bytes}` -> `borsh(AlkaneTxSummary)`
- `/alkane_block/{height_be}/{idx_be}` -> `txid_bytes`
- `/alkane_block/{height_be}/length` -> `u64le`
- `/alkane_addr/{address}/{idx_be}` -> `txid_bytes`
- `/alkane_addr/{address}/length` -> `u64le`
- `/alkane_latest_traces` -> `borsh(Vec<[u8;32]>)` (most recent trace txids, capped at 20)

Names + creation records:

- `/alkanes/name/{name}/{alkane block_be}{alkane tx_be}` -> empty
- `/alkanes/creation/id/{alkane block_be}{alkane tx_be}` -> `borsh(AlkaneCreationRecord)`
- `/alkanes/creation/ordered/{ts_be}{height_be}{tx_index_be}{alk_block_be}{alk_tx_be}`
  -> `borsh(AlkaneCreationRecord)`
- `/alkanes/creation/count` -> `u64le`

Block summaries:

- `/block_summary/{height_be}` -> `borsh(BlockSummary)`

#### MODULE: ammdata

The amm data is incharge of indexing trades and candles (in the form of OHLCV -> open, high, low, close, volume) for alkanes traded on OylAMM. It depends on the essentials moodule to get the alkane balance outflows for all transactions in the current block, which from it uses to determine if its a pool and if it is, uses it to index liquidity adds (twin positive outflows), liquidity removes (twin negative outflows), and trades (one positive, one negative outflow). From these it indexes trades and candles for the alkanes traded on OylAMM.

The ammdata module is also incharge of indexing current pools. It does this by checking if the factory contract is the OylAMM factory prxoy target. To get this information it relies on the essentials module, whom for **AlkaneCreationRecord** (which is served on essentials.get_alkane_info rpc method) stores the factory alkane (among other things). From this its able to create and maintain a current list of OylAMM pools in an O(1) index.

##### Indices

- Pool AlkaneId -> Candles (OHLCV) per timeframe + bucket
- Pool AlkaneId -> Activity events/trades (trade, liquidity, pool create)
- Pool AlkaneId -> Activity sort indexes (ts/amount/side)
- Pool AlkaneId -> Activity counts (all, trades, events)
- Global -> Reserves snapshot for all pools (latest reserves + base/quote ids)
- Module -> Index height

##### K/V storage format for Ammdata Module

All keys below are stored under the `ammdata:` prefix in RocksDB.

Key notation:

- `u32be`, `u64be`, `u128be` = big-endian integer bytes in key
- `u32le`, `u64le` = little-endian integer bytes in value
- `borsh(T)` = Borsh-serialized type `T`

Schema definitions live in:

- `src/modules/ammdata/schemas.rs` (SchemaCandleV1, SchemaFullCandleV1, SchemaActivityV1, SchemaPoolSnapshot, SchemaReservesSnapshot, ActivityKind/Direction, Timeframe)
- `src/schemas.rs` (SchemaAlkaneId)

Common types (Borsh):

- `SchemaCandleV1`: `{ open, high, low, close, volume }` (u128s, prices scaled by PRICE_SCALE = 1e8)
- `SchemaFullCandleV1`: `{ base_candle: SchemaCandleV1, quote_candle: SchemaCandleV1 }`
- `SchemaActivityV1`: `{ timestamp: u64, txid: [u8;32], kind, direction, base_delta: i128, quote_delta: i128 }`
- `SchemaPoolSnapshot`: `{ base_reserve: u128, quote_reserve: u128, base_id, quote_id }`
- `SchemaReservesSnapshot`: `{ entries: BTreeMap<SchemaAlkaneId, SchemaPoolSnapshot> }`

Index metadata:

- `/index_height` -> `u32le` (last indexed block height)

Candles (per pool/timeframe/bucket):

- `fc1:{pool_block_hex}:{pool_tx_hex}:{tf_code}:{bucket_ts}` -> `borsh(SchemaFullCandleV1)`
- `tf_code` = `10m|1h|4h|1d|1w|1M`
  - `bucket_ts` = bucket start timestamp in seconds (decimal string)
  - `pool_*_hex` are lowercase hex (no `0x`)

Activity (primary rows):

- `activity:v1:{pool_block_dec}:{pool_tx_dec}:{ts}:{seq}` -> `borsh(SchemaActivityV1)`
  - `ts` = unix timestamp (seconds, decimal string)
  - `seq` = per-(pool,ts) sequence (decimal string)

Activity indexes (secondary rows, values = `ts_be(8) | seq_be(4)`):

- `activity:idx:v1:{pool_block_dec}:{pool_tx_dec}:ts:{ts_be}{seq_be}`
- `activity:idx:v1:{pool_block_dec}:{pool_tx_dec}:absb:{abs_base_be}{ts_be}{seq_be}`
- `activity:idx:v1:{pool_block_dec}:{pool_tx_dec}:absq:{abs_quote_be}{ts_be}{seq_be}`
- `activity:idx:v1:{pool_block_dec}:{pool_tx_dec}:sb_ts:{side_byte}{ts_be}{seq_be}`
- `activity:idx:v1:{pool_block_dec}:{pool_tx_dec}:sq_ts:{side_byte}{ts_be}{seq_be}`
- `activity:idx:v1:{pool_block_dec}:{pool_tx_dec}:sb_absb:{side_byte}{abs_base_be}{ts_be}{seq_be}`
- `activity:idx:v1:{pool_block_dec}:{pool_tx_dec}:sq_absq:{side_byte}{abs_quote_be}{ts_be}{seq_be}`

Activity group indexes (same as above, but scoped):

- `activity:idx:v1:{pool_block_dec}:{pool_tx_dec}:trades:...`
- `activity:idx:v1:{pool_block_dec}:{pool_tx_dec}:events:...`

Activity counts:

- `activity:idx:v1:{pool_block_dec}:{pool_tx_dec}:__count` -> `u64be`
- `activity:idx:v1:{pool_block_dec}:{pool_tx_dec}:trades:__count` -> `u64be`
- `activity:idx:v1:{pool_block_dec}:{pool_tx_dec}:events:__count` -> `u64be`

Reserves snapshot (all pools):

- `/reserves_snapshot_v1` -> `borsh(SchemaReservesSnapshot)`

### MODULE: pizzafun

This is a very simple informatary module that stores no indicies and doesnt touch the database at all. While this module will grow over time for indicies pizza.fun requires, for now its only purpose keeping in memory a map of

seriesId -> AlkaneId (and reverse)

This is because pizza.fun's infrastructure maps to seriesids, not alkaneIds due to its protocol agnostic nature. You dont need to worry about this module.

## Core Traits

Theres three main traits you need to be aware of when working with Espo that define 90% of the codebase:

### The EspoModule trait

Trait definition: `src/modules/defs.rs` (`trait EspoModule`)

Methods defined:

- `get_name(&self) -> &'static str`
- `set_mdb(&mut self, mdb: Arc<Mdb>) -> ()`
- `get_genesis_block(&self, network: Network) -> u32`
- `index_block(&self, block: EspoBlock) -> Result<()>`
- `get_index_height(&self) -> Option<u32>`
- `register_rpc(&self, reg: &RpcNsRegistrar) -> ()`

Type references:

- `EspoBlock`: `src/alkanes/trace.rs`
- `Mdb`: `src/runtime/mdb.rs`
- `RpcNsRegistrar`: `src/modules/defs.rs`
- `Network`: `bitcoin::Network`
- `Result`: `anyhow::Result`

### The ElectrumLike trait

Espos explorer is located in the **explorer/** directory. It is a fully fledged explorer for alkanes that uses data from espo's indicies (specifically the essentials module), electrs (via the ElectrumLike trait, which is implemented in **src/utils/electrum_like.rs** ) and bitcoin rpc.

The **ElectrumLike** trait is an abstraction layer which the codebase depends on where electrs is called. It defines a simple interface which we define both for Electrum RPC and for the Electrs Esplora REST api. This allows the consumer of espo to choose whether they want to use electrs or electrum for espo, and espo will adapt to this.

Trait definition: `src/utils/electrum_like.rs` (`trait ElectrumLike`)

Methods defined:

- `batch_transaction_get_raw(&self, txids: &[Txid]) -> Result<Vec<Vec<u8>>>`
- `transaction_get_raw(&self, txid: &Txid) -> Result<Vec<u8>>`
- `tip_height(&self) -> Result<u32>`
- `transaction_get_outspends(&self, txid: &Txid) -> Result<Vec<Option<Txid>>>`
- `batch_transaction_get_outspends(&self, txids: &[Txid]) -> Result<Vec<Vec<Option<Txid>>>>`
- `transaction_get_height(&self, txid: &Txid) -> Result<Option<u64>>`
- `address_stats(&self, address: &Address) -> Result<AddressStats>`
- `address_history_page(&self, address: &Address, offset: usize, limit: usize) -> Result<AddressHistoryPage>`
- `address_history_page_cursor(&self, address: &Address, cursor: Option<&Txid>, limit: usize) -> Result<AddressHistoryPage>`

Type references:

- `Txid`, `Address`: `bitcoin`
- `AddressStats`, `AddressHistoryPage`: `src/utils/electrum_like.rs`
- `Result`: `anyhow::Result`

### The MetashrewAdapter trait and metashrews secondary db

The MetashrewAdapter trait is defined in **src/alkanes/metashrew.rs** and is an abstraction layer for reading metashrew's secondary db.

All data that espo uses to generate an **EspoBlock** in **src/alkanes/trace** related to alkanes is fetched from metashrew's local rocksdb database. We, however, cannot read metashrew's rocksdb database directly as it would likely corrupt metashrew's own operations and db. The reason for this is just OPENING a rocksdb database creates a lock file and logs. AKA - it writes to the metashrew db folder which we absolutely do not want to do.

To solve this Espo reads the metashrew db through an abstraction later rocksdb provides called a "Secondary DB". This is basically a proxy db to metashrew's db, but the lock files and logs are written in a folder Espo controls. This allows Espo to read metashrew's db safely.

MetashrewAdapter is a trait that defines an interface which directly communicated with the metashrew secondary db (known in the code as **SDB**). MetashrewAdapter also is aware of metashrew's on reorg-aware schema - basically works like this:

Every key on metashrew is suffixed by a /length and /idx. When a reorg happens because metashrew is an append only database, it increaeses the length key by one and writes new values to a new key with ah higher idx. For exmaple lets say metashrew indexes block 20, but then block 20 reorgs, now we have an invalid state on metashrew because we have a block saved on our database that is not on the main chain anymore. This is what causes divergences. To fix this, metashrew reindexes block 20 but now all keys the reindex affects are written to a new idx, and the length is increased by one. This way, the old data is still there (in case of another reorg back to the old chain), but metashrew only reads the latest idx for any key (the one with the highest idx value).

MetashrewAdapter abstracts this logic away and also abstracts metashrews own db schema away so that espo can just call simple functions like get_alkane_traces(txid) and get_block_header(height) without worrying about how metashrew stores this data.

Methods defined (MetashrewAdapter struct):

- `new(label: Option<String>) -> MetashrewAdapter`
- `get_alkane_wasm_bytes_with_db(&self, db: &SDB, alkane: &SchemaAlkaneId) -> Result<Option<(Vec<u8>, SchemaAlkaneId)>>`
- `get_alkane_wasm_bytes(&self, alkane: &SchemaAlkaneId) -> Result<Option<(Vec<u8>, SchemaAlkaneId)>>`
- `get_alkanes_tip_height(&self) -> Result<u32>`
- `traces_for_tx(&self, txid: &Txid) -> Result<Vec<PartialEspoTrace>>`
- `traces_for_tx_with_db(&self, db: &SDB, txid: &Txid) -> Result<Vec<PartialEspoTrace>>`
- `get_reserves_for_alkane_with_db(&self, db: &SDB, who_alkane: &SchemaAlkaneId, what_alkane: &SchemaAlkaneId, height: Option<u64>) -> Result<Option<u128>>`
- `get_reserves_for_alkane(&self, who_alkane: &SchemaAlkaneId, what_alkane: &SchemaAlkaneId, height: Option<u64>) -> Result<Option<u128>>`
- `traces_for_block_as_prost(&self, block: u64) -> Result<Vec<PartialEspoTrace>>`
- `traces_for_block_as_prost_with_db(&self, db: &SDB, block: u64) -> Result<Vec<PartialEspoTrace>>`

Type references:

- `SDB`: `src/runtime/sdb.rs`
- `SchemaAlkaneId`: `src/schemas.rs`
- `PartialEspoTrace`: `src/alkanes/trace.rs`
- `Txid`: `bitcoin`
- `Result`: `anyhow::Result`

## The Espo Explorer

The Espo Explorer is a fully fledged Alkanes explorerthat server-side rendered (SSR) and built with Axum + Maud (for templating). It lives under
`src/explorer/` and is optional at runtime (started only when `explorer_host` is set).

Everything the explorer serves is fetched from Espos indicies (mainly essentials), electrs (via ElectrumLike), and the bitcoind rpc. For some stuff it also directly calls metashrew (specifically for the inspect_contract ui where we must call `simulate` opcodes).

Essentially the explorer is meant as a looking glass into the index Espo builds and to metashrews on indicies.

The explorer runs on a sperate thread from the indexer. To debug you can turn on the 'view_only' option on Espo's config (see **config.json**) which will start only the explorer and all module's rpc servers, but not the indexer.

Key locations:

- Router + server entry: `src/explorer/mod.rs` (`explorer_router`, `run_explorer`)
- Pages (SSR handlers): `src/explorer/pages/` (`home`, `block`, `tx`, `address`, `alkane`, `alkanes`, `search`)
- Shared UI: `src/explorer/components/` (layout, header, tables, tx view, SVGs)
- JSON endpoints: `src/explorer/api.rs` (`/api/blocks/carousel`, `/api/search/guess`, `/api/alkane/simulate`)
- Path helpers: `src/explorer/paths.rs` (`explorer_path`, `explorer_base_path`)
- CSS: `src/explorer/assets/style.css` (served via `components/layout.rs::style`)

Routes (SSR):

- `/` -> `pages/home.rs::home_page`
- `/search` -> `pages/search.rs::search`
- `/block/{height}` -> `pages/block.rs::block_page`
- `/tx/{txid}` -> `pages/tx.rs::tx_page`
- `/address/{address}` -> `pages/address.rs::address_page`
- `/alkane/{alkane}` -> `pages/alkane.rs::alkane_page`
- `/alkanes` -> `pages/alkanes.rs::alkanes_page`
- `/static/style.css` -> `components/layout.rs::style`

Data dependencies:

- Essentials DB (`essentials:`) via `ExplorerState` in `src/explorer/pages/state.rs`
- Electrum-like client (`ElectrumLike`) for raw txs, outspends, and address history
- Bitcoind RPC for block/tx metadata
- Metashrew adapter for traces and `/api/alkane/simulate`
- Mempool overlays from `src/runtime/mempool.rs`

Config knobs:

- `explorer_host` enables the server (see `src/main.rs`)
- `explorer_base_path` supports reverse-proxy mounting; always use `explorer_path(...)`
- `explorer_networks` controls the network switcher in the header

Adding features:

- New page: add handler in `src/explorer/pages/`, register route in `explorer_router`
- Shared UI: add or update components in `src/explorer/components/`
- New JSON endpoint: add handler in `src/explorer/api.rs` and route in `explorer_router`

## Reorgs

Espo is reorg-aware. Even though metashrew handles reorgs for its own database, espo might read a reorged block from metashrew and must handle this itself.

This is because for example lets say metashrew indexes block 20, then espo reads metashrew and uses its index to build block 20, then block 20 reorgs and metashrew reindexes block 20 - but espo still has the OLD block 20 on its db. This is a divergence and must be handled.

To circumvent reorgs espo mantains an AOF db (seperate from the espo db in the **./db folder (default)**). AOF stands for "append only file". The easiest way to thinka bout it is like this:

Every call to mdb (this is the main wrapper of rocksdb) logs the put or delete operation attempting to be called before writing to db. Because MDB is the abstraction layer used everywhere on Espo, this means ALL writes to the espo db are logged to the AOF (its basically a giant undo history log).

The AOF is expensive so we only keep on there the latest N blocks (default is 100 but could be as low as 6, which is industry standard for reorg safety). When espo detects a reorg, it goes through the AOF file bakcwards and undos every single key change to the beginning of SAFETIP - N blocks (safetip is the latest block espo has indexed).

Then it reindexes from that height and by the time espo is done reindexing, it will have reindexed the reorged blocks and would be back on the main chain.

All of AOF is implemented in **src/runtime/aof.rs** and is used by the **Mdb** struct in **src/runtime/mdb.rs**. The AOF is written to disk on every block indexed, so in case of a crash espo can recover by just reading the AOF and undoing any partial writes.
