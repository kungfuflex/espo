cargo run --release -- \
  --readonly-metashrew-db-dir /data/.metashrew/v9-exp/.metashrew-v9 \
  --port 6778 \
  --electrs-esplora-url http://127.0.0.1:4332 \
  --bitcoind-rpc-url http://127.0.0.1:8332 \
  --bitcoind-rpc-user admin \
  --bitcoind-rpc-pass admin \
  --network mainnet \
  --indexer-block-delay-ms 50 \
  --block-source-mode rpc \
  --metashrew-rpc-url http://127.0.0.1:7045 \
  --explorer-host 0.0.0.0:6779 \
  --enable-aof \
  --debug \
  --db-path ./db2
# Add --view-only to disable indexing/mempool and serve existing data only.
