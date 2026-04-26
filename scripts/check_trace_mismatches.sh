#!/usr/bin/env bash
set -euo pipefail

cargo run --bin trace_mismatch_check -- "$@"
