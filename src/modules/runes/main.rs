use super::inscriptions::{
    RuneIcon, delegate_inscription_from_tx, image_inscription_from_tx, image_inscription_from_tx_at,
};
use super::rpc;
use super::storage::{
    OutpointRuneBalances, RuneActivity, RuneActivityKind, RuneActivityScope, RuneActivitySortField,
    RuneBalance, RuneEntry, RuneMintActivity, RuneTxIndexKind, RuneVolumeKind, RunesProvider,
    SchemaRuneId, TxRuneIo, action_tx_address_list_key, action_tx_block_list_key,
    action_tx_pointer_count_key, action_tx_pointer_key, address_balance_history_key,
    address_balance_history_list_idx_key, address_balance_history_list_len_key,
    address_balance_key, address_outpoint_key, append_rune_tx_index_values, encode,
    encode_action_tx_pointer_blob, encode_rune_tx_pointer_blob, encode_u32, encode_u64,
    encode_u128, entry_key, holder_key, holders_count_key, id_by_name_key, id_by_rune_key,
    make_entry, mint_activity_key, outpoint_key, rune_activity_index_key,
    rune_address_activity_index_key, rune_address_token_activity_index_key, rune_icon_key,
    rune_tx_address_list_key, rune_tx_block_list_key, rune_tx_chunk_counter_key,
    rune_tx_pointer_count_key, rune_tx_pointer_key, rune_volume_entry_key,
    rune_volume_list_idx_key, rune_volume_list_len_key, script_to_address, seq_count_key, seq_key,
    tx_io_key, uncommon_goods_avg_price_usd_by_height_key,
};
use super::transfer::{OutputRuneSheets, RuneSheet, RunestoneTransfer, TransferRules};
use crate::alkanes::trace::EspoBlock;
use crate::config::{
    debug_enabled, get_bitcoind_rpc_client, get_electrum_like, get_espo_db, get_network,
};
use crate::modules::ammdata::config::AmmDataConfig;
use crate::modules::ammdata::consts::{PRICE_SCALE, SATS_PER_BTC};
use crate::modules::ammdata::storage::AmmDataProvider;
use crate::modules::defs::{EspoModule, RpcNsRegistrar};
use crate::modules::essentials::storage::{
    AddressIndexListKind, EssentialsProvider, address_index_list_id_alkane_block_txs,
    get_address_index_list_len, get_address_index_list_range, load_tx_pointer_blob_v3_by_id,
};
use crate::runtime::mdb::Mdb;
use crate::runtime::state_at::StateAt;
use alloy_primitives::U256;
use anyhow::Result;
use bitcoin::blockdata::script::Instruction;
use bitcoin::consensus::encode::deserialize;
use bitcoin::hashes::Hash;
use bitcoin::{Network, OutPoint, Transaction, Txid, opcodes};
use bitcoincore_rpc::RpcApi;
use ordinals::{Artifact, Edict, Etching, Height, Rune, RuneId, Runestone};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::time::Instant;

const MAINNET_RUNES_GENESIS: u32 = 840_000;
const GENESIS_RUNE_ID: SchemaRuneId = SchemaRuneId { block: 1, tx: 0 };

