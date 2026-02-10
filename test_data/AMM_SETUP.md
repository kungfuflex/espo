# OYL AMM Test Harness Setup

This document describes the OYL AMM (Automated Market Maker) test infrastructure for ESPO integration testing.

## Available WASM Binaries

### Core Runtime
- **alkanes.wasm** (6.6 MB) - Alkanes v2.1.6 regtest runtime
  - Main metashrew indexer
  - Processes all alkane operations
  - Required for any alkanes testing

### OYL AMM Contracts
- **factory.wasm** (256 KB) - OYL AMM Factory contract
  - Manages protocol configuration
  - Creates new liquidity pools
  - Handles pool templates and proxies

- **pool.wasm** (277 KB) - OYL AMM Pool contract
  - Implements AMM logic (constant product formula)
  - Handles swaps, liquidity provision, and fees
  - Deployed as template for cloning

## AMM Deployment Pattern

The OYL AMM follows an upgradeable proxy pattern with the following deployment sequence:

### 1. Infrastructure Deployment

```
Block Height N:
  Deploy pool.wasm -> [3, 0xffef] (65519)
  Purpose: Template for pool cloning

Block Height N+1:
  Deploy auth_token_factory -> [3, 0xffed] (65517)
  Purpose: Manages access control tokens

Block Height N+2:
  Deploy factory_logic.wasm -> [3, 2]
  Purpose: Factory implementation logic

Block Height N+3:
  Deploy beacon_proxy -> [3, 0xbeac1]
  Purpose: Proxy pattern for pools

Block Height N+4:
  Deploy upgradeable_beacon -> [3, 0xbeac0]
  Purpose: Points to pool template for upgrades
  Inputs: [0x7fff, 4, 0xffef, 1]  // marker, block, tx, version
```

### 2. Factory Proxy Deployment & Initialization

```
Block Height N+5:
  TX 0: Deploy factory proxy -> [3, 1]
    Inputs: [0x7fff, 4, 2, 1]  // marker, logic block, logic tx, version

  TX 1: Initialize factory (SAME BLOCK!)
    Target: [4, 1]  // factory proxy
    Inputs: [0, 0xbeac1, 4, 0xbeac0]  // InitFactory opcode, ...
```

**Critical**: Factory deployment and initialization MUST happen in the same block!

## Alkane ID Conventions

### System Alkanes (Block 2)
- `[2, 0]` - Native alkane (satoshis)
- `[2, N]` - Protocol tokens (auth tokens, etc.)

### Contract Space (Block 3)
- `[3, N]` - Deployment space for contracts
- Special IDs:
  - `[3, 1]` - Factory proxy
  - `[3, 2]` - Factory logic implementation
  - `[3, 0xffef]` - Pool template
  - `[3, 0xffed]` - Auth token factory
  - `[3, 0xbeac0]` - Pool upgradeable beacon
  - `[3, 0xbeac1]` - Pool beacon proxy

### Deployed Contracts (Block 4+)
- `[4, 1]` - Factory proxy (after deployment)
- `[4, 2]` - Factory logic
- `[4, 0xffef]` - Pool template
- `[N, M]` - Dynamically created pools

## Testing Patterns

### Pattern 1: Full AMM Setup

```rust
use espo::test_utils::fixtures::{get_alkanes_wasm, get_factory_wasm, get_pool_wasm};

// 1. Initialize metashrew with alkanes.wasm
let alkanes_wasm = get_alkanes_wasm();
// ... initialize runtime

// 2. Deploy AMM infrastructure (6 blocks)
deploy_amm_infrastructure(start_height);

// 3. Deploy and initialize factory
deploy_factory_proxy(start_height + 5);

// 4. Create a pool
create_pool(token0, token1);

// 5. Test AMM operations
add_liquidity(pool_id, amount0, amount1);
swap(pool_id, amount_in, token_in);
```

### Pattern 2: Test with Existing Deployment

If AMM is already deployed (e.g., in a test database):

```rust
// Connect to existing factory
let factory_id = AlkaneId { block: 4, tx: 1 };

// Create pool
let pool_id = call_factory_create_pool(factory_id, token0, token1);

// Test operations
test_swap_on_pool(pool_id);
```

