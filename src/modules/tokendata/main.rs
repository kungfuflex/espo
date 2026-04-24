use super::rpc::register_rpc;
use super::schemas::{SchemaTokenActivityV1, TokenActivityKind, TokenActivitySource};
use super::storage::{
    GetIndexHeightParams, SetBatchParams, SetIndexHeightParams, TokenDataProvider,
    amount_from_row, scopes_for_source,
};
use crate::alkanes::trace::EspoBlock;
use crate::config::{debug_enabled, get_espo_db};
use crate::debug;
use crate::modules::ammdata::main::{load_balance_txs_by_height, pool_creator_spk_from_protostone};
use crate::modules::ammdata::storage::{AmmDataProvider, GetPoolDefsParams};
use crate::modules::ammdata::utils::reserves::{NewPoolInfo, extract_new_pools_from_espo_transaction};
use crate::modules::defs::{EspoModule, RpcNsRegistrar};
use crate::modules::essentials::storage::EssentialsProvider;
use crate::modules::essentials::utils::balances::mint_deltas_from_trace;
use crate::runtime::mdb::Mdb;
use crate::runtime::state_at::StateAt;
use crate::schemas::SchemaAlkaneId;
use anyhow::{Result, anyhow};
use bitcoin::hashes::Hash;
use bitcoin::{Network, Txid};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

pub struct TokenData {
    essentials_provider: Option<Arc<EssentialsProvider>>,
    amm_provider: Option<Arc<AmmDataProvider>>,
    provider: Option<Arc<TokenDataProvider>>,
    index_height: Arc<RwLock<Option<u32>>>,
}

impl TokenData {
    pub fn new() -> Self {
        Self {
            essentials_provider: None,
            amm_provider: None,
            provider: None,
            index_height: Arc::new(RwLock::new(None)),
        }
    }

    #[inline]
    fn essentials_provider(&self) -> &EssentialsProvider {
        self.essentials_provider
            .as_ref()
            .expect("ModuleRegistry must call set_mdb()")
            .as_ref()
    }

    #[inline]
    fn amm_provider(&self) -> &AmmDataProvider {
        self.amm_provider.as_ref().expect("ModuleRegistry must call set_mdb()").as_ref()
    }

    #[inline]
    fn provider(&self) -> &TokenDataProvider {
        self.provider.as_ref().expect("ModuleRegistry must call set_mdb()").as_ref()
    }

    fn load_index_height(&self) -> Option<u32> {
        self.provider()
            .get_index_height(GetIndexHeightParams { blockhash: StateAt::Latest })
            .ok()
            .and_then(|resp| resp.height)
    }

    fn persist_index_height(&self, height: u32, blockhash: StateAt) -> Result<()> {
        self.provider()
            .set_index_height(SetIndexHeightParams { blockhash, height })
            .map_err(|e| anyhow!("[TOKENDATA] rocksdb put(/index_height) failed: {e}"))
    }

    fn set_index_height(&self, new_height: u32, blockhash: StateAt) -> Result<()> {
        if let Some(prev) = *self.index_height.read().unwrap() {
            if new_height < prev {
                eprintln!("[TOKENDATA] index height rollback detected ({} -> {})", prev, new_height);
            }
        }
        self.persist_index_height(new_height, blockhash)?;
        *self.index_height.write().unwrap() = Some(new_height);
        Ok(())
    }
}

impl Default for TokenData {
    fn default() -> Self {
        Self::new()
    }
}