pub fn runes_genesis_block(network: Network) -> u32 {
    match network {
        Network::Bitcoin => MAINNET_RUNES_GENESIS,
        Network::Testnet => Rune::first_rune_height(Network::Testnet),
        Network::Regtest | Network::Signet => 0,
        _ => 0,
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunesConfig {
    #[serde(default)]
    pub enable: bool,
    #[serde(default = "default_load_live_outpoints")]
    pub load_live_outpoints: bool,
}

fn default_load_live_outpoints() -> bool {
    true
}

pub fn runes_enabled_from_global_config() -> bool {
    crate::config::get_module_config("runes")
        .and_then(|value| serde_json::from_value::<RunesConfig>(value.clone()).ok())
        .map(|cfg| cfg.enable)
        .unwrap_or(false)
}

pub struct Runes {
    provider: Option<Arc<RunesProvider>>,
    index_height: Arc<RwLock<Option<u32>>>,
    live_outpoints: Arc<RwLock<HashSet<(Txid, u32)>>>,
    config: RunesConfig,
}

impl Runes {
    pub fn new() -> Self {
        Self {
            provider: None,
            index_height: Arc::new(RwLock::new(None)),
            live_outpoints: Arc::new(RwLock::new(HashSet::new())),
            config: RunesConfig {
                enable: false,
                load_live_outpoints: default_load_live_outpoints(),
            },
        }
    }

    fn provider(&self) -> &RunesProvider {
        self.provider.as_ref().expect("ModuleRegistry must call set_mdb").as_ref()
    }

    fn reload_live_outpoints(&self, height: Option<u32>) -> Result<()> {
        if height.is_none() || !self.config.enable {
            self.live_outpoints.write().unwrap().clear();
            return Ok(());
        }

        if !self.config.load_live_outpoints {
            self.live_outpoints.write().unwrap().clear();
            eprintln!("[RUNES] live rune outpoint preload disabled; using RocksDB point lookups");
            return Ok(());
        }

        let live_outpoints = self.provider().get_live_outpoints()?;
        eprintln!("[RUNES] preloaded {} live rune outpoints", live_outpoints.len());
        *self.live_outpoints.write().unwrap() = live_outpoints;
        Ok(())
    }
}

impl Default for Runes {
    fn default() -> Self {
        Self::new()
    }
}

impl EspoModule for Runes {
    fn get_name(&self) -> &'static str {
        "runes"
    }

    fn set_mdb(&mut self, mdb: Arc<Mdb>) {
        let provider = Arc::new(RunesProvider::new(mdb));
        match provider.get_index_height() {
            Ok(mut height) => {
                if height.is_some()
                    && provider.get_rune(GENESIS_RUNE_ID).ok().flatten().is_none()
                    && runes_genesis_block(get_network()) == MAINNET_RUNES_GENESIS
                {
                    let cleared = provider.clear_namespace().unwrap_or_else(|err| {
                        eprintln!("[RUNES] failed to clear invalid runes namespace: {err:?}");
                        0
                    });
                    eprintln!(
                        "[RUNES] index height exists without genesis rune; cleared {cleared} keys and treating runes index as uninitialized"
                    );
                    height = None;
                }
                if let Some(indexed_height) = height {
                    if !provider.has_undo_for_height(indexed_height).unwrap_or(false) {
                        let cleared = provider.clear_namespace().unwrap_or_else(|err| {
                            eprintln!(
                                "[RUNES] failed to clear runes namespace without undo journal: {err:?}"
                            );
                            0
                        });
                        eprintln!(
                            "[RUNES] index height {indexed_height} has no undo journal; cleared {cleared} keys and treating runes index as uninitialized"
                        );
                        height = None;
                    }
                }

                *self.index_height.write().unwrap() = height;
                eprintln!("[RUNES] loaded index height: {:?}", height);
            }
            Err(e) => eprintln!("[RUNES] failed to load /index_height: {e:?}"),
        }
        self.provider = Some(provider);
        if let Err(err) = self.reload_live_outpoints(*self.index_height.read().unwrap()) {
            eprintln!("[RUNES] failed to initialize live outpoint mode: {err:?}");
        }
    }

    fn get_genesis_block(&self, network: Network) -> u32 {
        runes_genesis_block(network)
    }

    fn index_block(&self, block: EspoBlock) -> Result<()> {
        let t0 = Instant::now();
        if !self.config.enable {
            return Ok(());
        }
        if let Some(prev) = *self.index_height.read().unwrap() {
            if block.height <= prev {
                return Ok(());
            }
        }
        if self.config.load_live_outpoints {
            let mut live_outpoints = self.live_outpoints.write().unwrap();
            let mut indexer = BlockRunesIndexer::new(
                self.provider(),
                block.height,
                block.block_header.time,
                Some(&mut live_outpoints),
            );
            indexer.index_block(&block)?;
        } else {
            let mut indexer = BlockRunesIndexer::new(
                self.provider(),
                block.height,
                block.block_header.time,
                None,
            );
            indexer.index_block(&block)?;
        }
        *self.index_height.write().unwrap() = Some(block.height);
        if debug_enabled() {
            eprintln!(
                "[indexer] module=runes height={} index_block done in {:?}",
                block.height,
                t0.elapsed()
            );
        }
        Ok(())
    }

    fn get_index_height(&self) -> Option<u32> {
        *self.index_height.read().unwrap()
    }

    fn handle_reorg(&self, next_height: u32) -> Result<()> {
        if !self.config.enable {
            return Ok(());
        }
        self.provider().rollback_before_height(next_height)?;
        let height = self.provider().get_index_height()?;
        self.reload_live_outpoints(height)?;
        *self.index_height.write().unwrap() = height;
        eprintln!("[RUNES] reorg rollback complete; index height: {:?}", height);
        Ok(())
    }

    fn register_rpc(&self, reg: &RpcNsRegistrar) {
        if let Some(provider) = self.provider.as_ref() {
            rpc::register_rpc(reg, Arc::clone(provider));
        }
    }

    fn config_spec(&self) -> Option<&'static str> {
        Some(r#"{ "enable": true, "load_live_outpoints": true }"#)
    }

    fn set_config(&mut self, config: &serde_json::Value) -> Result<()> {
        self.config = serde_json::from_value(config.clone())?;
        Ok(())
    }
}

struct BlockRunesIndexer<'a> {
    provider: &'a RunesProvider,
    live_outpoints: Option<&'a mut HashSet<(Txid, u32)>>,
    live_outpoint_cache: HashMap<(Txid, u32), Option<OutpointRuneBalances>>,
    height: u32,
    timestamp: u64,
    network: Network,
    rules: TransferRules,
    ephem: HashMap<(Txid, u32), OutpointRuneBalances>,
    entries: HashMap<SchemaRuneId, RuneEntry>,
    rune_to_id: HashMap<u128, SchemaRuneId>,
    next_seq: Option<u64>,
    address_balance_cache: HashMap<(String, SchemaRuneId), u128>,
    address_balance_history_touched: HashSet<(String, SchemaRuneId)>,
    holder_balance_cache: HashMap<(SchemaRuneId, String), u128>,
    holder_count_cache: HashMap<SchemaRuneId, u64>,
    volume_cache: HashMap<(RuneVolumeKind, SchemaRuneId, String), u128>,
    volume_len_cache: HashMap<(RuneVolumeKind, SchemaRuneId), u32>,
    next_tx_pointer: Option<u64>,
    next_action_pointer: Option<u64>,
    next_block_chunk_id: Option<u64>,
    next_address_chunk_id: Option<u64>,
    next_action_block_chunk_id: Option<u64>,
    next_action_address_chunk_id: Option<u64>,
    block_tx_pointer_ids: Vec<u64>,
    address_tx_pointer_ids: HashMap<String, Vec<u64>>,
    action_block_pointer_ids: Vec<u64>,
    action_address_pointer_ids: HashMap<String, Vec<u64>>,
    action_candidates: BTreeMap<Txid, ActionCandidate>,
    uncommon_goods_usd_weighted_sum: U256,
    uncommon_goods_amount_sum: U256,
    puts: Vec<(Vec<u8>, Vec<u8>)>,
    deletes: HashSet<Vec<u8>>,
}

#[derive(Clone, Debug, Default)]
struct ActionCandidate {
    tx_index: u32,
    has_alkane: bool,
    has_rune: bool,
    addresses: HashSet<String>,
}

impl<'a> BlockRunesIndexer<'a> {
    fn new(
        provider: &'a RunesProvider,
        height: u32,
        timestamp: u32,
        live_outpoints: Option<&'a mut HashSet<(Txid, u32)>>,
    ) -> Self {
        Self {
            provider,
            live_outpoints,
            live_outpoint_cache: HashMap::new(),
            height,
            timestamp: timestamp as u64,
            network: get_network(),
            rules: TransferRules::default(),
            ephem: HashMap::new(),
            entries: HashMap::new(),
            rune_to_id: HashMap::new(),
            next_seq: None,
            address_balance_cache: HashMap::new(),
            address_balance_history_touched: HashSet::new(),
            holder_balance_cache: HashMap::new(),
            holder_count_cache: HashMap::new(),
            volume_cache: HashMap::new(),
            volume_len_cache: HashMap::new(),
            next_tx_pointer: None,
            next_action_pointer: None,
            next_block_chunk_id: None,
            next_address_chunk_id: None,
            next_action_block_chunk_id: None,
            next_action_address_chunk_id: None,
            block_tx_pointer_ids: Vec::new(),
            address_tx_pointer_ids: HashMap::new(),
            action_block_pointer_ids: Vec::new(),
            action_address_pointer_ids: HashMap::new(),
            action_candidates: BTreeMap::new(),
            uncommon_goods_usd_weighted_sum: U256::ZERO,
            uncommon_goods_amount_sum: U256::ZERO,
            puts: Vec::new(),
            deletes: HashSet::new(),
        }
    }

    fn live_outpoints_contains(&self, outpoint: &(Txid, u32)) -> bool {
        self.live_outpoints.as_ref().map(|set| set.contains(outpoint)).unwrap_or(false)
    }

    fn insert_live_outpoint(&mut self, outpoint: (Txid, u32)) {
        if let Some(live_outpoints) = self.live_outpoints.as_deref_mut() {
            live_outpoints.insert(outpoint);
        }
        self.live_outpoint_cache.remove(&outpoint);
    }

    fn remove_live_outpoint(&mut self, outpoint: (Txid, u32)) {
        if let Some(live_outpoints) = self.live_outpoints.as_deref_mut() {
            live_outpoints.remove(&outpoint);
        }
        self.live_outpoint_cache.remove(&outpoint);
    }

    fn get_point_lookup_outpoint(
        &mut self,
        outpoint: (Txid, u32),
    ) -> Result<Option<OutpointRuneBalances>> {
        if let Some(row) = self.live_outpoint_cache.get(&outpoint) {
            return Ok(row.clone());
        }
        let row = self.provider.get_outpoint_balances(&outpoint.0, outpoint.1)?;
        self.live_outpoint_cache.insert(outpoint, row.clone());
        Ok(row)
    }

