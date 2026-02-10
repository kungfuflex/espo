# Metashrew Adapter Changes (KeyValuePointer-Based)

## Goal

Replace the custom RocksDB/SDB parsing logic in the adapter with a
KeyValuePointer-backed reader so the native service can re-use the
same key layout and helpers the WASM side uses (protorune/alkanes).

## Summary of Changes

1. Add a native KeyValuePointer implementation
   - Introduce a pointer type (e.g., `SdbPointer`) that wraps:
     - `Arc<SDB>` or `&SDB`
     - `Arc<Vec<u8>>` key bytes
     - optional `label` prefix (to keep `label://` handling)
   - Implement `KeyValuePointer` for `SdbPointer`:
     - `wrap/unwrap`: store and return raw key bytes
     - `inherits`: carry the label and DB handle to child pointers
     - `get`: read from SDB and normalize the payload
       - If the value is `"height:HEX"`, strip the prefix and hex-decode
       - If the value is ASCII digits and the key ends with `/length`,
         convert to little-endian `u32` bytes so `length()` works
       - Otherwise return the raw bytes
     - `set`: no-op or write-through depending on service needs
   - This replaces `VersionedPointer`, `apply_label`, and `get_with_depth`.

2. Copy pointer-backed tables into the native binary
   - The `protorune::tables::RuneTable` and `alkanes::tables::TRACES`/`TRACES_BY_HEIGHT`
     are hard-coded to `IndexPointer` (WASM). Create native equivalents using
     the new pointer type:
     - `RuneTableNative` (same fields and prefixes, but using `SdbPointer`)
     - `TRACES_NATIVE` and `TRACES_BY_HEIGHT_NATIVE`
   - This is the “copy-paste” step from alkanes/protorune into the native service.

3. Replace manual key construction with table lookups
   - Outpoint balances:
     - Replace `outpoint_balance_prefix` + manual `runes`/`balances` scanning
       with `RuneTableNative::for_protocol(tag).OUTPOINT_TO_RUNES`.
     - Use `load_sheet(&ptr)` from `protorune::balance_sheet`.
   - Per-alkane balance lookup:
     - Use `BalanceSheet::new_ptr_backed(ptr)` and `load_balance` (uses `/id_to_balance`).
   - Alkane inventory/outpoint mappings:
     - Use the same `IndexPointer`-style pathing as `alkanes::utils`.

4. Replace trace scanning with pointer list operations
   - Current scan-based logic for `/trace/` can be simplified:
     - `TRACES_BY_HEIGHT_NATIVE.select_value(height).get_list()` yields outpoints
     - For each outpoint, fetch `TRACES_NATIVE.select(&outpoint_bytes).get()`
     - Use `decode_trace_blob` only as a compatibility shim, not the primary parser
   - This removes most of `traces_for_tx_with_db` and
     `traces_for_block_as_prost_with_db` scanning code.

5. Remove custom parsing helpers that duplicate ByteView
   - Replace:
     - `parse_ascii_or_le_u64`, `parse_ascii_or_le_usize`, `decode_u128_le`
     - `decode_alkane_id_le` / `encode_alkane_id_le`
   - With:
     - `ByteView` for u32/u64/u128
     - `protorune_support::balance_sheet::ProtoruneRuneId` / `AlkaneId` conversions
     - `consensus_encode` / `consensus_decode` for outpoints

6. Wire protocol tag once
   - Use `AlkaneMessageContext::protocol_tag()` (value is 1) as the
     protocol for `RuneTableNative::for_protocol(tag)`.
   - That removes hard-coded `/runes/proto/1/...` prefixes from adapter code.

## Concrete Mappings (Old -> New)

Outpoint balances

- Old: `outpoint_balances_from_id_to_balance` + manual list fallback
- New:
  - `let outpoint = OutPoint::new(txid, vout);`
  - `let out_bytes = consensus_encode(&outpoint)?;`
  - `let ptr = rune_table.OUTPOINT_TO_RUNES.select(&out_bytes);`
  - `let sheet = load_sheet(&ptr);`

Balance lookup by id

- Old: `get_outpoint_alkane_balance_for_id_with_db`
- New:
  - `let sheet = BalanceSheet::new_ptr_backed(ptr);`
  - `sheet.get(&protorune_id)`

Traces by height

- Old: `traces_for_block_as_prost_with_db` scanning `IteratorMode::From`
- New:
  - `TRACES_BY_HEIGHT_NATIVE.select_value(height as u64).get_list()`
  - For each outpoint, `TRACES_NATIVE.select(&outpoint).get()`

Traces by txid

- Old: `traces_for_tx_with_db` scanning both BE/LE prefixes
- New (if needed):
  - Iterate `TRACES_BY_HEIGHT` entries for the relevant height(s)
  - Filter by txid using decoded `OutPoint`
  - (Optional) keep a fallback scan for historical DBs only

## MetashrewAdapter Method-by-Method Migration

`new(label)`

- Keep the public constructor, but use it to build an `SdbPointer` factory.
- Store the normalized label on the pointer type, not on the adapter.

`next_prefix`

- Remove. This exists only to support prefix scans. The KVP approach uses
  list keys (`/length`, `/{index}`) and exact gets instead.

`apply_label`

- Move into `SdbPointer` so all key access is label-aware via `get()` and
  `select()` chaining.

`versioned_pointer`

- Replace with `SdbPointer` creation plus `keyword`/`select` chaining.
- If you still need height-stamped values, normalize them in `SdbPointer::get`.

