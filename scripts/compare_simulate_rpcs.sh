#!/usr/bin/env bash
set -euo pipefail

ALKANES_CLI_MAINNET="${ALKANES_CLI_MAINNET:-alkanes-cli-mainnet}"
ALKANES_CLI_BIN="${ALKANES_CLI_BIN:-}"
ALKANODE_RPC="${ALKANODE_RPC:-https://mainnet.alkanode.com/}"
SUBFROST_RPC="${SUBFROST_RPC:-}"

TARGET="${TARGET:-32:0}"
OPCODE="${OPCODE:-103}"
HEIGHT="${HEIGHT:-947095}"
TXINDEX="${TXINDEX:-0}"
TRANSACTION="${TRANSACTION:-0x}"
BLOCK="${BLOCK:-0x}"

if [[ -z "$SUBFROST_RPC" ]]; then
  cli_path="$(command -v "$ALKANES_CLI_MAINNET")"
  SUBFROST_RPC="$(
    sed -n 's/.*--metashrew-rpc-url[[:space:]]\+\([^"[:space:]]\+\).*/\1/p' "$cli_path" |
      head -n 1
  )"
fi

if [[ -z "$SUBFROST_RPC" ]]; then
  echo "could not discover subfrost RPC URL from $ALKANES_CLI_MAINNET" >&2
  exit 1
fi

if [[ -z "$ALKANES_CLI_BIN" ]]; then
  wrapper_dir="$(cd "$(dirname "$(command -v "$ALKANES_CLI_MAINNET")")" && pwd)"
  if [[ -x "${wrapper_dir}/alkanes-cli" ]]; then
    ALKANES_CLI_BIN="${wrapper_dir}/alkanes-cli"
  else
    ALKANES_CLI_BIN="$(command -v alkanes-cli)"
  fi
fi

ALKANE_ID="${TARGET}:${OPCODE}"

print_call_header() {
  local rpc_name="$1"
  local rpc_url="$2"

  echo
  echo "=== ${rpc_name} ==="
  echo "rpc: ${rpc_url}"
  echo "request: alkanes simulate ${ALKANE_ID} --height ${HEIGHT} --txindex ${TXINDEX} --transaction ${TRANSACTION} --block ${BLOCK} --raw"
}

call_simulate() {
  local rpc_url="$1"
  "$ALKANES_CLI_BIN" \
    -p mainnet \
    --metashrew-rpc-url "$rpc_url" \
    alkanes simulate "$ALKANE_ID" \
    --height "$HEIGHT" \
    --txindex "$TXINDEX" \
    --transaction "$TRANSACTION" \
    --block "$BLOCK" \
    --block-tag "$HEIGHT" \
    --raw
}

run_and_capture() {
  local rpc_name="$1"
  local rpc_url="$2"
  local outfile="$3"
  local status

  print_call_header "$rpc_name" "$rpc_url" | tee "$outfile"
  set +e
  call_simulate "$rpc_url" 2>&1 | tee -a "$outfile"
  status="${PIPESTATUS[0]}"
  echo "exit_status: ${status}" | tee -a "$outfile"
  RUN_AND_CAPTURE_STATUS="$status"
  return 0
}

normalize_response() {
  if command -v jq >/dev/null 2>&1; then
    jq -S .
  else
    sed 's/[[:space:]]//g'
  fi
}

extract_first_json_object() {
  awk '
    /^\{/ {
      capture = 1
      depth = 0
    }
    capture {
      print
      opens = gsub(/\{/, "{")
      closes = gsub(/\}/, "}")
      depth += opens - closes
      if (depth == 0) {
        exit
      }
    }
  '
}

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

set +e
run_and_capture "alkanode" "$ALKANODE_RPC" "$tmpdir/alkanode.out"
alkanode_status="$RUN_AND_CAPTURE_STATUS"
run_and_capture "subfrost" "$SUBFROST_RPC" "$tmpdir/subfrost.out"
subfrost_status="$RUN_AND_CAPTURE_STATUS"
set -e

extract_first_json_object < "$tmpdir/alkanode.out" | normalize_response > "$tmpdir/alkanode.normalized"
extract_first_json_object < "$tmpdir/subfrost.out" | normalize_response > "$tmpdir/subfrost.normalized"

echo
echo "=== comparison ==="
echo "alkanode_exit_status: ${alkanode_status}"
echo "subfrost_exit_status: ${subfrost_status}"

if [[ "$alkanode_status" != "0" || "$subfrost_status" != "0" ]]; then
  echo "match: cannot compare successful responses because at least one RPC call failed"
elif cmp -s "$tmpdir/alkanode.normalized" "$tmpdir/subfrost.normalized"; then
  echo "match: responses are identical after JSON normalization"
else
  echo "match: responses differ"
  if command -v diff >/dev/null 2>&1; then
    diff -u "$tmpdir/alkanode.normalized" "$tmpdir/subfrost.normalized" || true
  fi
fi