    fn take_point_lookup_outpoint(
        &mut self,
        outpoint: (Txid, u32),
    ) -> Result<Option<OutpointRuneBalances>> {
        if let Some(row) = self.live_outpoint_cache.remove(&outpoint) {
            return Ok(row);
        }
        self.provider.get_outpoint_balances(&outpoint.0, outpoint.1)
    }

    fn has_rune_outpoint(&mut self, outpoint: (Txid, u32)) -> Result<bool> {
        if self.ephem.contains_key(&outpoint) || self.live_outpoints_contains(&outpoint) {
            return Ok(true);
        }
        if self.live_outpoints.is_some() {
            return Ok(false);
        }
        Ok(self.get_point_lookup_outpoint(outpoint)?.is_some())
    }

    fn index_block(&mut self, block: &EspoBlock) -> Result<()> {
        let t_scan = Instant::now();
        let block_tx_map: HashMap<Txid, &Transaction> = block
            .transactions
            .iter()
            .map(|atx| (atx.transaction.compute_txid(), &atx.transaction))
            .collect();
        let external_prev_map = load_external_prev_txs_for_rune_mints(block);
        for (tx_index, atx) in block.transactions.iter().enumerate() {
            self.index_tx(tx_index as u32, &atx.transaction, &block_tx_map, &external_prev_map)?;
        }
        self.collect_alkane_action_candidates(block, &block_tx_map)?;
        let scan_elapsed = t_scan.elapsed();
        let t_flush = Instant::now();
        self.flush_address_balance_history()?;
        self.flush_action_tx_indexes()?;
        self.flush_tx_indexes()?;
        if !self.uncommon_goods_amount_sum.is_zero() {
            let avg = self.uncommon_goods_usd_weighted_sum / self.uncommon_goods_amount_sum;
            self.puts.push((
                uncommon_goods_avg_price_usd_by_height_key(self.height),
                u256_to_be(avg).to_vec(),
            ));
        }
        let flush_elapsed = t_flush.elapsed();
        let delete_set = std::mem::take(&mut self.deletes);
        let mut puts = std::mem::take(&mut self.puts);
        if !delete_set.is_empty() {
            puts.retain(|(key, _)| !delete_set.contains(key));
        }
        let deletes: Vec<Vec<u8>> = delete_set.into_iter().collect();
        let put_count = puts.len();
        let delete_count = deletes.len();
        let block_hash = block.block_header.block_hash();
        let t_write = Instant::now();
        self.provider.set_block_batch(puts, deletes, self.height, &block_hash)?;
        if debug_enabled() {
            eprintln!(
                "[RUNES][block] height={} txs={} scan={:?} flush_tx_indexes={:?} write_batch={:?} puts={} deletes={}",
                self.height,
                block.transactions.len(),
                scan_elapsed,
                flush_elapsed,
                t_write.elapsed(),
                put_count,
                delete_count
            );
        }
        Ok(())
    }

    fn index_tx(
        &mut self,
        tx_index: u32,
        tx: &Transaction,
        block_tx_map: &HashMap<Txid, &Transaction>,
        external_prev_map: &HashMap<Txid, Transaction>,
    ) -> Result<()> {
        let txid = tx.compute_txid();
        if self.height == MAINNET_RUNES_GENESIS && tx_index == 0 {
            self.ensure_genesis_rune(txid)?;
        }
        let has_runestone = tx_has_runestone_carrier(tx);
        let mut has_rune_input = false;
        for input in &tx.input {
            if self.has_rune_outpoint((input.previous_output.txid, input.previous_output.vout))? {
                has_rune_input = true;
                break;
            }
        }
        if !has_runestone && !has_rune_input {
            return Ok(());
        }
        let artifact = has_runestone.then(|| Runestone::decipher(tx)).flatten();
        let mut touched_addresses = HashSet::new();
        let mut transfer_participants: HashMap<SchemaRuneId, HashSet<String>> = HashMap::new();
        let mut transfer_amounts: HashMap<SchemaRuneId, u128> = HashMap::new();
        let mut io = TxRuneIo::default();
        let mut unallocated =
            self.unallocated(tx, &mut touched_addresses, &mut transfer_participants, &mut io)?;
        if artifact.is_none() && unallocated.is_empty() {
            return Ok(());
        }

        let etched = match artifact.as_ref() {
            Some(artifact) => self.etched(tx_index, tx, artifact)?,
            None => None,
        };
        if let Some((id, rune, etching)) = etched {
            if let Some(artifact) = &artifact {
                if let Artifact::Runestone(runestone) = artifact {
                    *unallocated.entry(id).or_default() += runestone
                        .etching
                        .as_ref()
                        .and_then(|etching| etching.premine)
                        .unwrap_or_default();
                }
                self.create_rune_entry(id, rune, txid, tx, etching, artifact)?;
                io.etched = Some(id);
            }
        }

        let mut minted: Vec<RuneBalance> = Vec::new();
        if let Some(artifact) = &artifact {
            if let Some(id) = artifact.mint() {
                if let Some(amount) = self.mint(id.into(), txid, tx_index)? {
                    *unallocated.entry(id.into()).or_default() += amount;
                    let bal = RuneBalance { id: id.into(), amount };
                    minted.push(bal.clone());
                    io.minted.push(bal);
                }
            }
        }
        let mint_fee_paid_sats = if minted.is_empty() {
            0
        } else {
            compute_tx_fee_sats(tx, block_tx_map, external_prev_map)
        };
        let btc_usd_price = if minted.is_empty() { None } else { self.btc_usd_price_at_height() };

        let mut allocated: OutputRuneSheets<SchemaRuneId> = BTreeMap::new();
        if let Some(Artifact::Runestone(runestone)) = artifact.as_ref() {
            for Edict { id, amount, output } in runestone.edicts.iter().copied() {
                let resolved_id = if id == RuneId::default() {
                    let Some((etched_id, _, _)) = etched else {
                        continue;
                    };
                    etched_id
                } else {
                    id.into()
                };
                self.rules.apply_edict(
                    tx,
                    &mut unallocated,
                    &mut allocated,
                    resolved_id,
                    amount,
                    output,
                );
            }
        }

        let burned = if matches!(artifact, Some(Artifact::Cenotaph(_))) {
            unallocated
        } else {
            let pointer = match artifact.as_ref() {
                Some(Artifact::Runestone(runestone)) => runestone.pointer,
                _ => None,
            };
            self.rules.route_leftovers(tx, unallocated, &mut allocated, pointer)
        };
        io.burned = balances_from_sheet(&burned);

        for (id, amount) in burned {
            if amount == 0 {
                continue;
            }
            if let Some(mut entry) = self.load_entry(id)? {
                entry.burned = entry.burned.saturating_add(amount);
                self.store_entry(&entry)?;
            }
        }

        for (vout, sheet) in allocated {
            if sheet.is_empty() {
                continue;
            }
            let balances = balances_from_sheet(&sheet);
            if tx.output[vout as usize].script_pubkey.is_op_return() {
                for balance in balances {
                    if let Some(mut entry) = self.load_entry(balance.id)? {
                        entry.burned = entry.burned.saturating_add(balance.amount);
                        self.store_entry(&entry)?;
                    }
                    io.burned.push(balance);
                }
                continue;
            }
            let address = script_to_address(&tx.output[vout as usize].script_pubkey, self.network);
            let row = OutpointRuneBalances {
                address: address.clone(),
                script_pubkey: tx.output[vout as usize].script_pubkey.to_bytes(),
                balances: balances.clone(),
            };
            self.ephem.insert((txid, vout), row.clone());
            self.insert_live_outpoint((txid, vout));
            self.queue_put(outpoint_key(&txid, vout), encode(&row)?);
            if let Some(address) = address.as_ref() {
                self.queue_put(address_outpoint_key(address, &txid, vout), Vec::new());
                touched_addresses.insert(address.clone());
                for balance in &balances {
                    transfer_participants.entry(balance.id).or_default().insert(address.clone());
                    *transfer_amounts.entry(balance.id).or_default() = transfer_amounts
                        .get(&balance.id)
                        .copied()
                        .unwrap_or(0)
                        .saturating_add(balance.amount);
                    self.apply_volume_delta(
                        RuneVolumeKind::TotalReceived,
                        balance.id,
                        address,
                        balance.amount,
                    )?;
                    self.apply_address_delta(address, balance.id, balance.amount as i128)?;
                }
            }
            io.outputs.insert(vout, balances);
        }

        for (id, amount) in transfer_amounts {
            let Some(participants) = transfer_participants.get(&id) else {
                continue;
            };
            for address in participants {
                self.apply_volume_delta(RuneVolumeKind::TransferVolume, id, address, amount)?;
            }
        }

        for balance in minted {
            let destination = self.first_output_address_for_rune(txid, balance.id);
            let divisibility =
                self.load_entry(balance.id)?.map(|entry| entry.divisibility).unwrap_or_default();
            let mint_price_paid_sats =
                scale_rune_fee_price_sats(mint_fee_paid_sats, balance.amount, divisibility);
            let mint_price_paid_usd = scale_rune_fee_price_usd(mint_price_paid_sats, btc_usd_price);
            if balance.id == GENESIS_RUNE_ID && btc_usd_price.is_some() && balance.amount > 0 {
                let amount = U256::from(balance.amount);
                self.uncommon_goods_amount_sum =
                    self.uncommon_goods_amount_sum.saturating_add(amount);
                self.uncommon_goods_usd_weighted_sum =
                    self.uncommon_goods_usd_weighted_sum.saturating_add(
                        U256::from_be_bytes(mint_price_paid_usd).saturating_mul(amount),
                    );
            }
            let activity = RuneMintActivity {
                id: balance.id,
                txid: txid.to_byte_array(),
                chain_txids: vec![txid.to_byte_array()],
                cpfp: false,
                height: self.height,
                tx_index,
                timestamp: self.timestamp,
                amount: balance.amount,
                fee_paid_sats: mint_fee_paid_sats,
                mint_price_paid_sats,
                mint_price_paid_usd,
                destination: destination.clone(),
                success: true,
            };
            self.puts.push((
                mint_activity_key(balance.id, self.timestamp, &txid, tx_index),
                encode(&activity)?,
            ));
            self.queue_rune_activity(RuneActivity {
                id: balance.id,
                txid: txid.to_byte_array(),
                chain_txids: vec![txid.to_byte_array()],
                cpfp: false,
                height: self.height,
                tx_index,
                timestamp: self.timestamp,
                kind: RuneActivityKind::Mint,
                amount: balance.amount,
                fee_paid_sats: mint_fee_paid_sats,
                mint_price_paid_sats,
                mint_price_paid_usd,
                destination,
                success: true,
            })?;
        }

        if let Some(id) = io.etched {
            if let Some(entry) = self.load_entry(id)? {
                self.queue_rune_activity(RuneActivity {
                    id,
                    txid: txid.to_byte_array(),
                    chain_txids: vec![txid.to_byte_array()],
                    cpfp: false,
                    height: self.height,
                    tx_index,
                    timestamp: self.timestamp,
                    kind: RuneActivityKind::Etch,
                    amount: entry.premine,
                    fee_paid_sats: 0,
                    mint_price_paid_sats: [0u8; 32],
                    mint_price_paid_usd: [0u8; 32],
                    destination: self.first_output_address_for_rune(txid, id),
                    success: true,
                })?;
            }
        }

        if !io.inputs.is_empty()
            || !io.outputs.is_empty()
            || !io.burned.is_empty()
            || !io.minted.is_empty()
            || io.etched.is_some()
        {
            self.puts.push((tx_io_key(&txid), encode(&io)?));
            self.queue_tx_index(txid, tx_index, &io, touched_addresses)?;
        }

        Ok(())
    }

