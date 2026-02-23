#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

DB_PATH="${1:-db/espo}"
INDEX_DOCS="${2:-context/espo-indicies}"
OUT_MD="${3:-docs/index-size-audit.md}"
TOP_ROWS="${4:-120}"

cargo run --bin index_size_audit -- \
  --db "${DB_PATH}" \
  --index-docs "${INDEX_DOCS}" \
  --out-md "${OUT_MD}" \
  --top "${TOP_ROWS}"
