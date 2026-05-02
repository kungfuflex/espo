use super::inscriptions::{
    RuneIcon, delegate_inscription_from_tx, image_inscription_from_tx, image_inscription_from_tx_at,
};
use super::rpc;
use super::storage::{
    OutpointRuneBalances, RuneBalance, RuneEntry, RuneMintActivity, RuneTxIndexKind, RunesProvider,
    SchemaRuneId, TxRuneIo, address_balance_key, append_rune_tx_index_values, encode,
    encode_rune_tx_pointer_blob, encode_u64, encode_u128, entry_key, holder_key, holders_count_key,
    id_by_name_key, id_by_rune_key, make_entry, mint_activity_key, outpoint_key, rune_icon_key,
    rune_tx_address_list_key, rune_tx_block_list_key, rune_tx_chunk_counter_key,
    rune_tx_pointer_count_key, rune_tx_pointer_key, script_to_address, seq_count_key, seq_key,
    tx_io_key,
};
use super::transfer::{OutputRuneSheets, RuneSheet, RunestoneTransfer, TransferRules};
use crate::alkanes::trace::EspoBlock;
use crate::config::{get_bitcoind_rpc_client, get_network};
use crate::modules::defs::{EspoModule, RpcNsRegistrar};
use crate::runtime::mdb::Mdb;
use anyhow::Result;
use bitcoin::hashes::Hash;
use bitcoin::{Network, OutPoint, Transaction, Txid};
use bitcoincore_rpc::RpcApi;
use ordinals::{Artifact, Edict, Etching, Height, Rune, RuneId, Runestone};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Arc, RwLock};

const MAINNET_RUNES_GENESIS: u32 = 840_000;
const GENESIS_RUNE_ID: SchemaRuneId = SchemaRuneId { block: 1, tx: 0 };

#[derive(Debug, Clone, Deserialize)]
pub struct RunesConfig {
    #[serde(default)]
    pub enable: bool,
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
    config: RunesConfig,
}

impl Runes {
    pub fn new() -> Self {
        Self {
            provider: None,
            index_height: Arc::new(RwLock::new(None)),
            config: RunesConfig { enable: false },
        }
    }

    fn provider(&self) -> &RunesProvider {
        self.provider.as_ref().expect("ModuleRegistry must call set_mdb").as_ref()
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
            Ok(height) => {
                *self.index_height.write().unwrap() = height;
                eprintln!("[RUNES] loaded index height: {:?}", height);
            }
            Err(e) => eprintln!("[RUNES] failed to load /index_height: {e:?}"),
        }
        self.provider = Some(provider);
    }

    fn get_genesis_block(&self, network: Network) -> u32 {
        match network {
            Network::Bitcoin => MAINNET_RUNES_GENESIS,
            Network::Regtest | Network::Signet => 0,
            Network::Testnet => Rune::first_rune_height(Network::Testnet),
            _ => MAINNET_RUNES_GENESIS,
        }
    }

    fn index_block(&self, block: EspoBlock) -> Result<()> {
        if !self.config.enable {
            return Ok(());
        }
        if let Some(prev) = *self.index_height.read().unwrap() {
            if block.height <= prev {
                return Ok(());
            }
        }
        let mut indexer =
            BlockRunesIndexer::new(self.provider(), block.height, block.block_header.time);
        indexer.index_block(&block)?;
        self.provider().set_index_height(block.height)?;
        *self.index_height.write().unwrap() = Some(block.height);
        Ok(())
    }

    fn get_index_height(&self) -> Option<u32> {
        *self.index_height.read().unwrap()
    }

    fn register_rpc(&self, reg: &RpcNsRegistrar) {
        if let Some(provider) = self.provider.as_ref() {
            rpc::register_rpc(reg, Arc::clone(provider));
        }
    }

    fn config_spec(&self) -> Option<&'static str> {
        Some(r#"{ "enable": true }"#)
    }

    fn set_config(&mut self, config: &serde_json::Value) -> Result<()> {
        self.config = serde_json::from_value(config.clone())?;
        Ok(())
    }
}

struct BlockRunesIndexer<'a> {
    provider: &'a RunesProvider,
    height: u32,
    timestamp: u64,
    network: Network,
    rules: TransferRules,
    ephem: HashMap<(Txid, u32), OutpointRuneBalances>,
    entries: HashMap<SchemaRuneId, RuneEntry>,
    rune_to_id: HashMap<u128, SchemaRuneId>,
    next_seq: Option<u64>,
    address_balance_cache: HashMap<(String, SchemaRuneId), u128>,
    holder_balance_cache: HashMap<(SchemaRuneId, String), u128>,
    holder_count_cache: HashMap<SchemaRuneId, u64>,
    next_tx_pointer: Option<u64>,
    next_block_chunk_id: Option<u64>,
    next_address_chunk_id: Option<u64>,
    block_tx_pointer_ids: Vec<u64>,
    address_tx_pointer_ids: HashMap<String, Vec<u64>>,
    puts: Vec<(Vec<u8>, Vec<u8>)>,
    deletes: Vec<Vec<u8>>,
}