    fn unallocated(
        &mut self,
        tx: &Transaction,
        touched_addresses: &mut HashSet<String>,
        transfer_participants: &mut HashMap<SchemaRuneId, HashSet<String>>,
        io: &mut TxRuneIo,
    ) -> Result<RuneSheet<SchemaRuneId>> {
        let mut unallocated = RuneSheet::new();
        let spending_txid = tx.compute_txid().to_byte_array();
        for (input_idx, input) in tx.input.iter().enumerate() {
            let prev = input.previous_output;
            let outpoint = (prev.txid, prev.vout);
            let row = if let Some(row) = self.ephem.remove(&outpoint) {
                Some(row)
            } else if self.live_outpoints.is_some() {
                if !self.live_outpoints_contains(&outpoint) {
                    None
                } else {
                    self.provider.get_outpoint_balances(&prev.txid, prev.vout)?
                }
            } else {
                self.take_point_lookup_outpoint(outpoint)?
            };
            let Some(row) = row else {
                continue;
            };
            self.remove_live_outpoint(outpoint);
            self.queue_delete(outpoint_key(&prev.txid, prev.vout));
            io.inputs.insert(input_idx as u32, row.balances.clone());
            if let Some(address) = row.address.as_ref() {
                self.queue_delete(address_outpoint_key(address, &prev.txid, prev.vout));
                touched_addresses.insert(address.clone());
                for balance in &row.balances {
                    transfer_participants.entry(balance.id).or_default().insert(address.clone());
                    self.apply_address_delta(address, balance.id, -(balance.amount as i128))?;
                }
            }
            for balance in row.balances {
                *unallocated.entry(balance.id).or_default() += balance.amount;
            }
        }
        let _ = spending_txid;
        Ok(unallocated)
    }

    fn first_output_address_for_rune(&self, txid: Txid, id: SchemaRuneId) -> Option<String> {
        let mut candidates: Vec<(u32, String)> = self
            .ephem
            .iter()
            .filter_map(|((candidate_txid, vout), row)| {
                if *candidate_txid != txid {
                    return None;
                }
                let has_rune = row.balances.iter().any(|balance| balance.id == id);
                has_rune.then(|| row.address.clone().map(|address| (*vout, address))).flatten()
            })
            .collect();
        candidates.sort_by_key(|(vout, _)| *vout);
        candidates.into_iter().map(|(_, address)| address).next()
    }

    fn btc_usd_price_at_height(&self) -> Option<u128> {
        let essentials_provider = Arc::new(EssentialsProvider::new(Arc::new(Mdb::from_db(
            get_espo_db(),
            b"essentials:",
        ))));
        let amm_provider = AmmDataProvider::new(
            Arc::new(Mdb::from_db(get_espo_db(), b"ammdata:")),
            essentials_provider,
        );
        amm_provider
            .get_btc_usd_price_at_or_before_height(self.height)
            .ok()
            .flatten()
            .or_else(|| {
                AmmDataConfig::load_from_global_config()
                    .ok()
                    .map(|cfg| cfg.pre_ammdata_btc_usd_price)
                    .filter(|price| *price > 0)
            })
    }