## Cellpack Format

Cellpacks are the standard way to interact with alkanes:

```rust
use alkanes_support::cellpack::Cellpack;
use alkanes_support::id::AlkaneId;

// Deploy a contract
let deploy_cellpack = Cellpack {
    target: AlkaneId { block: 3, tx: N },
    inputs: vec![50],  // Deployment marker
};

// Call a contract function
let call_cellpack = Cellpack {
    target: AlkaneId { block: 4, tx: 1 },  // Factory proxy
    inputs: vec![
        0,           // CreatePool opcode
        token0_block,
        token0_tx,
        token1_block,
        token1_tx,
    ],
};
```

## Testing Workflow

### Step 1: Setup Test Environment
```rust
// Create temp directories
let (config, _temp_dirs) = TestConfigBuilder::new()
    .with_network(Network::Regtest)
    .with_height_indexed(true)
    .build();

// Initialize metashrew runtime with alkanes.wasm
let runtime = MetashrewRuntime::new(get_alkanes_wasm())?;
```

### Step 2: Deploy AMM
```rust
// Deploy infrastructure (blocks 0-4)
let deployment = deploy_amm_infrastructure(0)?;

// Deploy and initialize factory (block 5)
let factory_id = deploy_factory_proxy(5, &deployment)?;
```

### Step 3: Create Pools & Test
```rust
// Create a BTC/Token pool
let pool_id = create_pool(factory_id, native_alkane, token_alkane)?;

// Add liquidity
add_liquidity(pool_id, 1_000_000, 1_000)?;

// Perform swap
let amount_out = swap(pool_id, 100_000, native_alkane)?;

// Query reserves
let (reserve0, reserve1) = get_reserves(pool_id)?;
```

## ESPO Integration

The ESPO ammdata module tracks AMM state:

```rust
// After AMM operations, ESPO indexes the results
espo_indexer.index_block(block_with_amm_ops)?;

// Query via ESPO
let reserves = espo_db.get_pool_reserves(pool_id)?;
let price = espo_db.get_pool_price(pool_id)?;
```

## Constants Reference

```rust
pub const AMM_FACTORY_ID: u128 = 0xffef;               // 65519
pub const AUTH_TOKEN_FACTORY_ID: u128 = 0xffed;        // 65517
pub const AMM_FACTORY_LOGIC_IMPL_TX: u128 = 2;
pub const POOL_BEACON_PROXY_TX: u128 = 0xbeac1;        // 781249
pub const POOL_UPGRADEABLE_BEACON_TX: u128 = 0xbeac0;  // 781248
pub const AMM_FACTORY_PROXY_TX: u128 = 1;
```

## Troubleshooting

### Factory Not Initialized
**Problem**: Calling factory before initialization
**Solution**: Ensure initialization cellpack is in same block as proxy deployment

### Pool Creation Fails
**Problem**: Beacon not pointing to pool template
**Solution**: Verify beacon deployed with correct inputs: `[0x7fff, 4, 0xffef, 1]`

### Traces Missing
**Problem**: Operations not generating traces
**Solution**: Check that block was indexed through metashrew runtime first

### Balance Mismatches
**Problem**: ESPO balances don't match metashrew
**Solution**: Enable strict mode and verify indexing order is correct

## Next Steps

1. **Implement MetashrewRuntime wrapper** - Create test-friendly wrapper around metashrew
2. **Create AMM deployment helpers** - Port amm_setup.rs patterns to ESPO
3. **Build EspoTestHarness** - Full harness with AMM support
4. **Write ammdata tests** - Integration tests for ESPO's AMM module
5. **Add pool creation helpers** - Simplify pool setup in tests
6. **Implement swap helpers** - Easy swap testing
7. **Add liquidity helpers** - Simplify liquidity operations

## References

- **subfrost-alkanes** - `reference/subfrost-alkanes/src/tests/amm_setup.rs`
- **OYL Protocol** - Original AMM implementation
- **Alkanes Support** - `alkanes_support` crate documentation
- **Metashrew** - Metashrew indexer documentation
