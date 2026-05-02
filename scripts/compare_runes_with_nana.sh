#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NANA_DIR="${NANA_DIR:-$ROOT/context/Nana}"
HEIGHT_START="${HEIGHT_START:-840000}"
BLOCKS="${BLOCKS:-100}"
ESPO_RPC="${ESPO_RPC:-http://127.0.0.1:5780/rpc}"

cat >&2 <<MSG
This helper is the repeatable Runes parity harness scaffold.

Expected inputs:
  NANA_DIR=$NANA_DIR
  HEIGHT_START=$HEIGHT_START
  BLOCKS=$BLOCKS
  ESPO_RPC=$ESPO_RPC

Next steps this script expects from the local Nana checkout:
  1. Disable Nana's RPC server import/startup path if its wasm dependency is unavailable.
  2. Run Nana over HEIGHT_START..HEIGHT_START+BLOCKS-1 and export balances JSON.
  3. Run Espo with modules.runes.enable=true over the same block window.
  4. Query runes.get_top_runes / runes.get_holders and diff balances.

Nana currently has multiple app-specific entrypoints, so wire the concrete
Nana export command into this script once that local checkout's env is known.
MSG

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required for result comparison" >&2
  exit 1
fi

curl -sS \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"runes.get_top_runes","params":{"page":1,"limit":100}}' \
  "$ESPO_RPC" \
  | jq -c '.'
