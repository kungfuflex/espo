use super::rpc::register_rpc;
use super::schemas::{SchemaTokenActivityV1, TokenActivityKind, TokenActivitySource};
use super::storage::{
    GetIndexHeightParams, SetBatchParams, SetIndexHeightParams, TokenDataProvider,
    amount_from_row, scopes_for_source,
};
use crate::alkanes::trace::EspoBlock;
use crate::config::{debug_enabled, get_electrum_like, get_espo_db};
use crate::debug;
use crate::modules::ammdata::consts::{AMOUNT_SCALE, PRICE_SCALE};
use crate::modules::ammdata::main::{load_balance_txs_by_height, pool_creator_spk_from_protostone};
use crate::modules::ammdata::storage::{
    AmmDataProvider, GetCanonicalPoolPricesParams, GetLatestTokenUsdCloseParams, GetPoolDefsParams,
};
use crate::modules::ammdata::schemas::Timeframe;
use crate::modules::ammdata::utils::reserves::{NewPoolInfo, extract_new_pools_from_espo_transaction};
use crate::modules::defs::{EspoModule, RpcNsRegistrar};
use crate::modules::essentials::storage::EssentialsProvider;
use crate::modules::essentials::utils::balances::mint_deltas_from_trace;
use crate::runtime::mdb::Mdb;
use crate::runtime::state_at::StateAt;
use crate::schemas::SchemaAlkaneId;
use alloy_primitives::U256;
use anyhow::{Result, anyhow};
use bitcoin::hashes::Hash;
use bitcoin::consensus::encode::deserialize;
use bitcoin::{Network, Transaction, Txid};
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
            self.amm_provider(),
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
        register_rpc(
            reg,
            Arc::clone(self.provider.as_ref().expect("set_mdb first")),
            Arc::clone(self.amm_provider.as_ref().expect("set_mdb first")),
        );
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
            let address_index_spks = unique_non_empty_spks(std::slice::from_ref(&address_spk));
            let base_row = market_row_for_token(
                height,
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
            write_row(table, base_row, &address_index_spks, *ordinal, puts)?;
            *ordinal = ordinal.saturating_add(1);
            *market_rows = market_rows.saturating_add(1);

            if defs.quote_alkane_id != defs.base_alkane_id {
                let quote_row = market_row_for_token(
                    height,
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
                write_row(table, quote_row, &address_index_spks, *ordinal, puts)?;
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
        let address_index_spks = unique_non_empty_spks(std::slice::from_ref(&address_spk));
        for NewPoolInfo { pool_id, defs, .. } in new_pools {
            let base_row = pool_create_row(
                block.height,
                defs.base_alkane_id,
                defs.quote_alkane_id,
                pool_id,
                txid_bytes,
                block_ts,
                address_spk.clone(),
                success,
            );
            write_row(table, base_row, &address_index_spks, *ordinal, puts)?;
            *ordinal = ordinal.saturating_add(1);
            *market_rows = market_rows.saturating_add(1);
            if defs.quote_alkane_id != defs.base_alkane_id {
                let quote_row = pool_create_row(
                    block.height,
                    defs.quote_alkane_id,
                    defs.base_alkane_id,
                    pool_id,
                    txid_bytes,
                    block_ts,
                    address_spk.clone(),
                    success,
                );
                write_row(table, quote_row, &address_index_spks, *ordinal, puts)?;
                *ordinal = ordinal.saturating_add(1);
                *market_rows = market_rows.saturating_add(1);
            }
        }
    }
    Ok(())
}

fn index_mints(
    block: &EspoBlock,
    amm_provider: &AmmDataProvider,
    table: &super::storage::TokenDataTable<'_>,
    block_ts: u64,
    tx_meta: &HashMap<Txid, (Vec<u8>, bool)>,
    ordinal: &mut u32,
    puts: &mut Vec<(Vec<u8>, Vec<u8>)>,
    mint_rows: &mut usize,
) -> Result<()> {
    let mut price_cache: HashMap<SchemaAlkaneId, MintPoolPriceSnapshot> = HashMap::new();
    for chain in build_mint_chains(block, tx_meta)? {
        for (token, delta) in chain.deltas {
            let pool_prices = price_cache
                .entry(token)
                .or_insert_with(|| load_mint_pool_prices(amm_provider, token, block_ts));
            let row = SchemaTokenActivityV1 {
                height: block.height,
                timestamp: block_ts,
                txid: chain.root_txid,
                chain_txids: chain.chain_txids.clone(),
                cpfp: chain.cpfp,
                mint_price_paid_sats: scale_fee_price_sats(chain.fee_paid_sats, delta),
                mint_price_pool_usd: pool_prices.usd_scaled,
                mint_price_pool_frbtc_sats: pool_prices.frbtc_sats_scaled,
                token,
                kind: TokenActivityKind::Mint,
                source: TokenActivitySource::Mint,
                pool: None,
                counter_token: None,
                token_delta: i128::try_from(delta).unwrap_or(i128::MAX),
                counter_delta: 0,
                address_spk: chain.display_address_spk.clone(),
                success: chain.success,
            };
            write_row(table, row, &chain.address_index_spks, *ordinal, puts)?;
            *ordinal = ordinal.saturating_add(1);
            *mint_rows = mint_rows.saturating_add(1);
        }
    }
    Ok(())
}

#[derive(Clone)]
struct MintChainActivity {
    root_txid: [u8; 32],
    chain_txids: Vec<[u8; 32]>,
    cpfp: bool,
    fee_paid_sats: u128,
    display_address_spk: Vec<u8>,
    address_index_spks: Vec<Vec<u8>>,
    success: bool,
    deltas: BTreeMap<SchemaAlkaneId, u128>,
}

#[derive(Clone)]
struct MintTxActivity {
    tx_index: usize,
    txid: Txid,
    txid_bytes: [u8; 32],
    parent_txids: Vec<Txid>,
    fee_paid_sats: u128,
    address_spk: Vec<u8>,
    success: bool,
    deltas: BTreeMap<SchemaAlkaneId, u128>,
}

#[derive(Clone, Copy)]
struct MintPoolPriceSnapshot {
    usd_scaled: [u8; 32],
    frbtc_sats_scaled: [u8; 32],
}

fn build_mint_chains(
    block: &EspoBlock,
    tx_meta: &HashMap<Txid, (Vec<u8>, bool)>,
) -> Result<Vec<MintChainActivity>> {
    let mint_txs = collect_mint_txs(block, tx_meta)?;
    if mint_txs.is_empty() {
        return Ok(Vec::new());
    }

    let tx_idx_by_txid: HashMap<Txid, usize> =
        mint_txs.iter().enumerate().map(|(idx, tx)| (tx.txid, idx)).collect();
    let mut parents: Vec<usize> = (0..mint_txs.len()).collect();

    for (idx, mint_tx) in mint_txs.iter().enumerate() {
        for parent_txid in &mint_tx.parent_txids {
            if let Some(parent_idx) = tx_idx_by_txid.get(parent_txid).copied() {
                union_components(&mut parents, idx, parent_idx);
            }
        }
    }

    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for idx in 0..mint_txs.len() {
        let root = find_component(&mut parents, idx);
        groups.entry(root).or_default().push(idx);
    }

    let mut out = Vec::with_capacity(groups.len());
    for member_indexes in groups.into_values() {
        let mut members: Vec<&MintTxActivity> =
            member_indexes.iter().map(|idx| &mint_txs[*idx]).collect();
        members.sort_by_key(|tx| tx.tx_index);
        let Some(root) = members.first().copied() else { continue };
        let mut deltas: BTreeMap<SchemaAlkaneId, u128> = BTreeMap::new();
        for member in &members {
            for (token, delta) in &member.deltas {
                *deltas.entry(*token).or_default() =
                    deltas.get(token).copied().unwrap_or(0).saturating_add(*delta);
            }
        }
        out.push(MintChainActivity {
            root_txid: root.txid_bytes,
            chain_txids: members.iter().map(|tx| tx.txid_bytes).collect(),
            cpfp: members.len() > 1,
            fee_paid_sats: members
                .iter()
                .fold(0u128, |acc, tx| acc.saturating_add(tx.fee_paid_sats)),
            display_address_spk: root.address_spk.clone(),
            address_index_spks: unique_non_empty_spks(
                &members.iter().map(|tx| tx.address_spk.clone()).collect::<Vec<_>>(),
            ),
            success: members.iter().all(|tx| tx.success),
            deltas,
        });
    }
    out.sort_by_key(|chain| {
        mint_txs
            .iter()
            .find(|tx| tx.txid_bytes == chain.root_txid)
            .map(|tx| tx.tx_index)
            .unwrap_or(usize::MAX)
    });
    Ok(out)
}

fn collect_mint_txs(
    block: &EspoBlock,
    tx_meta: &HashMap<Txid, (Vec<u8>, bool)>,
) -> Result<Vec<MintTxActivity>> {
    let block_tx_map: HashMap<Txid, &Transaction> = block
        .transactions
        .iter()
        .map(|atx| (atx.transaction.compute_txid(), &atx.transaction))
        .collect();
    let external_prev_map = load_external_prev_txs_for_mints(block);
    let mut out = Vec::new();
    for (tx_index, atx) in block.transactions.iter().enumerate() {
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
        out.push(MintTxActivity {
            tx_index,
            txid,
            txid_bytes: txid.to_byte_array(),
            parent_txids: atx
                .transaction
                .input
                .iter()
                .map(|input| input.previous_output.txid)
                .collect(),
            fee_paid_sats: compute_tx_fee_sats(&atx.transaction, &block_tx_map, &external_prev_map),
            address_spk,
            success,
            deltas: tx_mint_deltas,
        });
    }
    Ok(out)
}

fn find_component(parents: &mut [usize], idx: usize) -> usize {
    if parents[idx] != idx {
        let root = find_component(parents, parents[idx]);
        parents[idx] = root;
    }
    parents[idx]
}

fn union_components(parents: &mut [usize], a: usize, b: usize) {
    let root_a = find_component(parents, a);
    let root_b = find_component(parents, b);
    if root_a != root_b {
        parents[root_b] = root_a;
    }
}

fn unique_non_empty_spks(spks: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for spk in spks {
        if spk.is_empty() || out.iter().any(|existing| existing == spk) {
            continue;
        }
        out.push(spk.clone());
    }
    out
}

fn load_external_prev_txs_for_mints(block: &EspoBlock) -> HashMap<Txid, Transaction> {
    let block_txids: std::collections::HashSet<Txid> =
        block.transactions.iter().map(|atx| atx.transaction.compute_txid()).collect();
    let mut needed: Vec<Txid> = Vec::new();
    let mut seen: std::collections::HashSet<Txid> = std::collections::HashSet::new();
    for atx in &block.transactions {
        let Some(traces) = &atx.traces else { continue };
        let mut has_mint = false;
        for trace in traces {
            if mint_deltas_from_trace(&trace.sandshrew_trace, &block.host_function_values)
                .map(|m| !m.is_empty())
                .unwrap_or(false)
            {
                has_mint = true;
                break;
            }
        }
        if !has_mint {
            continue;
        }
        for input in &atx.transaction.input {
            if input.previous_output.is_null() || block_txids.contains(&input.previous_output.txid) {
                continue;
            }
            if seen.insert(input.previous_output.txid) {
                needed.push(input.previous_output.txid);
            }
        }
    }
    if needed.is_empty() {
        return HashMap::new();
    }
    let raws = get_electrum_like().batch_transaction_get_raw(&needed).unwrap_or_default();
    let mut out = HashMap::new();
    for (idx, raw) in raws.into_iter().enumerate() {
        if raw.is_empty() {
            continue;
        }
        if let Ok(tx) = deserialize::<Transaction>(&raw) {
            out.insert(needed[idx], tx);
        }
    }
    out
}

fn compute_tx_fee_sats(
    tx: &Transaction,
    block_tx_map: &HashMap<Txid, &Transaction>,
    external_prev_map: &HashMap<Txid, Transaction>,
) -> u128 {
    let mut input_total = 0u128;
    for input in &tx.input {
        if input.previous_output.is_null() {
            continue;
        }
        let prev_tx = block_tx_map
            .get(&input.previous_output.txid)
            .copied()
            .or_else(|| external_prev_map.get(&input.previous_output.txid));
        let Some(prev_tx) = prev_tx else { return 0 };
        let Some(prev_out) = prev_tx.output.get(input.previous_output.vout as usize) else { return 0 };
        input_total = input_total.saturating_add(prev_out.value.to_sat() as u128);
    }
    let output_total = tx
        .output
        .iter()
        .fold(0u128, |acc, output| acc.saturating_add(output.value.to_sat() as u128));
    input_total.saturating_sub(output_total)
}

fn load_mint_pool_prices(
    amm_provider: &AmmDataProvider,
    token: SchemaAlkaneId,
    now_ts: u64,
) -> MintPoolPriceSnapshot {
    let canonical = amm_provider
        .get_canonical_pool_prices(GetCanonicalPoolPricesParams {
            blockhash: StateAt::Latest,
            token,
            now_ts,
        })
        .ok();
    let frbtc_price = canonical.as_ref().map(|res| res.frbtc_price).unwrap_or(0);
    let usd_direct = canonical.as_ref().map(|res| res.busd_price).unwrap_or(0);
    let usd_price = if usd_direct != 0 {
        usd_direct
    } else {
        amm_provider
            .get_latest_token_usd_close(GetLatestTokenUsdCloseParams {
                blockhash: StateAt::Latest,
                token,
                timeframe: Timeframe::M10,
            })
            .ok()
            .and_then(|res| res.close)
            .unwrap_or(0)
    };
    MintPoolPriceSnapshot {
        usd_scaled: u128_to_u256_be(usd_price),
        frbtc_sats_scaled: u128_to_u256_be(frbtc_price),
    }
}

fn scale_fee_price_sats(fee_paid_sats: u128, token_amount: u128) -> [u8; 32] {
    if fee_paid_sats == 0 || token_amount == 0 {
        return [0u8; 32];
    }
    let price = U256::from(fee_paid_sats)
        .saturating_mul(U256::from(AMOUNT_SCALE))
        .saturating_mul(U256::from(PRICE_SCALE))
        / U256::from(token_amount);
    u256_to_be(price)
}

fn u128_to_u256_be(value: u128) -> [u8; 32] {
    u256_to_be(U256::from(value))
}

fn u256_to_be(value: U256) -> [u8; 32] {
    value.to_be_bytes::<32>()
}

fn market_row_for_token(
    height: u32,
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
        height,
        timestamp,
        txid,
        chain_txids: vec![txid],
        cpfp: false,
        mint_price_paid_sats: [0u8; 32],
        mint_price_pool_usd: [0u8; 32],
        mint_price_pool_frbtc_sats: [0u8; 32],
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
    height: u32,
    token: SchemaAlkaneId,
    counter_token: SchemaAlkaneId,
    pool: SchemaAlkaneId,
    txid: [u8; 32],
    timestamp: u64,
    address_spk: Vec<u8>,
    success: bool,
) -> SchemaTokenActivityV1 {
    SchemaTokenActivityV1 {
        height,
        timestamp,
        txid,
        chain_txids: vec![txid],
        cpfp: false,
        mint_price_paid_sats: [0u8; 32],
        mint_price_pool_usd: [0u8; 32],
        mint_price_pool_frbtc_sats: [0u8; 32],
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
    address_index_spks: &[Vec<u8>],
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
    for address_spk in address_index_spks {
        if address_spk.is_empty() {
            continue;
        }
        for scope in scopes_for_source(row.source) {
            puts.push((
                table.address_activity_key(
                    scope,
                    address_spk,
                    row.timestamp,
                    &row.txid,
                    ordinal,
                    row.kind,
                ),
                encoded.clone(),
            ));
            puts.push((
                table.address_activity_amount_key(
                    scope,
                    address_spk,
                    amount,
                    row.timestamp,
                    &row.txid,
                    ordinal,
                    row.kind,
                ),
                encoded.clone(),
            ));
            puts.push((
                table.address_token_activity_key(
                    scope,
                    address_spk,
                    &row.token,
                    row.timestamp,
                    &row.txid,
                    ordinal,
                    row.kind,
                ),
                encoded.clone(),
            ));
            puts.push((
                table.address_token_activity_amount_key(
                    scope,
                    address_spk,
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
    }
    Ok(())
}
