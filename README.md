
<img width="1124" height="336" alt="espobannernew" src="https://github.com/user-attachments/assets/525a8ed1-9811-4016-b5cb-f9efded12367" />


# Espo

#### 🍕 NOTE: A FREE version of ESPO is hosted at https://api.alkanode.com for anyone to use - courtesy of pizza.fun.

Espo is a production ready, general purpose indexer for Alkanes that builds and serves through its RPC indicies for highly sought after data may not be available through the default Sandshrew api. 

Espo does this through the concept of "modules" - during indexing, espo generates a struct called `EspoBlock`, which contains all the alkanes traces and transactions in a block. A pointer to espo block is passed around modules - in which they can then interpret as they wish to build any sort of indicies they like, such as OHLC data for example. 


 ### Requirements
 - A fully sycned Electrs (esplora fork): https://github.com/Blockstream/electrs
 - A fully sycned bitcoin core WITH txindex enabled
 - A fully synced metashrew


## Installation 
To start, clone the repo and build the binary:
```bash
git clone git@github.com:bitapeslabs/espo.git
cargo build --release
```

after the binary is built, configure `config.json` (see `sample.config.json`) and run:
```bash
./target/release/espo --config-path ./config.json
```

To serve the current database without running the indexer or mempool service, append `--view-only` to the command. This keeps the RPC server (and explorer if enabled) available for read-only access to the existing data.

To manually roll back on startup and resume indexing from a chosen height, set `rollback` in `config.json` or pass `--rollback <height>`. Espo rewinds indexed state to the parent of that height before the RPC/explorer servers start, then indexes forward from the requested height. This path uses the normal module reorg hooks, including runes undo journals.

Espo will build indicies for the .blk files in your bitcoin blocks directory and start indexing, with a fallback to the bitcoin RPC. I have only tested espo on my machine which has 32 cores adn 192gb of ram, and I achieve an index in a little less than 2 hours. On older hardware you can expect an index between 6-12 hours.

## Modules
- AMMDATA module (OHLC data, trades on oylswap, etc):
  https://github.com/bitapeslabs/espo/tree/main/src/modules/ammdata
  
- ESSENTIALS module (balances, holders data, address outpoints, K/V stores for contracts:
  https://github.com/bitapeslabs/espo/tree/main/src/modules/essentials

## Credits and License
This project is mantained by the pizza.fun foundation and opensourced to foster new developments on Alkanes. 

Espo is licensed under the BUSL agreement, which allows personal AND commercial use of the software UNLESS you are building a direct competitor to pizza.fun.




