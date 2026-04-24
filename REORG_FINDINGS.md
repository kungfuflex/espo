# ESPO Reorg Findings

## Current conclusion

The narrow hypothesis I originally had for the reported swap was too specific and was wrong for
the exact transaction/block pair you gave me.

I verified the canonical Bitcoin block at height `945189` and then queried the old Metashrew
block-height trace path against the configured local DB (`/data/.metashrew/v903`). The reported
tx:

- `b2ba6bc1063b843ee2b74a4cb18876b08eac1a5e0548c6192dfcc9cfaf3023fe`

does appear in the old height-based trace results for block `945189`.

So the statement "Metashrew's stale `traces_for_block(height)` for that exact block omitted this
tx" is not supported by the evidence I gathered.

That does **not** clear ESPO's reorg handling. The broader issue is still real:

- ESPO was trusting Metashrew by **height**.
- Metashrew and fork-observer both detect canonicality by **block hash continuity**.
- ESPO was not proving that "Metashrew's block at height H" was the same block as "Core's block at
  height H" before consuming height-scoped Metashrew data.

That is the actual bug class I fixed.

## What I verified

### 1. The tx-specific stale-trace claim was false for block 945189

Using the configured production-style Metashrew DB:

- Core canonical hash at `945189`:
  `00000000000000000000d40d0394aba09040a20c9bbae8ea3ceadaa9ed77e7d7`
- Core reports tx `b2ba...23fe` confirmed in that block.
- The old `traces_for_block_as_prost_with_db` path also included `b2ba...23fe`.

So that exact tx/block pair does not prove "Metashrew height traces stayed stuck on the old fork".

### 2. Metashrew stores canonicality data in the way its own sync engine expects

From the attached Metashrew source:

- `rockshrew-runtime/src/storage_adapter.rs` stores block hashes at
  `"/__INTERNAL/height-to-hash/{height}"`.
- `metashrew-sync/src/sync.rs::handle_reorg` compares that stored hash to the node's
  `get_block_hash(height)` while walking backward to find the common ancestor.

That is hash-anchored reorg detection, not height-only detection.

### 3. Fork-observer uses the same basic idea

From the attached fork-observer code and README:

- it reasons about chain state via `getblockhash` / headers / tip continuity
- it does not trust height alone as a sufficient identity for a block

That is the model ESPO needed to copy more closely.

### 4. The Metashrew hash key path and byte order are correct on the live DB

I probed the local Metashrew DB directly:

- `ldb --db=/data/.metashrew/v903 --value_hex get '/__INTERNAL/height-to-hash/946112'`
  returned:
  `0x00000000000000000001388AE3E501B52AB3F0651DBB6463B2F994128DD25842`
- `bitcoin-cli getblockhash 946112` returned:
  `00000000000000000001388ae3e501b52ab3f0651dbb6463b2f994128dd25842`

Those match exactly.

So ESPO can safely use that stored Metashrew hash as its local notion of
"what block does Metashrew think height H is?"

## Root cause

### Primary issue: ESPO consumed Metashrew state by height without proving hash agreement

Before this patch, ESPO did all of the following off height-based Metashrew state:

- derived the effective Alkane tip from Metashrew's indexed height
- read block traces by height
- then stitched those traces onto a canonical block fetched from the block source / Core

That creates a race during a fork/reorg window:

1. Core or the block source is already on the replacement canonical block at height `H`.
2. ESPO decides to process height `H`.
3. Metashrew's secondary view is still exposing the previous block hash at `H`.
4. ESPO asks Metashrew for height-scoped data at `H` anyway.
5. ESPO mixes canonical block body data with stale-fork Metashrew state.

That is exactly the kind of inconsistency you were worried about when you said ESPO may detect a
fork before Metashrew has converged.

### Secondary issue: ESPO's adapter never actually used a real height-to-blockhash mapping

In `src/alkanes/metashrew.rs`, `RuneTableNative.HEIGHT_TO_BLOCKHASH` and
`BLOCKHASH_TO_HEIGHT` were still wired to `/runes/null` placeholders, so ESPO had no real
canonicality check through the adapter layer.