    fn queue_rune_activity(&mut self, activity: RuneActivity) -> Result<()> {
        let scopes = match activity.kind {
            RuneActivityKind::Etch => [RuneActivityScope::All, RuneActivityScope::Etch],
            RuneActivityKind::Mint => [RuneActivityScope::All, RuneActivityScope::Mint],
        };
        for scope in scopes {
            for sort_by in [RuneActivitySortField::Timestamp, RuneActivitySortField::Amount] {
                self.puts
                    .push((rune_activity_index_key(&activity, scope, sort_by), encode(&activity)?));
                if let Some(address) = activity.destination.as_ref() {
                    self.puts.push((
                        rune_address_activity_index_key(&activity, address, scope, sort_by),
                        encode(&activity)?,
                    ));
                    self.puts.push((
                        rune_address_token_activity_index_key(&activity, address, scope, sort_by),
                        encode(&activity)?,
                    ));
                }
            }
        }
        Ok(())
    }

    fn queue_tx_index(
        &mut self,
        txid: Txid,
        tx_index: u32,
        io: &TxRuneIo,
        touched_addresses: HashSet<String>,
    ) -> Result<()> {
        let pointer_id = self.next_tx_pointer_id()?;
        self.puts.push((
            rune_tx_pointer_key(pointer_id),
            encode_rune_tx_pointer_blob(&txid, self.height, tx_index, io)?,
        ));
        self.block_tx_pointer_ids.push(pointer_id);
        let candidate = self.action_candidates.entry(txid).or_insert_with(|| ActionCandidate {
            tx_index,
            has_alkane: false,
            has_rune: false,
            addresses: HashSet::new(),
        });
        candidate.tx_index = candidate.tx_index.min(tx_index);
        candidate.has_rune = true;
        for address in touched_addresses {
            candidate.addresses.insert(address.clone());
            self.address_tx_pointer_ids.entry(address).or_default().push(pointer_id);
        }
        Ok(())
    }

    fn next_tx_pointer_id(&mut self) -> Result<u64> {
        let id = match self.next_tx_pointer {
            Some(id) => id,
            None => {
                let id = self
                    .provider
                    .mdb()
                    .get(&rune_tx_pointer_count_key())?
                    .and_then(|bytes| super::storage::decode_u64(&bytes))
                    .unwrap_or(0);
                self.next_tx_pointer = Some(id);
                id
            }
        };
        self.next_tx_pointer = Some(id.saturating_add(1));
        self.puts.push((rune_tx_pointer_count_key(), encode_u64(id.saturating_add(1))));
        Ok(id)
    }

    fn next_action_pointer_id(&mut self) -> Result<u64> {
        let id = match self.next_action_pointer {
            Some(id) => id,
            None => {
                let id = self
                    .provider
                    .mdb()
                    .get(&action_tx_pointer_count_key())?
                    .and_then(|bytes| super::storage::decode_u64(&bytes))
                    .unwrap_or(0);
                self.next_action_pointer = Some(id);
                id
            }
        };
        self.next_action_pointer = Some(id.saturating_add(1));
        self.puts
            .push((action_tx_pointer_count_key(), encode_u64(id.saturating_add(1))));
        Ok(id)
    }

    fn next_chunk_id(&mut self, kind: RuneTxIndexKind) -> Result<&mut u64> {
        let slot = match kind {
            RuneTxIndexKind::Block => &mut self.next_block_chunk_id,
            RuneTxIndexKind::Address => &mut self.next_address_chunk_id,
            RuneTxIndexKind::ActionBlock => &mut self.next_action_block_chunk_id,
            RuneTxIndexKind::ActionAddress => &mut self.next_action_address_chunk_id,
        };
        if slot.is_none() {
            let id = self
                .provider
                .mdb()
                .get(&rune_tx_chunk_counter_key(kind))?
                .and_then(|bytes| super::storage::decode_u64(&bytes))
                .unwrap_or(0);
            *slot = Some(id);
        }
        Ok(slot.as_mut().expect("chunk id initialized"))
    }

    fn collect_alkane_action_candidates(
        &mut self,
        block: &EspoBlock,
        block_tx_map: &HashMap<Txid, &Transaction>,
    ) -> Result<()> {
        let essentials =
            EssentialsProvider::new(Arc::new(Mdb::from_db(get_espo_db(), b"essentials:")));
        let list_id = address_index_list_id_alkane_block_txs(self.height as u64);
        let total = get_address_index_list_len(
            &essentials,
            StateAt::Latest,
            AddressIndexListKind::AlkaneBlockTxs,
            &list_id,
        )
        .unwrap_or(0);
        if total == 0 {
            return Ok(());
        }
        let ids = get_address_index_list_range(
            &essentials,
            StateAt::Latest,
            AddressIndexListKind::AlkaneBlockTxs,
            &list_id,
            0,
            total,
        )
        .unwrap_or_default();
        let mut alkane_txids = Vec::new();
        for id in ids {
            let Some(blob) = load_tx_pointer_blob_v3_by_id(&essentials, id) else {
                continue;
            };
            let txid = Txid::from_byte_array(blob.txid);
            let candidate = self.action_candidates.entry(txid).or_insert_with(|| ActionCandidate {
                tx_index: blob.tx_idx,
                has_alkane: false,
                has_rune: false,
                addresses: HashSet::new(),
            });
            candidate.tx_index = candidate.tx_index.min(blob.tx_idx);
            candidate.has_alkane = true;
            alkane_txids.push(txid);
        }
        if alkane_txids.is_empty() {
            return Ok(());
        }

        let external_prev_map = load_external_prev_txs_for_action_addresses(block, &alkane_txids);
        for txid in alkane_txids {
            let Some(tx) = block_tx_map.get(&txid).copied() else {
                continue;
            };
            let addresses =
                bitcoin_addresses_for_tx(tx, block_tx_map, &external_prev_map, self.network);
            if let Some(candidate) = self.action_candidates.get_mut(&txid) {
                candidate.addresses.extend(addresses);
            }
        }
        Ok(())
    }

