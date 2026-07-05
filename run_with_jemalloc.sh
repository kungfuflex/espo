#!/usr/bin/env bash
set -euo pipefail

export MALLOC_CONF="${MALLOC_CONF:-prof:true,prof_active:false,lg_prof_sample:20,prof_final:false,background_thread:true}"

cargo run --release --features "binary jemalloc-prof" -- \
  --config-path "${ESPO_CONFIG_PATH:-./config.json}" \
  "$@"
