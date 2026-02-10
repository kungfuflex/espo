#!/usr/bin/env bash
set -euo pipefail

# Editable targets
EXPLORER_BASE_URL="${EXPLORER_BASE_URL:-http://127.0.0.1:5779}"
BLOCK_HEIGHT="${BLOCK_HEIGHT:-880000}"
ADDRESS="${ADDRESS:-bc1pn85w5h02uq4tgnrjv26sf0zkqs6a0gsra9nt0uxnccvjrnca6rmq2ngvj4}"
ALKANE_ID="${ALKANE_ID:-2:0}"
TXID="${TXID:-40a973594ec916790ca7fbaf591a2001336e5f4846650b25e50b8e11b3ace49d}"

measure() {
  local path="$1"
  local url="${EXPLORER_BASE_URL}${path}"
  local out
  out="$(curl -s -o /dev/null -w '%{http_code} %{time_total}' "$url")"
  local code time
  code="$(printf '%s' "$out" | awk '{print $1}')"
  time="$(printf '%s' "$out" | awk '{print $2}')"
  printf '%-30s %s %s\n' "$path" "$code" "$time"
}

echo "Base: ${EXPLORER_BASE_URL}"
echo "Format: path status_code time_total_seconds"

measure "/"
measure "/alkanes"
measure "/block/${BLOCK_HEIGHT}"
measure "/address/${ADDRESS}"
measure "/alkane/${ALKANE_ID}"
measure "/tx/${TXID}"