    fn flush_action_tx_indexes(&mut self) -> Result<()> {
        let mut candidates: Vec<(Txid, ActionCandidate)> =
            std::mem::take(&mut self.action_candidates).into_iter().collect();
        candidates.sort_by_key(|(txid, candidate)| (candidate.tx_index, *txid));
        let mut pointer_ids_by_txid = HashMap::new();
        for (txid, candidate) in &candidates {
            if !candidate.has_alkane && !candidate.has_rune {
                continue;
            }
            let pointer_id = self.next_action_pointer_id()?;
            self.puts.push((
                action_tx_pointer_key(pointer_id),
                encode_action_tx_pointer_blob(
                    txid,
                    self.height,
                    candidate.tx_index,
                    candidate.has_alkane,
                    candidate.has_rune,
                )?,
            ));
            pointer_ids_by_txid.insert(*txid, pointer_id);
            self.action_block_pointer_ids.push(pointer_id);
        }

        for (txid, candidate) in candidates {
            let Some(pointer_id) = pointer_ids_by_txid.get(&txid).copied() else {
                continue;
            };
            for address in candidate.addresses {
                self.action_address_pointer_ids.entry(address).or_default().push(pointer_id);
            }
        }

        if !self.action_block_pointer_ids.is_empty() {
            let values = std::mem::take(&mut self.action_block_pointer_ids);
            let mut next_chunk_id = *self.next_chunk_id(RuneTxIndexKind::ActionBlock)?;
            append_rune_tx_index_values(
                self.provider,
                RuneTxIndexKind::ActionBlock,
                action_tx_block_list_key(self.height as u64),
                &values,
                &mut next_chunk_id,
                &mut self.puts,
            )?;
            self.next_action_block_chunk_id = Some(next_chunk_id);
            self.puts.push((
                rune_tx_chunk_counter_key(RuneTxIndexKind::ActionBlock),
                encode_u64(next_chunk_id),
            ));
        }

        let address_tx_pointer_ids = std::mem::take(&mut self.action_address_pointer_ids);
        if !address_tx_pointer_ids.is_empty() {
            let mut next_chunk_id = *self.next_chunk_id(RuneTxIndexKind::ActionAddress)?;
            for (address, values) in address_tx_pointer_ids {
                append_rune_tx_index_values(
                    self.provider,
                    RuneTxIndexKind::ActionAddress,
                    action_tx_address_list_key(&address),
                    &values,
                    &mut next_chunk_id,
                    &mut self.puts,
                )?;
            }
            self.next_action_address_chunk_id = Some(next_chunk_id);
            self.puts.push((
                rune_tx_chunk_counter_key(RuneTxIndexKind::ActionAddress),
                encode_u64(next_chunk_id),
            ));
        }
        Ok(())
    }

    fn flush_tx_indexes(&mut self) -> Result<()> {
        if !self.block_tx_pointer_ids.is_empty() {
            let values = std::mem::take(&mut self.block_tx_pointer_ids);
            let mut next_chunk_id = *self.next_chunk_id(RuneTxIndexKind::Block)?;
            append_rune_tx_index_values(
                self.provider,
                RuneTxIndexKind::Block,
                rune_tx_block_list_key(self.height as u64),
                &values,
                &mut next_chunk_id,
                &mut self.puts,
            )?;
            self.next_block_chunk_id = Some(next_chunk_id);
            self.puts.push((
                rune_tx_chunk_counter_key(RuneTxIndexKind::Block),
                encode_u64(next_chunk_id),
            ));
        }

        let address_tx_pointer_ids = std::mem::take(&mut self.address_tx_pointer_ids);
        if !address_tx_pointer_ids.is_empty() {
            let mut next_chunk_id = *self.next_chunk_id(RuneTxIndexKind::Address)?;
            for (address, values) in address_tx_pointer_ids {
                append_rune_tx_index_values(
                    self.provider,
                    RuneTxIndexKind::Address,
                    rune_tx_address_list_key(&address),
                    &values,
                    &mut next_chunk_id,
                    &mut self.puts,
                )?;
            }
            self.next_address_chunk_id = Some(next_chunk_id);
            self.puts.push((
                rune_tx_chunk_counter_key(RuneTxIndexKind::Address),
                encode_u64(next_chunk_id),
            ));
        }
        Ok(())
    }

    fn mint(&mut self, id: SchemaRuneId, txid: Txid, tx_index: u32) -> Result<Option<u128>> {
        let Some(mut entry) = self.load_entry(id)? else {
            return Ok(None);
        };
        let Some(amount) = entry.mintable(self.height as u64, tx_index) else {
            return Ok(None);
        };
        entry.mints = entry.mints.saturating_add(1);
        self.store_entry(&entry)?;
        let _ = (txid, tx_index);
        Ok(Some(amount))
    }

    fn ensure_genesis_rune(&mut self, txid: Txid) -> Result<()> {
        if self.load_entry(GENESIS_RUNE_ID)?.is_some() {
            return Ok(());
        }
        let terms = ordinals::Terms {
            amount: Some(1),
            cap: Some(u128::MAX),
            height: (Some(MAINNET_RUNES_GENESIS as u64), Some(1_050_000)),
            offset: (None, None),
        };
        let entry = make_entry(
            GENESIS_RUNE_ID,
            Rune::from_str("UNCOMMONGOODS")?,
            128,
            Some('⧉'),
            0,
            0,
            Some(terms),
            txid,
            self.next_sequence()?,
            self.timestamp,
            true,
        );
        self.store_entry(&entry)?;
        self.rune_to_id.insert(entry.rune, entry.id);
        self.puts
            .push((id_by_rune_key(Rune::from_str("UNCOMMONGOODS")?), encode(&entry.id)?));
        self.puts.push((id_by_name_key(&entry.name), encode(&entry.id)?));
        self.puts.push((id_by_name_key(&entry.spaced_name), encode(&entry.id)?));
        self.puts.push((seq_key(entry.number), encode(&entry.id)?));
        Ok(())
    }

    fn etched(
        &mut self,
        tx_index: u32,
        tx: &Transaction,
        artifact: &Artifact,
    ) -> Result<Option<(SchemaRuneId, Rune, Option<Etching>)>> {
        let rune_opt = match artifact {
            Artifact::Runestone(runestone) => match runestone.etching {
                Some(etching) => etching.rune,
                None => return Ok(None),
            },
            Artifact::Cenotaph(cenotaph) => match cenotaph.etching {
                Some(rune) => Some(rune),
                None => return Ok(None),
            },
        };
        let rune = if let Some(rune) = rune_opt {
            let minimum = Rune::minimum_at_height(self.network, Height(self.height));
            if rune < minimum
                || rune.is_reserved()
                || self.rune_exists(rune)?
                || !self.tx_commits_to_rune(tx, rune)
            {
                return Ok(None);
            }
            rune
        } else {
            Rune::reserved(self.height as u64, tx_index)
        };
        let etching = match artifact {
            Artifact::Runestone(runestone) => runestone.etching,
            Artifact::Cenotaph(_) => None,
        };
        Ok(Some((SchemaRuneId { block: self.height as u64, tx: tx_index }, rune, etching)))
    }

    fn create_rune_entry(
        &mut self,
        id: SchemaRuneId,
        rune: Rune,
        txid: Txid,
        tx: &Transaction,
        etching: Option<Etching>,
        artifact: &Artifact,
    ) -> Result<()> {
        let number = self.next_sequence()?;
        let entry = match artifact {
            Artifact::Cenotaph(_) => {
                make_entry(id, rune, 0, None, 0, 0, None, txid, number, self.timestamp, false)
            }
            Artifact::Runestone(_) => {
                let etching = etching.unwrap_or_default();
                make_entry(
                    id,
                    rune,
                    etching.spacers.unwrap_or_default(),
                    etching.symbol,
                    etching.divisibility.unwrap_or_default(),
                    etching.premine.unwrap_or_default(),
                    etching.terms,
                    txid,
                    number,
                    self.timestamp,
                    etching.turbo,
                )
            }
        };
        self.store_entry(&entry)?;
        self.rune_to_id.insert(rune.0, id);
        self.puts.push((id_by_rune_key(rune), encode(&id)?));
        self.puts.push((id_by_name_key(&entry.name), encode(&id)?));
        self.puts.push((id_by_name_key(&entry.spaced_name), encode(&id)?));
        self.puts.push((seq_key(number), encode(&id)?));
        if let Some(icon) = self.rune_icon_from_etching_tx(tx) {
            self.puts.push((rune_icon_key(id), encode(&icon)?));
        }
        Ok(())
    }