impl EspoModule for TokenData {
    fn get_name(&self) -> &'static str {
        "tokendata"
    }

    fn set_mdb(&mut self, mdb: Arc<Mdb>) {
        let db = get_espo_db();
        let essentials_provider =
            Arc::new(EssentialsProvider::new(Arc::new(Mdb::from_db(Arc::clone(&db), b"essentials:"))));
        let amm_provider = Arc::new(AmmDataProvider::new(
            Arc::new(Mdb::from_db(db, b"ammdata:")),
            Arc::clone(&essentials_provider),
        ));
        self.essentials_provider = Some(essentials_provider);
        self.amm_provider = Some(amm_provider);
        self.provider = Some(Arc::new(TokenDataProvider::new(mdb)));
        *self.index_height.write().unwrap() = self.load_index_height();
    }

    fn get_genesis_block(&self, network: Network) -> u32 {
        crate::modules::essentials::consts::essentials_genesis_block(network)
    }

    fn index_block(&self, block: EspoBlock) -> Result<()> {
        let t0 = std::time::Instant::now();
        let debug = debug_enabled();
        let module = self.get_name();
        let height = block.height;
        if let Some(prev) = *self.index_height.read().unwrap() {
            if height <= prev {
                eprintln!("[TOKENDATA] skipping already indexed block #{height} (last={prev})");
                return Ok(());
            }
        }

        let timer = debug::start_if(debug);
        let provider = self.provider();
        let table = provider.table();
        let blockhash = block.block_header.block_hash();
        let block_ts = block.block_header.time as u64;
        let tx_meta = build_tx_meta(&block);
        let mut puts: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        let mut ordinal: u32 = 0;
        let mut market_rows = 0usize;
        let mut mint_rows = 0usize;
        debug::log_elapsed(module, "init_context", timer);

        let timer = debug::start_if(debug);
        index_market_activity(
            self.amm_provider(),
            self.essentials_provider(),
            &table,
            height,
            block_ts,
            &tx_meta,
            &mut ordinal,
            &mut puts,
            &mut market_rows,
        )?;
        index_pool_creations(
            &block,
            &table,
            block_ts,
            &tx_meta,
            &mut ordinal,
            &mut puts,
            &mut market_rows,
        )?;
        debug::log_elapsed(module, "index_market_activity", timer);

        let timer = debug::start_if(debug);
        index_mints(
            &block,
            &table,
            block_ts,
            &tx_meta,
            &mut ordinal,
            &mut puts,
            &mut mint_rows,
        )?;
        debug::log_elapsed(module, "index_mints", timer);

        let timer = debug::start_if(debug);
        if !puts.is_empty() {
            provider.set_batch(SetBatchParams {
                blockhash: StateAt::Latest,
                puts,
                deletes: Vec::new(),
            })?;
        }
        self.set_index_height(height, StateAt::Latest)?;
        debug::log_elapsed(module, "commit", timer);

        eprintln!(
            "[TOKENDATA] indexed block #{height}: market_rows={market_rows}, mint_rows={mint_rows}, elapsed={:?}",
            t0.elapsed()
        );
        let _ = blockhash;
        Ok(())
    }

    fn get_index_height(&self) -> Option<u32> {
        *self.index_height.read().unwrap()
    }

    fn register_rpc(&self, reg: &RpcNsRegistrar) {
        register_rpc(reg, Arc::clone(self.provider.as_ref().expect("set_mdb first")));
    }
}

fn build_tx_meta(block: &EspoBlock) -> HashMap<Txid, (Vec<u8>, bool)> {
    let mut tx_meta: HashMap<Txid, (Vec<u8>, bool)> = HashMap::new();
    for atx in &block.transactions {
        let txid = atx.transaction.compute_txid();
        let spk_bytes = pool_creator_spk_from_protostone(&atx.transaction)
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_default();
        let success = atx.traces.as_ref().map_or(true, |traces| {
            !traces.iter().any(|trace| {
                trace.sandshrew_trace.events.iter().any(|ev| {
                    matches!(
                        ev,
                        crate::alkanes::trace::EspoSandshrewLikeTraceEvent::Return(r)
                            if r.status
                                == crate::alkanes::trace::EspoSandshrewLikeTraceStatus::Failure
                    )
                })
            })
        });
        tx_meta.insert(txid, (spk_bytes, success));
    }
    tx_meta
}

