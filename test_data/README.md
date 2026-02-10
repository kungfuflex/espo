# Test Data Directory

This directory contains test fixtures and data files needed for ESPO integration tests.

## alkanes.wasm

The `alkanes.wasm` file is required for running integration tests with the metashrew runtime.

### How to obtain alkanes.wasm

**Option 1: Download from alkanes-rs releases**
```bash
# Download from the v2.1.6 release
curl -L https://github.com/kungfuflex/alkanes-rs/releases/download/v2.1.6/alkanes.wasm \
  -o test_data/alkanes.wasm
```

**Option 2: Build from source**
```bash
# Clone the alkanes-rs repository
git clone https://github.com/kungfuflex/alkanes-rs
cd alkanes-rs
git checkout v2.1.6

# Build the WASM module
# (Follow the build instructions in the alkanes-rs repository)

# Copy the built WASM file
cp target/wasm32-unknown-unknown/release/alkanes.wasm /path/to/espo/test_data/
```

**Option 3: Use build.rs (TODO)**

In the future, we may add a `build.rs` script that automatically downloads `alkanes.wasm` if it's not present.

## File Structure

```
test_data/
├── README.md          # This file
├── alkanes.wasm      # Alkanes runtime WASM module (v2.1.6)
└── (future test fixtures)
```

## Usage in Tests

Once `alkanes.wasm` is available, it can be loaded in tests via:

```rust
use espo::test_utils::fixtures::get_alkanes_wasm;

let wasm_bytes = get_alkanes_wasm();
// Use with metashrew runtime...
```