    fn rune_icon_from_etching_tx(&self, tx: &Transaction) -> Option<RuneIcon> {
        if let Some(icon) = image_inscription_from_tx(tx) {
            return Some(icon);
        }

        let delegate = delegate_inscription_from_tx(tx)?;
        let delegate_tx =
            get_bitcoind_rpc_client().get_raw_transaction(&delegate.txid, None).ok()?;
        image_inscription_from_tx_at(&delegate_tx, delegate.index)
    }

    fn next_sequence(&mut self) -> Result<u64> {
        let seq = match self.next_seq {
            Some(seq) => seq,
            None => {
                let seq = self
                    .provider
                    .mdb()
                    .get(&seq_count_key())?
                    .and_then(|bytes| super::storage::decode_u64(&bytes))
                    .unwrap_or(0);
                self.next_seq = Some(seq);
                seq
            }
        };
        self.next_seq = Some(seq.saturating_add(1));
        self.puts.push((seq_count_key(), encode_u64(seq.saturating_add(1))));
        Ok(seq)
    }

    fn load_entry(&mut self, id: SchemaRuneId) -> Result<Option<RuneEntry>> {
        if let Some(entry) = self.entries.get(&id) {
            return Ok(Some(entry.clone()));
        }
        let entry = self.provider.get_rune(id)?;
        if let Some(entry) = entry.as_ref() {
            self.entries.insert(id, entry.clone());
            self.rune_to_id.insert(entry.rune, id);
        }
        Ok(entry)
    }

    fn store_entry(&mut self, entry: &RuneEntry) -> Result<()> {
        self.entries.insert(entry.id, entry.clone());
        self.puts.push((entry_key(entry.id), encode(entry)?));
        Ok(())
    }

    fn queue_put(&mut self, key: Vec<u8>, value: Vec<u8>) {
        self.deletes.remove(&key);
        self.puts.push((key, value));
    }

    fn queue_delete(&mut self, key: Vec<u8>) {
        self.deletes.insert(key);
    }

    fn rune_exists(&mut self, rune: Rune) -> Result<bool> {
        if self.rune_to_id.contains_key(&rune.0) {
            return Ok(true);
        }
        Ok(self.provider.mdb().get(&id_by_rune_key(rune))?.is_some())
    }

    fn tx_commits_to_rune(&self, tx: &Transaction, rune: Rune) -> bool {
        let commitment = rune.commitment();
        for input in &tx.input {
            #[allow(deprecated)]
            let Some(tapscript) = input.witness.tapscript() else {
                continue;
            };
            let mut matched = false;
            for instruction in tapscript.instructions() {
                let Ok(instruction) = instruction else {
                    break;
                };
                let Some(pushbytes) = instruction.push_bytes() else {
                    continue;
                };
                if pushbytes.as_bytes() == commitment {
                    matched = true;
                    break;
                }
            }
            if !matched {
                continue;
            }
            if self.commitment_input_is_mature(input.previous_output) {
                return true;
            }
        }
        false
    }

    fn commitment_input_is_mature(&self, outpoint: OutPoint) -> bool {
        let Ok(info) = get_bitcoind_rpc_client().get_raw_transaction_info(&outpoint.txid, None)
        else {
            return false;
        };
        let Some(prev_out) = info.vout.get(outpoint.vout as usize) else {
            return false;
        };
        let Ok(script) = prev_out.script_pub_key.script() else {
            return false;
        };
        if !script.is_p2tr() {
            return false;
        }
        let Some(blockhash) = info.blockhash else {
            return false;
        };
        let Ok(header) = get_bitcoind_rpc_client().get_block_header_info(&blockhash) else {
            return false;
        };
        let Ok(commit_height) = u32::try_from(header.height) else {
            return false;
        };
        self.height.saturating_sub(commit_height).saturating_add(1)
            >= Runestone::COMMIT_CONFIRMATIONS as u32
    }

    fn apply_address_delta(&mut self, address: &str, id: SchemaRuneId, delta: i128) -> Result<()> {
        let address_cache_key = (address.to_string(), id);
        let key = address_balance_key(address, id);
        let prev = match self.address_balance_cache.get(&address_cache_key).copied() {
            Some(value) => value,
            None => {
                let value = self
                    .provider
                    .mdb()
                    .get(&key)?
                    .and_then(|bytes| super::storage::decode_u128(&bytes))
                    .unwrap_or(0);
                self.address_balance_cache.insert(address_cache_key.clone(), value);
                value
            }
        };
        let next = if delta.is_negative() {
            prev.saturating_sub(delta.unsigned_abs())
        } else {
            prev.saturating_add(delta as u128)
        };
        self.address_balance_cache.insert(address_cache_key, next);
        self.address_balance_history_touched.insert((address.to_string(), id));
        if next == 0 {
            self.queue_delete(key);
        } else {
            self.queue_put(key, encode_u128(next));
        }

        let holder_cache_key = (id, address.to_string());
        let hkey = holder_key(id, address);
        let hprev = match self.holder_balance_cache.get(&holder_cache_key).copied() {
            Some(value) => value,
            None => {
                let value = self
                    .provider
                    .mdb()
                    .get(&hkey)?
                    .and_then(|bytes| super::storage::decode_u128(&bytes))
                    .unwrap_or(0);
                self.holder_balance_cache.insert(holder_cache_key.clone(), value);
                value
            }
        };
        let hnext = if delta.is_negative() {
            hprev.saturating_sub(delta.unsigned_abs())
        } else {
            hprev.saturating_add(delta as u128)
        };
        self.holder_balance_cache.insert(holder_cache_key, hnext);
        if hnext == 0 {
            self.queue_delete(hkey);
        } else {
            self.queue_put(hkey, encode_u128(hnext));
        }

        let count_key = holders_count_key(id);
        let count = match self.holder_count_cache.get(&id).copied() {
            Some(value) => value,
            None => {
                let value = self
                    .provider
                    .mdb()
                    .get(&count_key)?
                    .and_then(|bytes| super::storage::decode_u64(&bytes))
                    .unwrap_or(0);
                self.holder_count_cache.insert(id, value);
                value
            }
        };
        let next_count = match (hprev == 0, hnext == 0) {
            (true, false) => count.saturating_add(1),
            (false, true) => count.saturating_sub(1),
            _ => count,
        };
        if next_count != count {
            self.holder_count_cache.insert(id, next_count);
            self.queue_put(count_key, encode_u64(next_count));
        }
        Ok(())
    }

