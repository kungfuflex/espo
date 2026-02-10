# ammdata-liquidity-path-feature

## Goal
Add “derived liquidity” USD charts for tokens that do not have direct canonical quote pools (frBTC/bUSD), by routing through a whitelisted intermediate token (e.g., `2:0`). Optionally merge derived charts with direct canonical charts using a configurable strategy, and expose derived metrics via indices, sorting, and API fields.

## New Config
Add optional `derived_liquidity` to ammdata config with **per-quote merge strategy**:

```json
"derived_liquidity": [
  { "alkane": "2:0", "strategy": "optimistic" },
  { "alkane": "2:123", "strategy": "neutral" }
]
```

- `derived_liquidity`: list of intermediate alkanes allowed as routing quotes, each with its own merge strategy.
- `strategy`: how to combine direct canonical USD candles with derived candles for that quote if both exist.

## Core Behavior

### A) Build Derived USD Candles (Liquidity Path)
When token `T` lacks a direct canonical USD chart, but has a pool with a whitelisted quote `Q` (e.g., `2:0`), and `Q` has a canonical USD chart, derive USD candles for `T` via:

`T -> Q -> canonical USD`

**How candles are derived:**
- Use the `T/Q` pool candles to convert `Q`’s USD candles into `T`’s USD candles.
- The start of the derived chart is the first trade in the `T/Q` pool, or (if later) the first trade in the `Q/canonical` pool.
- For each timeframe bucket:
  - Find the `T/Q` candle for that bucket; if missing, use the latest prior `T/Q` candle (carry forward).
  - Use that `T/Q` candle to convert `Q/USD` OHLC into `T/USD` OHLC.
  - Volume should match the USD volume logic used for the canonical USD chart (same volume value you set in `T-usd` candles).

### B) Merge Strategy (Derived Only)
Derived USD candles **do not** affect the canonical `T-usd` series.  
Canonical USD is built **only** from canonical quote pools (bUSD/frBTC).

When a canonical `T-usd` candle exists for the same bucket, **merge it into the derived candle**
using the configured `strategy` **and write the result only to** `T-derived_Q-usd`.

**Strategies (apply to OHLC only):**
- **Optimistic:** max of open/high/low/close
- **Pessimistic:** min of open/high/low/close
- **Neutral:** average of open/high/low/close
- **Neutral-vwap:** volume-weighted average of open/high/low/close

**Volume:** always taken from the derived path (T/Q volume converted to USD).

## Storage / Indices

### Derived Candle Storage
- Create new candle series per `(token, derived_quote)` in a **separate namespace** from `tuc1:` (confirmed).
- Example key suffix: `derived_2:0` (exact key pattern TBD).

### Derived Metrics + Indices
For each derived candle series, compute and store:
- price, marketcap, volume (1d/7d/30d/all-time), changes (1d/7d/30d/all-time)

Add indices similar to canonical metrics, with derived keys like:
- `change1d-derived_2:0`
- `volume7d-derived_2:0`

Also add **name indices** so derived series can be searched / sorted independently.

### Sorting Rules
- `get_alkanes` should accept sort_by keys for derived indices.
- If sorting by a derived index, only tokens with that derived candle series should be returned (non-participants are excluded).

## API Changes (Oyl API)
Add `derived_data` to `AlkaneToken` in `get_alkanes`:

```json
"derived_data": {
  "tokenVolume1d-derived_2:0": 123,
  "priceUsd-derived_2:0": 0.42,
  ...
}
```

Include entries for all derived series available per token.

## Implementation Notes
- Derived candles are built for **all timeframes**.
- Candle filling should follow existing USD candle behavior (forward-fill from prior close where needed).
- Derived series should be generated every time canonical USD candles are updated.
- Ensure merge behavior only runs when both direct and derived candles exist.

## Confirmed Decisions
- **Storage:** derived candles live in a **separate namespace** (not mixed with `tuc1:`).
- **Conversion math:** use the pool `base_candle` price (quote per base) and **multiply**:  
  `TUSD = QUSD * (Q per T)`.
- **Chart start:** derived chart starts at the **first T/Q candle**.  
  If `Q/USD` doesn’t exist yet, wait until **both** `T/Q` and `Q/USD` have started; the first derived candle is at the later of those two start times.
- **Missing T/Q candle:** **forward-fill** from the latest prior `T/Q` candle.
- **Open/close rule (derived charts):** derived candle **open is always the previous T/USD close** (i.e., carry-forward open), not recomputed by merge or conversion logic.
- **Volume:** derived USD volume comes from **T/Q volume**, converted to USD via the derived T/USD price. **Do not** use Q/USD volume.
- **Merge behavior:** derived series is used alone until a direct canonical T/USD exists. Once both paths exist, **merge canonical into the derived candle only** (no effect on `T-usd`), and keep historical pre‑merge derived candles as‑is.
- **Multiple quotes:** every derived quote gets its **own derived series + indices**, treated independently.

## Open Questions
None — timestamps are aligned because ammdata fills empty candles for all timeframes.
