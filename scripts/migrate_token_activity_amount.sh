#!/usr/bin/env bash
set -euo pipefail

CONFIG_PATH="${CONFIG_PATH:-./config.json}"

if [[ $# -gt 0 && "${1:-}" != --* ]]; then
  CONFIG_PATH="$1"
  shift
fi

echo "[token-activity-amount-migration] launching config=${CONFIG_PATH} extra_args=$*"
cargo run --release --features binary --bin migrate_token_activity_amount -- \
  --config-path "${CONFIG_PATH}" \
  "$@"