    fn flush_address_balance_history(&mut self) -> Result<()> {
        let touched: Vec<(String, SchemaRuneId)> =
            self.address_balance_history_touched.drain().collect();
        for (address, id) in touched {
            let amount =
                self.address_balance_cache.get(&(address.clone(), id)).copied().unwrap_or(0);
            self.queue_put(
                address_balance_history_key(&address, id, self.height),
                encode_u128(amount),
            );
            let len_key = address_balance_history_list_len_key(&address, id);
            let len = self
                .provider
                .mdb()
                .get(&len_key)?
                .and_then(|bytes| super::storage::decode_u32(&bytes))
                .unwrap_or(0);
            self.queue_put(
                address_balance_history_list_idx_key(&address, id, len),
                self.height.to_be_bytes().to_vec(),
            );
            self.queue_put(len_key, encode_u32(len.saturating_add(1)));
        }
        Ok(())
    }

    fn apply_volume_delta(
        &mut self,
        kind: RuneVolumeKind,
        id: SchemaRuneId,
        address: &str,
        delta: u128,
    ) -> Result<()> {
        if delta == 0 {
            return Ok(());
        }
        let cache_key = (kind, id, address.to_string());
        let key = rune_volume_entry_key(kind, id, address);
        let (prev, had_row) = match self.volume_cache.get(&cache_key).copied() {
            Some(value) => (value, true),
            None => {
                let raw = self.provider.mdb().get(&key)?;
                let had_row = raw.is_some();
                let value = raw.and_then(|bytes| super::storage::decode_u128(&bytes)).unwrap_or(0);
                (value, had_row)
            }
        };
        let next = prev.saturating_add(delta);
        self.volume_cache.insert(cache_key, next);
        self.queue_put(key, encode_u128(next));

        if had_row {
            return Ok(());
        }

        let len_key = rune_volume_list_len_key(kind, id);
        let len_cache_key = (kind, id);
        let len = match self.volume_len_cache.get(&len_cache_key).copied() {
            Some(value) => value,
            None => {
                let value = self
                    .provider
                    .mdb()
                    .get(&len_key)?
                    .and_then(|bytes| super::storage::decode_u32(&bytes))
                    .unwrap_or(0);
                self.volume_len_cache.insert(len_cache_key, value);
                value
            }
        };
        self.queue_put(rune_volume_list_idx_key(kind, id, len), address.as_bytes().to_vec());
        let next_len = len.saturating_add(1);
        self.volume_len_cache.insert(len_cache_key, next_len);
        self.queue_put(len_key, encode_u32(next_len));
        Ok(())
    }
}

fn balances_from_sheet(sheet: &RuneSheet<SchemaRuneId>) -> Vec<RuneBalance> {
    sheet
        .iter()
        .filter_map(|(id, amount)| {
            (*amount > 0).then_some(RuneBalance { id: *id, amount: *amount })
        })
        .collect()
}

fn load_external_prev_txs_for_rune_mints(block: &EspoBlock) -> HashMap<Txid, Transaction> {
    let block_txids: HashSet<Txid> =
        block.transactions.iter().map(|atx| atx.transaction.compute_txid()).collect();
    let mut needed = Vec::new();
    let mut seen = HashSet::new();
    for atx in &block.transactions {
        let tx = &atx.transaction;
        if !tx_has_runestone_carrier(tx) {
            continue;
        }
        let has_mint =
            Runestone::decipher(tx).as_ref().and_then(|artifact| artifact.mint()).is_some();
        if !has_mint {
            continue;
        }
        for input in &tx.input {
            if input.previous_output.is_null() || block_txids.contains(&input.previous_output.txid)
            {
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
        let Some(prev_out) = prev_tx.output.get(input.previous_output.vout as usize) else {
            return 0;
        };
        input_total = input_total.saturating_add(prev_out.value.to_sat() as u128);
    }
    let output_total = tx
        .output
        .iter()
        .fold(0u128, |acc, output| acc.saturating_add(output.value.to_sat() as u128));
    input_total.saturating_sub(output_total)
}

fn scale_rune_fee_price_sats(
    fee_paid_sats: u128,
    minted_amount: u128,
    divisibility: u8,
) -> [u8; 32] {
    if fee_paid_sats == 0 || minted_amount == 0 {
        return [0u8; 32];
    }
    let mut unit_scale = U256::from(1u8);
    for _ in 0..divisibility {
        unit_scale = unit_scale.saturating_mul(U256::from(10u8));
    }
    let price = U256::from(fee_paid_sats)
        .saturating_mul(unit_scale)
        .saturating_mul(U256::from(PRICE_SCALE))
        / U256::from(minted_amount);
    u256_to_be(price)
}

fn scale_rune_fee_price_usd(
    mint_price_paid_sats: [u8; 32],
    btc_price_usd_scaled: Option<u128>,
) -> [u8; 32] {
    let Some(btc_price_usd_scaled) = btc_price_usd_scaled else {
        return [0u8; 32];
    };
    let sats_scaled = U256::from_be_bytes(mint_price_paid_sats);
    if sats_scaled.is_zero() {
        return [0u8; 32];
    }
    let usd_scaled = sats_scaled.saturating_mul(U256::from(btc_price_usd_scaled))
        / U256::from(PRICE_SCALE.saturating_mul(SATS_PER_BTC));
    u256_to_be(usd_scaled)
}

fn u256_to_be(value: U256) -> [u8; 32] {
    value.to_be_bytes::<32>()
}

fn load_external_prev_txs_for_action_addresses(
    block: &EspoBlock,
    txids: &[Txid],
) -> HashMap<Txid, Transaction> {
    let block_tx_map: HashMap<Txid, &Transaction> = block
        .transactions
        .iter()
        .map(|atx| (atx.transaction.compute_txid(), &atx.transaction))
        .collect();
    let mut needed = Vec::new();
    let mut seen = HashSet::new();
    for txid in txids {
        let Some(tx) = block_tx_map.get(txid).copied() else {
            continue;
        };
        for input in &tx.input {
            if input.previous_output.is_null()
                || block_tx_map.contains_key(&input.previous_output.txid)
            {
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

fn bitcoin_addresses_for_tx(
    tx: &Transaction,
    block_tx_map: &HashMap<Txid, &Transaction>,
    external_prev_map: &HashMap<Txid, Transaction>,
    network: Network,
) -> HashSet<String> {
    let mut addresses = HashSet::new();
    for output in &tx.output {
        if output.script_pubkey.is_op_return() {
            continue;
        }
        if let Some(address) = script_to_address(&output.script_pubkey, network) {
            addresses.insert(address);
        }
    }
    for input in &tx.input {
        if input.previous_output.is_null() {
            continue;
        }
        let prev_tx = block_tx_map
            .get(&input.previous_output.txid)
            .copied()
            .or_else(|| external_prev_map.get(&input.previous_output.txid));
        let Some(prev_tx) = prev_tx else { continue };
        let Some(prev_out) = prev_tx.output.get(input.previous_output.vout as usize) else {
            continue;
        };
        if let Some(address) = script_to_address(&prev_out.script_pubkey, network) {
            addresses.insert(address);
        }
    }
    addresses
}

fn tx_has_runestone_carrier(tx: &Transaction) -> bool {
    tx.output.iter().any(|output| {
        let mut instructions = output.script_pubkey.instructions();
        matches!(instructions.next(), Some(Ok(Instruction::Op(opcodes::all::OP_RETURN))))
            && matches!(instructions.next(), Some(Ok(Instruction::Op(opcodes::all::OP_PUSHNUM_13))))
    })
}