impl<'a> BlockRunesIndexer<'a> {
    fn new(provider: &'a RunesProvider, height: u32, timestamp: u32) -> Self {
        Self {
            provider,
            height,
            timestamp: timestamp as u64,
            network: get_network(),
            rules: TransferRules::default(),
            ephem: HashMap::new(),
            entries: HashMap::new(),
            rune_to_id: HashMap::new(),
            next_seq: None,
            address_balance_cache: HashMap::new(),
            holder_balance_cache: HashMap::new(),
            holder_count_cache: HashMap::new(),
            next_tx_pointer: None,
            next_block_chunk_id: None,
            next_address_chunk_id: None,
            block_tx_pointer_ids: Vec::new(),
            address_tx_pointer_ids: HashMap::new(),
            puts: Vec::new(),
            deletes: Vec::new(),
        }
    }

    fn index_block(&mut self, block: &EspoBlock) -> Result<()> {
        for (tx_index, atx) in block.transactions.iter().enumerate() {
            self.index_tx(tx_index as u32, &atx.transaction)?;
        }
        self.flush_tx_indexes()?;
        let puts = std::mem::take(&mut self.puts);
        let deletes = std::mem::take(&mut self.deletes);
        self.provider.set_batch(puts, deletes)?;
        Ok(())
    }

    fn index_tx(&mut self, tx_index: u32, tx: &Transaction) -> Result<()> {
        let txid = tx.compute_txid();
        if self.height == MAINNET_RUNES_GENESIS && tx_index == 0 {
            self.ensure_genesis_rune(txid)?;
        }
        let artifact = Runestone::decipher(tx);
        let mut touched_addresses = HashSet::new();
        let mut unallocated = self.unallocated(tx, &mut touched_addresses)?;
        if artifact.is_none() && unallocated.is_empty() {
            return Ok(());
        }

        let mut io = TxRuneIo::default();
        for (input_idx, input) in tx.input.iter().enumerate() {
            let key = (input.previous_output.txid, input.previous_output.vout);
            if let Some(prev) = self.provider.get_outpoint_balances(&key.0, key.1)? {
                if let Some(address) = prev.address.as_ref() {
                    touched_addresses.insert(address.clone());
                }
                io.inputs.insert(input_idx as u32, prev.balances);
            }
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
            self.queue_put(outpoint_key(&txid, vout), encode(&row)?);
            if let Some(address) = address.as_ref() {
                touched_addresses.insert(address.clone());
                for balance in &balances {
                    self.apply_address_delta(address, balance.id, balance.amount as i128)?;
                }
            }
            io.outputs.insert(vout, balances);
        }

        for balance in minted {
            if let Some(((_, vout), row)) = self.ephem.iter().find(|((tid, _), _)| *tid == txid) {
                let destination = row.address.clone();
                let activity = RuneMintActivity {
                    id: balance.id,
                    txid: txid.to_byte_array(),
                    height: self.height,
                    tx_index,
                    timestamp: self.timestamp,
                    amount: balance.amount,
                    destination,
                };
                self.puts.push((
                    mint_activity_key(balance.id, self.timestamp, &txid, *vout),
                    encode(&activity)?,
                ));
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
    ) -> Result<RuneSheet<SchemaRuneId>> {
        let mut unallocated = RuneSheet::new();
        let spending_txid = tx.compute_txid().to_byte_array();
        for input in &tx.input {
            let prev = input.previous_output;
            let row = if let Some(row) = self.ephem.remove(&(prev.txid, prev.vout)) {
                Some(row)
            } else {
                self.provider.get_outpoint_balances(&prev.txid, prev.vout)?
            };
            let Some(row) = row else {
                continue;
            };
            self.queue_delete(outpoint_key(&prev.txid, prev.vout));
            if let Some(address) = row.address.as_ref() {
                touched_addresses.insert(address.clone());
                for balance in &row.balances {
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
        for address in touched_addresses {
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

    fn next_chunk_id(&mut self, kind: RuneTxIndexKind) -> Result<&mut u64> {
        let slot = match kind {
            RuneTxIndexKind::Block => &mut self.next_block_chunk_id,
            RuneTxIndexKind::Address => &mut self.next_address_chunk_id,
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
        self.deletes.retain(|pending| pending != &key);
        self.puts.push((key, value));
    }

    fn queue_delete(&mut self, key: Vec<u8>) {
        self.puts.retain(|(pending, _)| pending != &key);
        if !self.deletes.iter().any(|pending| pending == &key) {
            self.deletes.push(key);
        }
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
}

fn balances_from_sheet(sheet: &RuneSheet<SchemaRuneId>) -> Vec<RuneBalance> {
    sheet
        .iter()
        .filter_map(|(id, amount)| {
            (*amount > 0).then_some(RuneBalance { id: *id, amount: *amount })
        })
        .collect()
}