fn index_market_activity(
    amm_provider: &AmmDataProvider,
    essentials: &EssentialsProvider,
    table: &super::storage::TokenDataTable<'_>,
    height: u32,
    block_ts: u64,
    tx_meta: &HashMap<Txid, (Vec<u8>, bool)>,
    ordinal: &mut u32,
    puts: &mut Vec<(Vec<u8>, Vec<u8>)>,
    market_rows: &mut usize,
) -> Result<()> {
    let balance_txs = load_balance_txs_by_height(essentials, height).unwrap_or_else(|e| {
        eprintln!("[TOKENDATA] failed to load balance txs for height {height}: {e:?}");
        BTreeMap::new()
    });

    for (pool, entries) in balance_txs {
        let Some(defs) = amm_provider
            .get_pool_defs(GetPoolDefsParams { blockhash: StateAt::Latest, pool })
            .ok()
            .and_then(|resp| resp.defs)
        else {
            continue;
        };
        for entry in entries {
            let base_delta =
                crate::modules::ammdata::signed_from_delta(entry.outflow.get(&defs.base_alkane_id));
            let quote_delta =
                crate::modules::ammdata::signed_from_delta(entry.outflow.get(&defs.quote_alkane_id));
            let pool_kind = match (base_delta.signum(), quote_delta.signum()) {
                (1, -1) => Some(crate::modules::ammdata::schemas::ActivityKind::TradeSell),
                (-1, 1) => Some(crate::modules::ammdata::schemas::ActivityKind::TradeBuy),
                (1, 1) => Some(crate::modules::ammdata::schemas::ActivityKind::LiquidityAdd),
                (-1, -1) => Some(crate::modules::ammdata::schemas::ActivityKind::LiquidityRemove),
                _ => None,
            };
            let Some(pool_kind) = pool_kind else { continue };
            let txid = entry.txid;
            let txid_obj = Txid::from_byte_array(txid);
            let (address_spk, success) =
                tx_meta.get(&txid_obj).cloned().unwrap_or_else(|| (Vec::new(), true));
            let base_row = market_row_for_token(
                defs.base_alkane_id,
                defs.quote_alkane_id,
                pool,
                pool_kind,
                base_delta,
                quote_delta,
                txid,
                block_ts,
                address_spk.clone(),
                success,
            );
            write_row(table, base_row, *ordinal, puts)?;
            *ordinal = ordinal.saturating_add(1);
            *market_rows = market_rows.saturating_add(1);

            if defs.quote_alkane_id != defs.base_alkane_id {
                let quote_row = market_row_for_token(
                    defs.quote_alkane_id,
                    defs.base_alkane_id,
                    pool,
                    pool_kind,
                    quote_delta,
                    base_delta,
                    txid,
                    block_ts,
                    address_spk,
                    success,
                );
                write_row(table, quote_row, *ordinal, puts)?;
                *ordinal = ordinal.saturating_add(1);
                *market_rows = market_rows.saturating_add(1);
            }
        }
    }
    Ok(())
}

fn index_pool_creations(
    block: &EspoBlock,
    table: &super::storage::TokenDataTable<'_>,
    block_ts: u64,
    tx_meta: &HashMap<Txid, (Vec<u8>, bool)>,
    ordinal: &mut u32,
    puts: &mut Vec<(Vec<u8>, Vec<u8>)>,
    market_rows: &mut usize,
) -> Result<()> {
    for transaction in &block.transactions {
        let new_pools = extract_new_pools_from_espo_transaction(transaction, &block.host_function_values)
            .unwrap_or_default();
        if new_pools.is_empty() {
            continue;
        }
        let txid = transaction.transaction.compute_txid();
        let txid_bytes = txid.to_byte_array();
        let (address_spk, success) =
            tx_meta.get(&txid).cloned().unwrap_or_else(|| (Vec::new(), true));
        for NewPoolInfo { pool_id, defs, .. } in new_pools {
            let base_row = pool_create_row(
                defs.base_alkane_id,
                defs.quote_alkane_id,
                pool_id,
                txid_bytes,
                block_ts,
                address_spk.clone(),
                success,
            );
            write_row(table, base_row, *ordinal, puts)?;
            *ordinal = ordinal.saturating_add(1);
            *market_rows = market_rows.saturating_add(1);
            if defs.quote_alkane_id != defs.base_alkane_id {
                let quote_row = pool_create_row(
                    defs.quote_alkane_id,
                    defs.base_alkane_id,
                    pool_id,
                    txid_bytes,
                    block_ts,
                    address_spk.clone(),
                    success,
                );
                write_row(table, quote_row, *ordinal, puts)?;
                *ordinal = ordinal.saturating_add(1);
                *market_rows = market_rows.saturating_add(1);
            }
        }
    }
    Ok(())
}