The attached `alkanes-rs` / `protorune` code does have real blockhash tables, but ESPO was not
using the correct Metashrew-side source of truth for reorg safety.

### What I did not find

I did not find a stronger, better-supported bug in ESPO's chain-versioned tree reads that explains
the production report more convincingly than the missing hash-consistency check.

There may still be edge cases in versioned visibility worth auditing further, but the concrete,
high-confidence bug was ESPO consuming Metashrew by height without a hash match.

## Changes made

### 1. Added a real Metashrew hash-at-height check

In `src/alkanes/metashrew.rs` I added:

- `get_indexed_block_hash_with_db`
- `ensure_canonical_height_with_db`
- `get_canonical_tip_height`

These read Metashrew's stored `"/__INTERNAL/height-to-hash/{height}"` entry and compare it to
Bitcoin Core's `getblockhash(height)`.

If the hashes differ, ESPO now treats that height as non-canonical / not ready.

### 2. Safe tip now uses Metashrew's canonical tip, not raw indexed height

`src/alkanes/utils.rs::get_safe_tip()` now uses:

- Metashrew canonical tip by hash agreement
- then `min(canonical_metashrew_tip, electrum_tip)` when electrum is available

This prevents ESPO from advancing purely because Metashrew indexed "some block at height H".
It only advances when Metashrew and Core agree on what block `H` actually is.

### 3. Height-based trace reads are now gated by hash agreement

`src/alkanes/metashrew.rs::traces_for_block_as_prost_with_db()` now:

1. catches up the secondary DB
2. proves Metashrew's stored hash at that height matches Core
3. only then scans height-based traces

So read paths that still use height-scoped block traces will now fail closed instead of silently
serving stale-fork data.

### 4. Canonical block assembly also checks hash agreement before using Metashrew traces

`src/alkanes/trace.rs::select_canonical_traces()` now verifies Metashrew is canonical at the block
height before it reads any height-scoped trace data.

That means the indexer will now retry the block rather than index a canonical block with Metashrew
data from the wrong fork.

### 5. The earlier txid fallback remains in place

I kept the earlier canonical-tx fallback behavior:

- if the canonical block contains an Alkane tx but Metashrew's height trace list misses it,
  ESPO falls back to direct txid trace lookup
- if the trace still cannot be found, ESPO fails the block instead of indexing incomplete data

That is still useful for incomplete height-index trace sets even when the height itself is
canonical.

### 6. The indexer now logs canonicality waits explicitly

In `src/main.rs`, ESPO now classifies canonicality-related retry conditions into dedicated
operator-visible log lines:

- `metashrew_tip_behind`
- `metashrew_missing_height_hash`
- `metashrew_hash_mismatch`

These are emitted as `[reorg_wait] ...` lines during:

- safe-tip withholding
- block-load retries

The retry loop now uses a bounded backoff for those cases and keeps re-checking until Metashrew
and Core converge on the same block hash for the required height.

## Why this is closer to fork-observer

The important change is not "poll faster" or "retry more".

It is this:

- before: ESPO trusted `height`
- after: ESPO trusts `height + matching block hash`

That is the same fundamental model fork-observer and Metashrew's own reorg handler use.

## Verification

I ran:

- `cargo check`
- `cargo test alkanes::trace -- --nocapture`

Both passed.

I also verified the Metashrew internal block-hash key against the live local DB and Core RPC, as
documented above.

## Residual risk

The highest-confidence indexing bug is fixed, but there is still one area worth a follow-up audit:

- "latest" Metashrew reads that are not scoped to a specific block height can still be risky if a
  caller wants strict canonical guarantees during a reorg window

For the indexer path, the new canonical-height gate is the important fix because ESPO now refuses
to process the block until Metashrew agrees with Core on that height.

If you want, the next audit step should be to classify every remaining "latest Metashrew" read in
ESPO and decide which of them also need explicit canonical-tip guarding.