`outpoint_balance_prefix`

- Replace with `RuneTable::for_protocol(AlkaneMessageContext::protocol_tag())`
  and use the `OUTPOINT_TO_RUNES` prefix bytes as the base pointer.
- Pattern: `let base = rune_table.OUTPOINT_TO_RUNES.unwrap();`
  then `SdbPointer::wrap(&base).select(&consensus_encode(outpoint)?)`.

`outpoint_balances_from_id_to_balance`

- Replace with `BalanceSheet::new_ptr_backed(ptr)` where `ptr` is the
  `OUTPOINT_TO_RUNES` pointer for that outpoint.
- Use `sheet.balances()` to get `(ProtoruneRuneId, u128)` and map to
  `SupportAlkaneId { block: id.block, tx: id.tx }`.
- This uses RuneTable for prefixing and KeyValuePointer for loading `/id_to_balance`.

`read_uint_key`

- Replace with a raw-key pointer helper such as `SdbPointer::from_bytes`.
- Use `get_value::<u32>()` or `get_value::<u64>()` (ByteView) directly.
- Keep raw keys for internal metadata like `__INTERNAL/height`.

`load_wasm_inner`

- Replace with the same pointer logic used by `get_alkane_binary` in alkanes:
  - Base pointer is `/alkanes/`.
  - `ptr.select(&alkane_id.into()).get()` to read payload.
  - If payload is 32 bytes, treat it as an alias id and recurse.
  - Otherwise, `gz::decompress` the payload.
- Use KeyValuePointer for reads only; keep alias cycle detection logic.

`get_alkane_wasm_bytes_with_db`

- Call the new KeyValuePointer-backed `load_wasm_inner`.
- Convert `SupportAlkaneId` to `SchemaAlkaneId` as you already do.

`get_alkane_wasm_bytes`

- Unchanged shape, just delegate to the KVP-backed version after `catch_up_now`.

`get_alkanes_tip_height`

- Replace `read_uint_key` with `SdbPointer::from_bytes(b\"__INTERNAL/height\")`.
- `get_value::<u32>()` gives the tip height (no ascii parsing required).

`traces_for_tx`

- Keep as a thin wrapper around `traces_for_tx_with_db`.
- The new implementation should prefer KVP lookups when the tx height is known.

`traces_for_tx_with_db`

- Primary path: obtain the block height externally, then use
  `TRACES_BY_HEIGHT` list to get outpoints and filter by txid.
- Read each trace via `TRACES.select(&outpoint_bytes).get()` and
  `decode_trace_blob` for compatibility.
- Keep a fallback prefix scan only if height lookup is unavailable.

`get_reserves_for_alkane_with_db`

- Replace the manual `balance_prefix` + `versioned_pointer` with:
  - `SdbPointer::from_keyword(\"/alkanes/\")`
  - `.select(&what_id.into())`
  - `.keyword(\"/balances/\")`
  - `.select(&who_id.into())`
- Use `length()` and `select_index(i).get()` to read versioned entries.
- Keep the binary search by height, but parse `height:HEX` only from
  the list entry payload, not from keys.

`get_reserves_for_alkane`

- Unchanged shape; delegate to the new KVP-backed function.

`get_outpoint_alkane_balances_with_db`

- Replace the `runes`/`balances` list scanning with:
  - `let table = RuneTable::for_protocol(AlkaneMessageContext::protocol_tag());`
  - `let base = table.OUTPOINT_TO_RUNES.unwrap();`
  - `let ptr = SdbPointer::wrap(&base).select(&consensus_encode(outpoint)?);`
  - `let sheet = load_sheet(&ptr);`
  - Map `sheet.balances()` into `(SupportAlkaneId, u128)`.

`get_outpoint_alkane_balances`

- Unchanged shape; delegate to the KVP-backed version after `catch_up_now`.

`get_outpoint_alkane_balance_for_id_with_db`

- Replace manual `/id_to_balance` key build with:
  - `let ptr = base_ptr.keyword(\"/id_to_balance\").select(&id.into());`
  - `ptr.get_value::<u128>()` (return `None` if empty).

`traces_for_block_as_prost`

- Unchanged shape; delegate to the KVP-backed version after `catch_up_now`.

`traces_for_block_as_prost_with_db`

- Replace the list scan logic with:
  - `TRACES_BY_HEIGHT` pointer for the height (list of outpoints).
  - For each outpoint, read `TRACES.select(&outpoint_bytes).get()`.
  - Decode via `decode_trace_blob` only when bytes are not raw protobuf.

## Implementation Notes

- The native `KeyValuePointer` should normalize values to raw bytes so
  existing `get_value::<T>()` and list ops behave the same as on-chain.
- If the SDB can expose raw values directly (without height prefixes),
  normalization can be simplified to a no-op.
- Keep `decode_trace_blob` only as a compatibility shim for legacy entries.

## Files to Add in the Service

- `native_index_pointer.rs` (or similar): `SdbPointer` + `KeyValuePointer` impl
- `native_tables.rs`: `RuneTableNative`, `TRACES_NATIVE`, `TRACES_BY_HEIGHT_NATIVE`
- Optional: `native_trace.rs` for minimal trace helpers built on pointers

## Expected Outcome

- The adapter no longer has to hand-build keys, parse list lengths, or
  scan RocksDB ranges; it uses the same key schema and utilities that
  the WASM indexer uses, just backed by SDB in native code.