fn index_mints(
    block: &EspoBlock,
    table: &super::storage::TokenDataTable<'_>,
    block_ts: u64,
    tx_meta: &HashMap<Txid, (Vec<u8>, bool)>,
    ordinal: &mut u32,
    puts: &mut Vec<(Vec<u8>, Vec<u8>)>,
    mint_rows: &mut usize,
) -> Result<()> {
    for atx in &block.transactions {
        let txid = atx.transaction.compute_txid();
        let Some(traces) = &atx.traces else { continue };
        let mut tx_mint_deltas: BTreeMap<SchemaAlkaneId, u128> = BTreeMap::new();
        for trace in traces {
            if let Some(mints) = mint_deltas_from_trace(&trace.sandshrew_trace, &block.host_function_values) {
                for (token, delta) in mints {
                    if delta == 0 {
                        continue;
                    }
                    *tx_mint_deltas.entry(token).or_default() =
                        tx_mint_deltas.get(&token).copied().unwrap_or(0).saturating_add(delta);
                }
            }
        }
        if tx_mint_deltas.is_empty() {
            continue;
        }
        let (address_spk, success) =
            tx_meta.get(&txid).cloned().unwrap_or_else(|| (Vec::new(), true));
        let txid_bytes = txid.to_byte_array();
        for (token, delta) in tx_mint_deltas {
            let row = SchemaTokenActivityV1 {
                timestamp: block_ts,
                txid: txid_bytes,
                token,
                kind: TokenActivityKind::Mint,
                source: TokenActivitySource::Mint,
                pool: None,
                counter_token: None,
                token_delta: i128::try_from(delta).unwrap_or(i128::MAX),
                counter_delta: 0,
                address_spk: address_spk.clone(),
                success,
            };
            write_row(table, row, *ordinal, puts)?;
            *ordinal = ordinal.saturating_add(1);
            *mint_rows = mint_rows.saturating_add(1);
        }
    }
    Ok(())
}

fn market_row_for_token(
    token: SchemaAlkaneId,
    counter_token: SchemaAlkaneId,
    pool: SchemaAlkaneId,
    pool_kind: crate::modules::ammdata::schemas::ActivityKind,
    pool_token_delta: i128,
    pool_counter_delta: i128,
    txid: [u8; 32],
    timestamp: u64,
    address_spk: Vec<u8>,
    success: bool,
) -> SchemaTokenActivityV1 {
    let token_delta = -pool_token_delta;
    let counter_delta = -pool_counter_delta;
    let kind = match pool_kind {
        crate::modules::ammdata::schemas::ActivityKind::TradeBuy
        | crate::modules::ammdata::schemas::ActivityKind::TradeSell => {
            if token_delta >= 0 { TokenActivityKind::Buy } else { TokenActivityKind::Sell }
        }
        crate::modules::ammdata::schemas::ActivityKind::LiquidityAdd => TokenActivityKind::LiquidityAdd,
        crate::modules::ammdata::schemas::ActivityKind::LiquidityRemove => TokenActivityKind::LiquidityRemove,
        crate::modules::ammdata::schemas::ActivityKind::PoolCreate => TokenActivityKind::PoolCreate,
    };
    SchemaTokenActivityV1 {
        timestamp,
        txid,
        token,
        kind,
        source: TokenActivitySource::Market,
        pool: Some(pool),
        counter_token: Some(counter_token),
        token_delta,
        counter_delta,
        address_spk,
        success,
    }
}

fn pool_create_row(
    token: SchemaAlkaneId,
    counter_token: SchemaAlkaneId,
    pool: SchemaAlkaneId,
    txid: [u8; 32],
    timestamp: u64,
    address_spk: Vec<u8>,
    success: bool,
) -> SchemaTokenActivityV1 {
    SchemaTokenActivityV1 {
        timestamp,
        txid,
        token,
        kind: TokenActivityKind::PoolCreate,
        source: TokenActivitySource::Market,
        pool: Some(pool),
        counter_token: Some(counter_token),
        token_delta: 0,
        counter_delta: 0,
        address_spk,
        success,
    }
}

fn write_row(
    table: &super::storage::TokenDataTable<'_>,
    row: SchemaTokenActivityV1,
    ordinal: u32,
    puts: &mut Vec<(Vec<u8>, Vec<u8>)>,
) -> Result<()> {
    let encoded = borsh::to_vec(&row)?;
    let amount = amount_from_row(&row);
    for scope in scopes_for_source(row.source) {
        puts.push((
            table.token_activity_key(scope, &row.token, row.timestamp, &row.txid, ordinal, row.kind),
            encoded.clone(),
        ));
        puts.push((
            table.token_activity_amount_key(
                scope,
                &row.token,
                amount,
                row.timestamp,
                &row.txid,
                ordinal,
                row.kind,
            ),
            encoded.clone(),
        ));
    }
    Ok(())
}
