cargo run --release --features binary -- \
  --config-path ./config.json \
  --rollback 957400
# Add --view-only to disable indexing/mempool and serve existing data only.
