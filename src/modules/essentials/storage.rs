use crate::alkanes::trace::{
    EspoSandshrewLikeTrace, EspoSandshrewLikeTraceEvent, EspoTrace, prettyify_protobuf_trace_json,
};
use crate::config::{get_bitcoind_rpc_client, get_electrum_like, get_metashrew, get_network};
use crate::modules::essentials::utils::balances::{
    SignedU128, get_address_activity_for_address, get_alkane_balances,
    get_alkane_balances_at_or_before, get_balance_for_address, get_holders_for_alkane,
    get_outpoint_balances as get_outpoint_balances_index, get_total_received_for_alkane,
    get_transfer_volume_for_alkane,
};
use crate::modules::essentials::utils::inspections::{AlkaneCreationRecord, inspection_to_json};
use crate::runtime::mdb::{Mdb, MdbBatch};
use crate::schemas::{EspoOutpoint, SchemaAlkaneId};
use alkanes_support::proto::alkanes::AlkanesTrace;
use bitcoin::consensus::encode::{deserialize, serialize};
use bitcoin::hashes::Hash;
use bitcoin::{Address, AddressType, Network, ScriptBuf, Transaction, Txid};
use bitcoincore_rpc::RpcApi;
use borsh::{BorshDeserialize, BorshSerialize};
use ordinals::{Artifact, Runestone};
use protorune_support::protostone::Protostone;
use serde_json::{Value, json, map::Map};

use crate::runtime::mempool::{
    MempoolEntry, decode_seen_key, get_mempool_mdb, get_tx_from_mempool, pending_by_txid,
    pending_for_address,
};
use crate::utils::electrum_like::AddressHistoryEntry;
use anyhow::{Result, anyhow};
use hex;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::str::FromStr;
use std::sync::{Arc, OnceLock, RwLock};

#[derive(Clone)]
pub struct MdbPointer<'a> {
    mdb: &'a Mdb,
    key: Vec<u8>,
}

impl<'a> MdbPointer<'a> {
    pub fn root(mdb: &'a Mdb) -> Self {
        Self { mdb, key: Vec::new() }
    }

    pub fn with_key(mdb: &'a Mdb, key: Vec<u8>) -> Self {
        Self { mdb, key }
    }

    pub fn key(&self) -> &[u8] {
        &self.key
    }

    pub fn keyword(&self, suffix: &str) -> Self {
        self.select(suffix.as_bytes())
    }

    pub fn select(&self, suffix: &[u8]) -> Self {
        let mut key = self.key.clone();
        key.extend_from_slice(suffix);
        Self { mdb: self.mdb, key }
    }

    pub fn get(&self) -> Result<Option<Vec<u8>>> {
        self.mdb.get(&self.key).map_err(|e| anyhow!("mdb.get failed: {e}"))
    }

    pub fn put(&self, value: &[u8]) -> Result<()> {
        self.mdb.put(&self.key, value).map_err(|e| anyhow!("mdb.put failed: {e}"))
    }

    pub fn multi_get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>> {
        let full_keys: Vec<Vec<u8>> = keys
            .iter()
            .map(|k| {
                let mut key = self.key.clone();
                key.extend_from_slice(k);
                key
            })
            .collect();
        self.mdb.multi_get(&full_keys).map_err(|e| anyhow!("mdb.multi_get failed: {e}"))
    }

    pub fn scan_prefix(&self) -> Result<Vec<Vec<u8>>> {
        self.mdb
            .scan_prefix(&self.key)
            .map_err(|e| anyhow!("mdb.scan_prefix failed: {e}"))
    }

    pub fn bulk_write<F>(&self, build: F) -> Result<()>
    where
        F: FnOnce(&mut MdbBatch<'_>),
    {
        self.mdb.bulk_write(build).map_err(|e| anyhow!("mdb.bulk_write failed: {e}"))
    }

    pub fn mdb(&self) -> &Mdb {
        self.mdb
    }
}

#[allow(non_snake_case)]
#[derive(Clone)]
pub struct EssentialsTable<'a> {
    pub ROOT: MdbPointer<'a>,
    // Core kv directory rows (0x01 = values, 0x03 = directory entries).
    pub KV_ROWS: MdbPointer<'a>,
    pub DIR_ROWS: MdbPointer<'a>,
    pub INDEX_HEIGHT: MdbPointer<'a>,
    // Balances + outpoint indexes (address/outpoint views).
    pub BALANCES: MdbPointer<'a>,
    pub OUTPOINT_BALANCES: MdbPointer<'a>,
    pub OUTPOINT_ADDR: MdbPointer<'a>,
    pub UTXO_SPK: MdbPointer<'a>,
    pub ADDR_SPK: MdbPointer<'a>,
    // Alkane holders and balances.
    pub HOLDERS: MdbPointer<'a>,
    pub HOLDERS_COUNT: MdbPointer<'a>,
    pub HOLDERS_ORDERED: MdbPointer<'a>,
    pub TRANSFER_VOLUME: MdbPointer<'a>,
    pub TOTAL_RECEIVED: MdbPointer<'a>,
    pub ADDRESS_ACTIVITY: MdbPointer<'a>,
    pub ALKANE_BALANCES: MdbPointer<'a>,
    pub ALKANE_BALANCES_BY_HEIGHT: MdbPointer<'a>,
    pub ALKANE_BALANCE_TXS: MdbPointer<'a>,
    pub ALKANE_BALANCE_TXS_PAGED: MdbPointer<'a>,
    pub ALKANE_BALANCE_TXS_META: MdbPointer<'a>,
    pub ALKANE_BALANCE_TXS_BY_TOKEN: MdbPointer<'a>,
    pub ALKANE_BALANCE_TXS_BY_TOKEN_PAGED: MdbPointer<'a>,
    pub ALKANE_BALANCE_TXS_BY_TOKEN_META: MdbPointer<'a>,
    pub ALKANE_BALANCE_TXS_BY_HEIGHT: MdbPointer<'a>,
    // Alkane creation + metadata.
    pub ALKANE_INFO: MdbPointer<'a>,
    pub ALKANE_NAME_INDEX: MdbPointer<'a>,
    pub ALKANE_SYMBOL_INDEX: MdbPointer<'a>,
    pub ORBITAL_COLLECTION_NAME: MdbPointer<'a>,
    pub ALKANE_CREATION_BY_ID: MdbPointer<'a>,
    pub ALKANE_CREATION_ORDERED: MdbPointer<'a>,
    pub ALKANE_CREATION_COUNT: MdbPointer<'a>,
    pub CIRCULATING_SUPPLY: MdbPointer<'a>,
    pub CIRCULATING_SUPPLY_LATEST: MdbPointer<'a>,
    pub TOTAL_MINTED: MdbPointer<'a>,
    pub TOTAL_MINTED_LATEST: MdbPointer<'a>,
    // Transaction summaries + reverse indexes.
    pub ALKANE_TX_SUMMARY: MdbPointer<'a>,
    pub ALKANE_BLOCK: MdbPointer<'a>,
    pub ALKANE_ADDR: MdbPointer<'a>,
    pub ALKANE_LATEST_TRACES: MdbPointer<'a>,
    // Block summaries.
    pub BLOCK_SUMMARY: MdbPointer<'a>,
}

impl<'a> EssentialsTable<'a> {
    pub fn new(mdb: &'a Mdb) -> Self {
        let root = MdbPointer::root(mdb);
        EssentialsTable {
            ROOT: root.clone(),
            KV_ROWS: root.select(&[0x01]),
            DIR_ROWS: root.select(&[0x03]),
            INDEX_HEIGHT: root.keyword("/index_height"),
            BALANCES: root.keyword("/balances/"),
            OUTPOINT_BALANCES: root.keyword("/outpoint_balances/"),
            OUTPOINT_ADDR: root.keyword("/outpoint_addr/"),
            UTXO_SPK: root.keyword("/utxo_spk/"),
            ADDR_SPK: root.keyword("/addr_spk/"),
            HOLDERS: root.keyword("/holders/"),
            HOLDERS_COUNT: root.keyword("/holders/count/"),
            HOLDERS_ORDERED: root.keyword("/alkanes/holders/ordered/"),
            TRANSFER_VOLUME: root.keyword("/alkanes/transfer_volume/"),
            TOTAL_RECEIVED: root.keyword("/alkanes/total_received/"),
            ADDRESS_ACTIVITY: root.keyword("/addresses/alkane_activity/"),
            ALKANE_BALANCES: root.keyword("/alkane_balances/"),
            ALKANE_BALANCES_BY_HEIGHT: root.keyword("/alkane_balances_by_height/"),
            ALKANE_BALANCE_TXS: root.keyword("/alkane_balance_txs/"),
            ALKANE_BALANCE_TXS_PAGED: root.keyword("/alkane_balance_txs_paged/"),
            ALKANE_BALANCE_TXS_META: root.keyword("/alkane_balance_txs_meta/"),
            ALKANE_BALANCE_TXS_BY_TOKEN: root.keyword("/alkane_balance_txs_by_token/"),
            ALKANE_BALANCE_TXS_BY_TOKEN_PAGED: root.keyword("/alkane_balance_txs_by_token_paged/"),
            ALKANE_BALANCE_TXS_BY_TOKEN_META: root.keyword("/alkane_balance_txs_by_token_meta/"),
            ALKANE_BALANCE_TXS_BY_HEIGHT: root.keyword("/alkane_balance_txs_by_height/"),
            ALKANE_INFO: root.keyword("/alkane_info/"),
            ALKANE_NAME_INDEX: root.keyword("/alkanes/name/"),
            ALKANE_SYMBOL_INDEX: root.keyword("/alkanes/symbol/"),
            ORBITAL_COLLECTION_NAME: root.keyword("/orbitals/collection/name/"),
            ALKANE_CREATION_BY_ID: root.keyword("/alkanes/creation/id/"),
            ALKANE_CREATION_ORDERED: root.keyword("/alkanes/creation/ordered/"),
            ALKANE_CREATION_COUNT: root.keyword("/alkanes/creation/count"),
            CIRCULATING_SUPPLY: root.keyword("/circulating_supply/v1/"),
            CIRCULATING_SUPPLY_LATEST: root.keyword("/circulating_supply/latest/"),
            TOTAL_MINTED: root.keyword("/total_minted/v1/"),
            TOTAL_MINTED_LATEST: root.keyword("/total_minted/latest/"),
            ALKANE_TX_SUMMARY: root.keyword("/alkane_tx_summary/"),
            ALKANE_BLOCK: root.keyword("/alkane_block/"),
            ALKANE_ADDR: root.keyword("/alkane_addr/"),
            ALKANE_LATEST_TRACES: root.keyword("/alkane_latest_traces"),
            BLOCK_SUMMARY: root.keyword("/block_summary/"),
        }
    }
}

impl<'a> EssentialsTable<'a> {
    pub fn kv_row_key(&self, alk: &SchemaAlkaneId, skey: &[u8]) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(4 + 8 + 2 + skey.len());
        suffix.extend_from_slice(&alk.block.to_be_bytes());
        suffix.extend_from_slice(&alk.tx.to_be_bytes());
        let len = u16::try_from(skey.len()).unwrap_or(u16::MAX);
        suffix.extend_from_slice(&len.to_be_bytes());
        if len as usize != skey.len() {
            suffix.extend_from_slice(&skey[..(len as usize)]);
        } else {
            suffix.extend_from_slice(skey);
        }
        self.KV_ROWS.select(&suffix).key().to_vec()
    }

    pub fn dir_row_key(&self, alk: &SchemaAlkaneId, skey: &[u8]) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(4 + 8 + 2 + skey.len());
        suffix.extend_from_slice(&alk.block.to_be_bytes());
        suffix.extend_from_slice(&alk.tx.to_be_bytes());
        let len = u16::try_from(skey.len()).unwrap_or(u16::MAX);
        suffix.extend_from_slice(&len.to_be_bytes());
        if len as usize != skey.len() {
            suffix.extend_from_slice(&skey[..(len as usize)]);
        } else {
            suffix.extend_from_slice(skey);
        }
        self.DIR_ROWS.select(&suffix).key().to_vec()
    }

    pub fn dir_scan_prefix(&self, alk: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(4 + 8);
        suffix.extend_from_slice(&alk.block.to_be_bytes());
        suffix.extend_from_slice(&alk.tx.to_be_bytes());
        self.DIR_ROWS.select(&suffix).key().to_vec()
    }

    pub fn addr_spk_key(&self, addr: &str) -> Vec<u8> {
        self.ADDR_SPK.select(addr.as_bytes()).key().to_vec()
    }

    pub fn balances_key(&self, address: &str, outp: &EspoOutpoint) -> Result<Vec<u8>> {
        let mut suffix = Vec::with_capacity(address.len() + 1 + 64);
        suffix.extend_from_slice(address.as_bytes());
        suffix.push(b'/');
        suffix.extend_from_slice(&borsh::to_vec(outp)?);
        Ok(self.BALANCES.select(&suffix).key().to_vec())
    }

    pub fn holders_key(&self, alkane: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12);
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        self.HOLDERS.select(&suffix).key().to_vec()
    }

    pub fn holders_count_key(&self, alkane: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12);
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        self.HOLDERS_COUNT.select(&suffix).key().to_vec()
    }

    pub fn transfer_volume_key(&self, alkane: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12);
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        self.TRANSFER_VOLUME.select(&suffix).key().to_vec()
    }

    pub fn total_received_key(&self, alkane: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12);
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        self.TOTAL_RECEIVED.select(&suffix).key().to_vec()
    }

    pub fn address_activity_key(&self, address: &str) -> Vec<u8> {
        self.ADDRESS_ACTIVITY.select(address.as_bytes()).key().to_vec()
    }

    pub fn alkane_balance_txs_key(&self, alkane: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12);
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        self.ALKANE_BALANCE_TXS.select(&suffix).key().to_vec()
    }

    pub fn alkane_balance_txs_page_key(&self, alkane: &SchemaAlkaneId, page: u64) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12 + 1 + 8);
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        suffix.push(b'/');
        suffix.extend_from_slice(&page.to_be_bytes());
        self.ALKANE_BALANCE_TXS_PAGED.select(&suffix).key().to_vec()
    }

    pub fn alkane_balance_txs_meta_key(&self, alkane: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12);
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        self.ALKANE_BALANCE_TXS_META.select(&suffix).key().to_vec()
    }

    pub fn alkane_balance_txs_by_height_key(&self, height: u32) -> Vec<u8> {
        self.ALKANE_BALANCE_TXS_BY_HEIGHT.select(&height.to_be_bytes()).key().to_vec()
    }

    pub fn alkane_balance_txs_by_token_key(
        &self,
        owner: &SchemaAlkaneId,
        token: &SchemaAlkaneId,
    ) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(25);
        suffix.extend_from_slice(&owner.block.to_be_bytes());
        suffix.extend_from_slice(&owner.tx.to_be_bytes());
        suffix.push(b'/');
        suffix.extend_from_slice(&token.block.to_be_bytes());
        suffix.extend_from_slice(&token.tx.to_be_bytes());
        self.ALKANE_BALANCE_TXS_BY_TOKEN.select(&suffix).key().to_vec()
    }

    pub fn alkane_balance_txs_by_token_page_key(
        &self,
        owner: &SchemaAlkaneId,
        token: &SchemaAlkaneId,
        page: u64,
    ) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(25 + 1 + 8);
        suffix.extend_from_slice(&owner.block.to_be_bytes());
        suffix.extend_from_slice(&owner.tx.to_be_bytes());
        suffix.push(b'/');
        suffix.extend_from_slice(&token.block.to_be_bytes());
        suffix.extend_from_slice(&token.tx.to_be_bytes());
        suffix.push(b'/');
        suffix.extend_from_slice(&page.to_be_bytes());
        self.ALKANE_BALANCE_TXS_BY_TOKEN_PAGED.select(&suffix).key().to_vec()
    }

    pub fn alkane_balance_txs_by_token_meta_key(
        &self,
        owner: &SchemaAlkaneId,
        token: &SchemaAlkaneId,
    ) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(25);
        suffix.extend_from_slice(&owner.block.to_be_bytes());
        suffix.extend_from_slice(&owner.tx.to_be_bytes());
        suffix.push(b'/');
        suffix.extend_from_slice(&token.block.to_be_bytes());
        suffix.extend_from_slice(&token.tx.to_be_bytes());
        self.ALKANE_BALANCE_TXS_BY_TOKEN_META.select(&suffix).key().to_vec()
    }

    pub fn alkane_balances_key(&self, owner: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12);
        suffix.extend_from_slice(&owner.block.to_be_bytes());
        suffix.extend_from_slice(&owner.tx.to_be_bytes());
        self.ALKANE_BALANCES.select(&suffix).key().to_vec()
    }

    pub fn alkane_balances_by_height_key(&self, owner: &SchemaAlkaneId, height: u32) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12 + 1 + 4);
        suffix.extend_from_slice(&owner.block.to_be_bytes());
        suffix.extend_from_slice(&owner.tx.to_be_bytes());
        suffix.push(b'/');
        suffix.extend_from_slice(&height.to_be_bytes());
        self.ALKANE_BALANCES_BY_HEIGHT.select(&suffix).key().to_vec()
    }

    pub fn alkane_balances_by_height_prefix(&self, owner: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12 + 1);
        suffix.extend_from_slice(&owner.block.to_be_bytes());
        suffix.extend_from_slice(&owner.tx.to_be_bytes());
        suffix.push(b'/');
        self.ALKANE_BALANCES_BY_HEIGHT.select(&suffix).key().to_vec()
    }

    pub fn alkane_info_key(&self, alkane: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12);
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        self.ALKANE_INFO.select(&suffix).key().to_vec()
    }

    pub fn alkane_name_index_key(&self, name: &str, alkane: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(name.len() + 1 + 12);
        suffix.extend_from_slice(name.as_bytes());
        suffix.push(b'/');
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        self.ALKANE_NAME_INDEX.select(&suffix).key().to_vec()
    }

    pub fn alkane_name_index_prefix(&self, name_prefix: &str) -> Vec<u8> {
        self.ALKANE_NAME_INDEX.select(name_prefix.as_bytes()).key().to_vec()
    }

    pub fn alkane_symbol_index_key(&self, symbol: &str, alkane: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(symbol.len() + 1 + 12);
        suffix.extend_from_slice(symbol.as_bytes());
        suffix.push(b'/');
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        self.ALKANE_SYMBOL_INDEX.select(&suffix).key().to_vec()
    }

    pub fn alkane_symbol_index_prefix(&self, symbol_prefix: &str) -> Vec<u8> {
        self.ALKANE_SYMBOL_INDEX.select(symbol_prefix.as_bytes()).key().to_vec()
    }

    pub fn orbital_collection_name_key(&self, factory: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12);
        suffix.extend_from_slice(&factory.block.to_be_bytes());
        suffix.extend_from_slice(&factory.tx.to_be_bytes());
        self.ORBITAL_COLLECTION_NAME.select(&suffix).key().to_vec()
    }

    pub fn alkane_holders_ordered_key(&self, count: u64, alkane: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(8 + 12);
        suffix.extend_from_slice(&count.to_be_bytes());
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        self.HOLDERS_ORDERED.select(&suffix).key().to_vec()
    }

    pub fn alkane_holders_ordered_prefix(&self) -> Vec<u8> {
        self.HOLDERS_ORDERED.key().to_vec()
    }

    pub fn parse_alkane_name_index_key(&self, key: &[u8]) -> Option<(String, SchemaAlkaneId)> {
        let prefix = self.ALKANE_NAME_INDEX.key();
        if !key.starts_with(prefix) {
            return None;
        }
        let rest = &key[prefix.len()..];
        let split = rest.iter().rposition(|b| *b == b'/')?;
        let name_bytes = &rest[..split];
        let id_bytes = &rest[split + 1..];
        if id_bytes.len() != 12 {
            return None;
        }
        let mut block_arr = [0u8; 4];
        block_arr.copy_from_slice(&id_bytes[..4]);
        let mut tx_arr = [0u8; 8];
        tx_arr.copy_from_slice(&id_bytes[4..12]);
        let name = String::from_utf8(name_bytes.to_vec()).ok()?;
        Some((
            name,
            SchemaAlkaneId { block: u32::from_be_bytes(block_arr), tx: u64::from_be_bytes(tx_arr) },
        ))
    }

    pub fn parse_alkane_symbol_index_key(&self, key: &[u8]) -> Option<(String, SchemaAlkaneId)> {
        let prefix = self.ALKANE_SYMBOL_INDEX.key();
        if !key.starts_with(prefix) {
            return None;
        }
        let rest = &key[prefix.len()..];
        let split = rest.iter().rposition(|b| *b == b'/')?;
        let symbol_bytes = &rest[..split];
        let id_bytes = &rest[split + 1..];
        if id_bytes.len() != 12 {
            return None;
        }
        let mut block_arr = [0u8; 4];
        block_arr.copy_from_slice(&id_bytes[..4]);
        let mut tx_arr = [0u8; 8];
        tx_arr.copy_from_slice(&id_bytes[4..12]);
        let symbol = String::from_utf8(symbol_bytes.to_vec()).ok()?;
        Some((
            symbol,
            SchemaAlkaneId { block: u32::from_be_bytes(block_arr), tx: u64::from_be_bytes(tx_arr) },
        ))
    }

    pub fn parse_alkane_holders_ordered_key(&self, key: &[u8]) -> Option<(u64, SchemaAlkaneId)> {
        let prefix = self.HOLDERS_ORDERED.key();
        if !key.starts_with(prefix) {
            return None;
        }
        let rest = &key[prefix.len()..];
        if rest.len() != 20 {
            return None;
        }
        let mut count_arr = [0u8; 8];
        count_arr.copy_from_slice(&rest[..8]);
        let mut block_arr = [0u8; 4];
        block_arr.copy_from_slice(&rest[8..12]);
        let mut tx_arr = [0u8; 8];
        tx_arr.copy_from_slice(&rest[12..20]);
        Some((
            u64::from_be_bytes(count_arr),
            SchemaAlkaneId { block: u32::from_be_bytes(block_arr), tx: u64::from_be_bytes(tx_arr) },
        ))
    }

    pub fn alkane_creation_by_id_key(&self, alkane: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12);
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        self.ALKANE_CREATION_BY_ID.select(&suffix).key().to_vec()
    }

    pub fn alkane_creation_ordered_key(
        &self,
        timestamp: u32,
        height: u32,
        tx_index: u32,
        alkane: &SchemaAlkaneId,
    ) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(4 + 4 + 4 + 12);
        suffix.extend_from_slice(&timestamp.to_be_bytes());
        suffix.extend_from_slice(&height.to_be_bytes());
        suffix.extend_from_slice(&tx_index.to_be_bytes());
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        self.ALKANE_CREATION_ORDERED.select(&suffix).key().to_vec()
    }

    pub fn alkane_creation_ordered_prefix(&self) -> Vec<u8> {
        self.ALKANE_CREATION_ORDERED.key().to_vec()
    }

    pub fn circulating_supply_key(&self, alkane: &SchemaAlkaneId, height: u32) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12 + 4);
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        suffix.extend_from_slice(&height.to_be_bytes());
        self.CIRCULATING_SUPPLY.select(&suffix).key().to_vec()
    }

    pub fn circulating_supply_prefix(&self, alkane: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12);
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        self.CIRCULATING_SUPPLY.select(&suffix).key().to_vec()
    }

    pub fn circulating_supply_latest_key(&self, alkane: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12);
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        self.CIRCULATING_SUPPLY_LATEST.select(&suffix).key().to_vec()
    }

    pub fn total_minted_key(&self, alkane: &SchemaAlkaneId, height: u32) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12 + 4);
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        suffix.extend_from_slice(&height.to_be_bytes());
        self.TOTAL_MINTED.select(&suffix).key().to_vec()
    }

    pub fn total_minted_prefix(&self, alkane: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12);
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        self.TOTAL_MINTED.select(&suffix).key().to_vec()
    }

    pub fn total_minted_latest_key(&self, alkane: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12);
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        self.TOTAL_MINTED_LATEST.select(&suffix).key().to_vec()
    }

    pub fn alkane_creation_count_key(&self) -> Vec<u8> {
        self.ALKANE_CREATION_COUNT.key().to_vec()
    }

    pub fn alkane_tx_summary_key(&self, txid: &[u8; 32]) -> Vec<u8> {
        self.ALKANE_TX_SUMMARY.select(txid).key().to_vec()
    }

    pub fn alkane_block_txid_key(&self, height: u64, idx: u64) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(8 + 1 + 8);
        suffix.extend_from_slice(&height.to_be_bytes());
        suffix.push(b'/');
        suffix.extend_from_slice(&idx.to_be_bytes());
        self.ALKANE_BLOCK.select(&suffix).key().to_vec()
    }

    pub fn alkane_block_len_key(&self, height: u64) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(8 + 7);
        suffix.extend_from_slice(&height.to_be_bytes());
        suffix.extend_from_slice(b"/length");
        self.ALKANE_BLOCK.select(&suffix).key().to_vec()
    }

    pub fn alkane_address_txid_key(&self, addr: &str, idx: u64) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(addr.len() + 1 + 8);
        suffix.extend_from_slice(addr.as_bytes());
        suffix.push(b'/');
        suffix.extend_from_slice(&idx.to_be_bytes());
        self.ALKANE_ADDR.select(&suffix).key().to_vec()
    }

    pub fn alkane_address_len_key(&self, addr: &str) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(addr.len() + 7);
        suffix.extend_from_slice(addr.as_bytes());
        suffix.extend_from_slice(b"/length");
        self.ALKANE_ADDR.select(&suffix).key().to_vec()
    }

    pub fn alkane_latest_traces_key(&self) -> Vec<u8> {
        self.ALKANE_LATEST_TRACES.key().to_vec()
    }

    pub fn outpoint_addr_key(&self, outp: &EspoOutpoint) -> Result<Vec<u8>> {
        let suffix = borsh::to_vec(outp)?;
        Ok(self.OUTPOINT_ADDR.select(&suffix).key().to_vec())
    }

    pub fn utxo_spk_key(&self, outp: &EspoOutpoint) -> Result<Vec<u8>> {
        let suffix = borsh::to_vec(outp)?;
        Ok(self.UTXO_SPK.select(&suffix).key().to_vec())
    }

    pub fn outpoint_balances_key(&self, outp: &EspoOutpoint) -> Result<Vec<u8>> {
        let suffix = borsh::to_vec(outp)?;
        Ok(self.OUTPOINT_BALANCES.select(&suffix).key().to_vec())
    }

    pub fn block_summary_key(&self, height: u32) -> Vec<u8> {
        self.BLOCK_SUMMARY.select(&height.to_be_bytes()).key().to_vec()
    }

    pub fn block_summary_prefix(&self) -> Vec<u8> {
        self.BLOCK_SUMMARY.key().to_vec()
    }

    pub fn outpoint_balances_prefix(&self, txid: &[u8], vout: u32) -> Result<Vec<u8>> {
        let suffix = borsh::to_vec(&OutpointPrefix { txid: txid.to_vec(), vout })?;
        Ok(self.OUTPOINT_BALANCES.select(&suffix).key().to_vec())
    }
}

#[derive(Clone)]
pub struct EssentialsProvider {
    mdb: Arc<Mdb>,
}

impl EssentialsProvider {
    pub fn new(mdb: Arc<Mdb>) -> Self {
        Self { mdb }
    }

    pub fn table(&self) -> EssentialsTable<'_> {
        EssentialsTable::new(self.mdb.as_ref())
    }

    pub fn mdb(&self) -> &Mdb {
        self.mdb.as_ref()
    }

    pub fn get_raw_value(&self, params: GetRawValueParams) -> Result<GetRawValueResult> {
        let value = self.mdb.get(&params.key).map_err(|e| anyhow!("mdb.get failed: {e}"))?;
        Ok(GetRawValueResult { value })
    }

    pub fn get_multi_values(&self, params: GetMultiValuesParams) -> Result<GetMultiValuesResult> {
        let values = self
            .mdb
            .multi_get(&params.keys)
            .map_err(|e| anyhow!("mdb.multi_get failed: {e}"))?;
        Ok(GetMultiValuesResult { values })
    }

    pub fn get_scan_prefix(&self, params: GetScanPrefixParams) -> Result<GetScanPrefixResult> {
        let keys = self
            .mdb
            .scan_prefix(&params.prefix)
            .map_err(|e| anyhow!("mdb.scan_prefix failed: {e}"))?;
        Ok(GetScanPrefixResult { keys })
    }

    pub fn get_iter_prefix_rev(
        &self,
        params: GetIterPrefixRevParams,
    ) -> Result<GetIterPrefixRevResult> {
        let full_prefix = self.mdb.prefixed(&params.prefix);
        let mut entries = Vec::new();
        for res in self.mdb.iter_prefix_rev(&full_prefix) {
            let (k_full, v) = res.map_err(|e| anyhow!("mdb.iter_prefix_rev failed: {e}"))?;
            let rel = &k_full[self.mdb.prefix().len()..];
            entries.push((rel.to_vec(), v));
        }
        Ok(GetIterPrefixRevResult { entries })
    }

    pub fn get_iter_from(&self, params: GetIterFromParams) -> Result<GetIterFromResult> {
        let mut entries = Vec::new();
        for res in self.mdb.iter_from(&params.start) {
            let (k_full, v) = res.map_err(|e| anyhow!("mdb.iter_from failed: {e}"))?;
            let rel = &k_full[self.mdb.prefix().len()..];
            entries.push((rel.to_vec(), v));
        }
        Ok(GetIterFromResult { entries })
    }

    pub fn set_raw_value(&self, params: SetRawValueParams) -> Result<()> {
        self.mdb
            .put(&params.key, &params.value)
            .map_err(|e| anyhow!("mdb.put failed: {e}"))
    }

    pub fn set_batch(&self, params: SetBatchParams) -> Result<()> {
        self.mdb
            .bulk_write(|wb: &mut MdbBatch<'_>| {
                for key in &params.deletes {
                    wb.delete(key);
                }
                for (key, value) in &params.puts {
                    wb.put(key, value);
                }
            })
            .map_err(|e| anyhow!("mdb.bulk_write failed: {e}"))
    }

    pub fn get_index_height(&self, _params: GetIndexHeightParams) -> Result<GetIndexHeightResult> {
        crate::debug_timer_log!("get_index_height");
        let table = self.table();
        let Some(bytes) = table.INDEX_HEIGHT.get()? else {
            return Ok(GetIndexHeightResult { height: None });
        };
        if bytes.len() != 4 {
            return Err(anyhow!("[ESSENTIALS] invalid /index_height length {}", bytes.len()));
        }
        let mut arr = [0u8; 4];
        arr.copy_from_slice(&bytes);
        Ok(GetIndexHeightResult { height: Some(u32::from_le_bytes(arr)) })
    }

    pub fn set_index_height(&self, params: SetIndexHeightParams) -> Result<()> {
        crate::debug_timer_log!("set_index_height");
        let table = self.table();
        table
            .INDEX_HEIGHT
            .put(&params.height.to_le_bytes())
            .map_err(|e| anyhow!("[ESSENTIALS] rocksdb put(/index_height) failed: {e}"))
    }

    pub fn get_creation_record(
        &self,
        params: GetCreationRecordParams,
    ) -> Result<GetCreationRecordResult> {
        crate::debug_timer_log!("get_creation_record");
        let table = self.table();
        let key = table.alkane_creation_by_id_key(&params.alkane);
        let Some(bytes) = self.mdb.get(&key).map_err(|e| anyhow!("mdb.get failed: {e}"))? else {
            return Ok(GetCreationRecordResult { record: None });
        };
        let record = decode_creation_record(&bytes)?;
        Ok(GetCreationRecordResult { record: Some(record) })
    }

    pub fn get_creation_records_by_id(
        &self,
        params: GetCreationRecordsByIdParams,
    ) -> Result<GetCreationRecordsByIdResult> {
        crate::debug_timer_log!("get_creation_records_by_id");
        let table = self.table();
        let keys: Vec<Vec<u8>> =
            params.alkanes.iter().map(|alk| table.alkane_creation_by_id_key(alk)).collect();
        let values = self.mdb.multi_get(&keys).map_err(|e| anyhow!("mdb.multi_get failed: {e}"))?;
        let mut records = Vec::with_capacity(values.len());
        for val in values {
            if let Some(bytes) = val {
                records.push(Some(decode_creation_record(&bytes)?));
            } else {
                records.push(None);
            }
        }
        Ok(GetCreationRecordsByIdResult { records })
    }

    pub fn get_creation_records_ordered(
        &self,
        _params: GetCreationRecordsOrderedParams,
    ) -> Result<GetCreationRecordsOrderedResult> {
        crate::debug_timer_log!("get_creation_records_ordered");
        let table = self.table();
        let entries = match self.get_iter_prefix_rev(GetIterPrefixRevParams {
            prefix: table.alkane_creation_ordered_prefix(),
        }) {
            Ok(v) => v.entries,
            Err(_) => Vec::new(),
        };
        let mut records = Vec::with_capacity(entries.len());
        for (_k, v) in entries {
            if let Ok(rec) = decode_creation_record(&v) {
                records.push(rec);
            }
        }
        Ok(GetCreationRecordsOrderedResult { records })
    }

    pub fn get_creation_records_ordered_page(
        &self,
        params: GetCreationRecordsOrderedPageParams,
    ) -> Result<GetCreationRecordsOrderedPageResult> {
        crate::debug_timer_log!("get_creation_records_ordered_page");
        let table = self.table();
        let prefix = table.alkane_creation_ordered_prefix();
        let mut records = Vec::new();
        let mut skipped: u64 = 0;

        if params.desc {
            let full_prefix = self.mdb.prefixed(&prefix);
            for res in self.mdb.iter_prefix_rev(&full_prefix) {
                let (_k_full, v) = res.map_err(|e| anyhow!("mdb.iter_prefix_rev failed: {e}"))?;
                if skipped < params.offset {
                    skipped += 1;
                    continue;
                }
                if let Ok(rec) = decode_creation_record(&v) {
                    records.push(rec);
                    if records.len() >= params.limit as usize {
                        break;
                    }
                }
            }
        } else {
            let full_prefix = self.mdb.prefixed(&prefix);
            for res in self.mdb.iter_from(&prefix) {
                let (k_full, v) = res.map_err(|e| anyhow!("mdb.iter_from failed: {e}"))?;
                if !k_full.starts_with(&full_prefix) {
                    break;
                }
                if skipped < params.offset {
                    skipped += 1;
                    continue;
                }
                if let Ok(rec) = decode_creation_record(&v) {
                    records.push(rec);
                    if records.len() >= params.limit as usize {
                        break;
                    }
                }
            }
        }

        Ok(GetCreationRecordsOrderedPageResult { records })
    }

    pub fn get_alkane_ids_by_name_prefix(
        &self,
        params: GetAlkaneIdsByNamePrefixParams,
    ) -> Result<GetAlkaneIdsByNamePrefixResult> {
        crate::debug_timer_log!("get_alkane_ids_by_name_prefix");
        let table = self.table();
        let keys = match self.get_scan_prefix(GetScanPrefixParams {
            prefix: table.alkane_name_index_prefix(&params.prefix),
        }) {
            Ok(v) => v.keys,
            Err(_) => Vec::new(),
        };
        let mut ids = Vec::new();
        let mut seen = HashSet::new();
        for key in keys {
            if let Some((_name, id)) = table.parse_alkane_name_index_key(&key) {
                if seen.insert(id) {
                    ids.push(id);
                }
            }
        }
        Ok(GetAlkaneIdsByNamePrefixResult { ids })
    }

    pub fn get_alkane_ids_by_name_prefix_page(
        &self,
        params: GetAlkaneIdsByNamePrefixPageParams,
    ) -> Result<GetAlkaneIdsByNamePrefixResult> {
        crate::debug_timer_log!("get_alkane_ids_by_name_prefix_page");
        let table = self.table();
        let prefix = table.alkane_name_index_prefix(&params.prefix);
        let full_prefix = self.mdb.prefixed(&prefix);
        let mut ids = Vec::new();
        let mut seen = HashSet::new();
        let mut unique_skipped: u64 = 0;

        for res in self.mdb.iter_from(&prefix) {
            let (k_full, _v) = res.map_err(|e| anyhow!("mdb.iter_from failed: {e}"))?;
            if !k_full.starts_with(&full_prefix) {
                break;
            }
            let rel = &k_full[self.mdb.prefix().len()..];
            if let Some((_name, id)) = table.parse_alkane_name_index_key(rel) {
                if seen.insert(id) {
                    if unique_skipped < params.offset {
                        unique_skipped += 1;
                        continue;
                    }
                    ids.push(id);
                    if ids.len() >= params.limit as usize {
                        break;
                    }
                }
            }
        }

        Ok(GetAlkaneIdsByNamePrefixResult { ids })
    }

    pub fn get_alkane_ids_by_symbol_prefix(
        &self,
        params: GetAlkaneIdsBySymbolPrefixParams,
    ) -> Result<GetAlkaneIdsBySymbolPrefixResult> {
        crate::debug_timer_log!("get_alkane_ids_by_symbol_prefix");
        let table = self.table();
        let keys = match self.get_scan_prefix(GetScanPrefixParams {
            prefix: table.alkane_symbol_index_prefix(&params.prefix),
        }) {
            Ok(v) => v.keys,
            Err(_) => Vec::new(),
        };
        let mut ids = Vec::new();
        let mut seen = HashSet::new();
        for key in keys {
            if let Some((_sym, id)) = table.parse_alkane_symbol_index_key(&key) {
                if seen.insert(id) {
                    ids.push(id);
                }
            }
        }
        Ok(GetAlkaneIdsBySymbolPrefixResult { ids })
    }

    pub fn get_alkane_ids_by_symbol_prefix_page(
        &self,
        params: GetAlkaneIdsBySymbolPrefixPageParams,
    ) -> Result<GetAlkaneIdsBySymbolPrefixResult> {
        crate::debug_timer_log!("get_alkane_ids_by_symbol_prefix_page");
        let table = self.table();
        let prefix = table.alkane_symbol_index_prefix(&params.prefix);
        let full_prefix = self.mdb.prefixed(&prefix);
        let mut ids = Vec::new();
        let mut seen = HashSet::new();
        let mut unique_skipped: u64 = 0;

        for res in self.mdb.iter_from(&prefix) {
            let (k_full, _v) = res.map_err(|e| anyhow!("mdb.iter_from failed: {e}"))?;
            if !k_full.starts_with(&full_prefix) {
                break;
            }
            let rel = &k_full[self.mdb.prefix().len()..];
            if let Some((_symbol, id)) = table.parse_alkane_symbol_index_key(rel) {
                if seen.insert(id) {
                    if unique_skipped < params.offset {
                        unique_skipped += 1;
                        continue;
                    }
                    ids.push(id);
                    if ids.len() >= params.limit as usize {
                        break;
                    }
                }
            }
        }

        Ok(GetAlkaneIdsBySymbolPrefixResult { ids })
    }

    pub fn get_creation_count(
        &self,
        _params: GetCreationCountParams,
    ) -> Result<GetCreationCountResult> {
        crate::debug_timer_log!("get_creation_count");
        let table = self.table();
        let count = table
            .ALKANE_CREATION_COUNT
            .get()?
            .and_then(|b| {
                if b.len() == 8 {
                    let mut arr = [0u8; 8];
                    arr.copy_from_slice(&b);
                    Some(u64::from_le_bytes(arr))
                } else {
                    None
                }
            })
            .unwrap_or(0);
        Ok(GetCreationCountResult { count })
    }

    pub fn get_holders_count(
        &self,
        params: GetHoldersCountParams,
    ) -> Result<GetHoldersCountResult> {
        crate::debug_timer_log!("get_holders_count");
        let table = self.table();
        let count = self
            .get_raw_value(GetRawValueParams { key: table.holders_count_key(&params.alkane) })
            .ok()
            .and_then(|resp| resp.value)
            .and_then(|raw| HoldersCountEntry::try_from_slice(&raw).ok())
            .map(|entry| entry.count)
            .unwrap_or(0);
        Ok(GetHoldersCountResult { count })
    }

    pub fn get_holders_counts_by_id(
        &self,
        params: GetHoldersCountsByIdParams,
    ) -> Result<GetHoldersCountsByIdResult> {
        crate::debug_timer_log!("get_holders_counts_by_id");
        let table = self.table();
        let keys: Vec<Vec<u8>> =
            params.alkanes.iter().map(|alk| table.holders_count_key(alk)).collect();
        let values = self.mdb.multi_get(&keys).map_err(|e| anyhow!("mdb.multi_get failed: {e}"))?;
        let mut counts = Vec::with_capacity(values.len());
        for val in values {
            let count = val
                .and_then(|raw| HoldersCountEntry::try_from_slice(&raw).ok())
                .map(|entry| entry.count)
                .unwrap_or(0);
            counts.push(count);
        }
        Ok(GetHoldersCountsByIdResult { counts })
    }

    pub fn get_holders_ordered_page(
        &self,
        params: GetHoldersOrderedPageParams,
    ) -> Result<GetHoldersOrderedPageResult> {
        crate::debug_timer_log!("get_holders_ordered_page");
        let table = self.table();
        let prefix = table.alkane_holders_ordered_prefix();
        let mut ids = Vec::new();
        let mut skipped: u64 = 0;

        if params.desc {
            let full_prefix = self.mdb.prefixed(&prefix);
            for res in self.mdb.iter_prefix_rev(&full_prefix) {
                let (k_full, _v) = res.map_err(|e| anyhow!("mdb.iter_prefix_rev failed: {e}"))?;
                let rel = &k_full[self.mdb.prefix().len()..];
                let Some((_count, id)) = table.parse_alkane_holders_ordered_key(rel) else {
                    continue;
                };
                if skipped < params.offset {
                    skipped += 1;
                    continue;
                }
                ids.push(id);
                if ids.len() >= params.limit as usize {
                    break;
                }
            }
        } else {
            let full_prefix = self.mdb.prefixed(&prefix);
            for res in self.mdb.iter_from(&prefix) {
                let (k_full, _v) = res.map_err(|e| anyhow!("mdb.iter_from failed: {e}"))?;
                if !k_full.starts_with(&full_prefix) {
                    break;
                }
                let rel = &k_full[self.mdb.prefix().len()..];
                let Some((_count, id)) = table.parse_alkane_holders_ordered_key(rel) else {
                    continue;
                };
                if skipped < params.offset {
                    skipped += 1;
                    continue;
                }
                ids.push(id);
                if ids.len() >= params.limit as usize {
                    break;
                }
            }
        }

        Ok(GetHoldersOrderedPageResult { ids })
    }

    pub fn get_latest_circulating_supply(
        &self,
        params: GetLatestCirculatingSupplyParams,
    ) -> Result<GetLatestCirculatingSupplyResult> {
        crate::debug_timer_log!("get_latest_circulating_supply");
        let table = self.table();
        let supply = self
            .get_raw_value(GetRawValueParams {
                key: table.circulating_supply_latest_key(&params.alkane),
            })
            .ok()
            .and_then(|resp| resp.value)
            .and_then(|raw| decode_u128_value(&raw).ok())
            .unwrap_or(0);
        Ok(GetLatestCirculatingSupplyResult { supply })
    }

    pub fn get_latest_total_minted(
        &self,
        params: GetLatestTotalMintedParams,
    ) -> Result<GetLatestTotalMintedResult> {
        crate::debug_timer_log!("get_latest_total_minted");
        let table = self.table();
        let total_minted = self
            .get_raw_value(GetRawValueParams { key: table.total_minted_latest_key(&params.alkane) })
            .ok()
            .and_then(|resp| resp.value)
            .and_then(|raw| decode_u128_value(&raw).ok())
            .unwrap_or(0);
        Ok(GetLatestTotalMintedResult { total_minted })
    }

    pub fn get_circulating_supply(
        &self,
        params: GetCirculatingSupplyParams,
    ) -> Result<GetCirculatingSupplyResult> {
        crate::debug_timer_log!("get_circulating_supply");
        let table = self.table();
        let supply = self
            .get_raw_value(GetRawValueParams {
                key: table.circulating_supply_key(&params.alkane, params.height),
            })
            .ok()
            .and_then(|resp| resp.value)
            .and_then(|raw| decode_u128_value(&raw).ok())
            .unwrap_or(0);
        Ok(GetCirculatingSupplyResult { supply })
    }

    pub fn get_alkane_storage_value(
        &self,
        params: GetAlkaneStorageValueParams,
    ) -> Result<GetAlkaneStorageValueResult> {
        crate::debug_timer_log!("get_alkane_storage_value");
        let table = self.table();
        let key = table.kv_row_key(&params.alkane, &params.key);
        let value = self
            .get_raw_value(GetRawValueParams { key })?
            .value
            .map(|raw| split_txid_value(&raw).1.to_vec());
        Ok(GetAlkaneStorageValueResult { value })
    }

    pub fn get_block_summary(
        &self,
        params: GetBlockSummaryParams,
    ) -> Result<GetBlockSummaryResult> {
        crate::debug_timer_log!("get_block_summary");
        let table = self.table();
        let key = table.block_summary_key(params.height);
        let summary = self
            .mdb
            .get(&key)
            .map_err(|e| anyhow!("mdb.get failed: {e}"))?
            .and_then(|b| BlockSummary::try_from_slice(&b).ok());
        Ok(GetBlockSummaryResult { summary })
    }

    pub fn get_mempool_seen_page(
        &self,
        params: GetMempoolSeenPageParams,
    ) -> Result<GetMempoolSeenPageResult> {
        crate::debug_timer_log!("get_mempool_seen_page");
        let mdb = get_mempool_mdb();
        let pref = mdb.prefixed(b"seen/");
        let it = mdb.iter_prefix_rev(&pref);
        let offset = params.limit.saturating_mul(params.page.saturating_sub(1));

        let mut idx: usize = 0;
        let mut txids: Vec<Txid> = Vec::new();
        let mut has_more = false;

        for res in it {
            let Ok((k_full, _)) = res else { continue };
            let rel = &k_full[mdb.prefix().len()..];
            if !rel.starts_with(b"seen/") {
                break;
            }
            if idx < offset {
                idx += 1;
                continue;
            }
            if txids.len() >= params.limit {
                has_more = true;
                break;
            }
            if let Some((_, txid)) = decode_seen_key(rel) {
                txids.push(txid);
            }
            idx += 1;
        }

        Ok(GetMempoolSeenPageResult { txids, has_more })
    }

    pub fn get_mempool_entry(
        &self,
        params: GetMempoolEntryParams,
    ) -> Result<GetMempoolEntryResult> {
        crate::debug_timer_log!("get_mempool_entry");
        Ok(GetMempoolEntryResult { entry: get_tx_from_mempool(&params.txid) })
    }

    pub fn get_mempool_pending_for_address(
        &self,
        params: GetMempoolPendingForAddressParams,
    ) -> Result<GetMempoolPendingForAddressResult> {
        crate::debug_timer_log!("get_mempool_pending_for_address");
        Ok(GetMempoolPendingForAddressResult { entries: pending_for_address(&params.address) })
    }

    pub fn rpc_get_mempool_traces(
        &self,
        params: RpcGetMempoolTracesParams,
    ) -> Result<RpcGetMempoolTracesResult> {
        let page = params.page.unwrap_or(1).max(1) as usize;
        let limit = params.limit.unwrap_or(100).max(1) as usize;
        let address = params.address.as_deref().and_then(normalize_address);

        let mut items: Vec<Value> = Vec::new();
        let mut total_traces: usize = 0;

        let has_more = if let Some(addr) = address {
            let pending = self
                .get_mempool_pending_for_address(GetMempoolPendingForAddressParams {
                    address: addr.clone(),
                })
                .map(|resp| resp.entries)
                .unwrap_or_default();
            let pending_len = pending.len();
            let offset = limit.saturating_mul(page.saturating_sub(1));
            for (idx, entry) in pending.into_iter().enumerate() {
                if idx < offset {
                    continue;
                }
                if entry.traces.as_ref().map_or(true, |t| t.is_empty()) {
                    continue;
                }
                if items.len() >= limit {
                    break;
                }
                if let Some(t) = entry.traces.as_ref() {
                    total_traces += t.len();
                }
                items.push(mem_entry_to_json(&entry));
            }
            pending_len > offset + items.len()
        } else {
            let seen_page = self
                .get_mempool_seen_page(GetMempoolSeenPageParams { page, limit })
                .unwrap_or(GetMempoolSeenPageResult { txids: Vec::new(), has_more: false });
            for txid in seen_page.txids {
                let entry = self
                    .get_mempool_entry(GetMempoolEntryParams { txid })
                    .ok()
                    .and_then(|resp| resp.entry);
                let Some(entry) = entry else { continue };
                if entry.traces.as_ref().map_or(true, |t| t.is_empty()) {
                    continue;
                }
                if let Some(t) = entry.traces.as_ref() {
                    total_traces += t.len();
                }
                items.push(mem_entry_to_json(&entry));
            }
            seen_page.has_more
        };

        Ok(RpcGetMempoolTracesResult {
            value: json!({
                "ok": true,
                "page": page,
                "limit": limit,
                "has_more": has_more,
                "total": total_traces,
                "items": items,
            }),
        })
    }

    pub fn rpc_get_keys(&self, params: RpcGetKeysParams) -> Result<RpcGetKeysResult> {
        let Some(alk_raw) = params.alkane.as_deref() else {
            return Ok(RpcGetKeysResult {
                value: json!({
                    "ok": false,
                    "error": "missing_or_invalid_alkane",
                    "hint": "alkane should be a string like \"2:68441\" or \"0x2:0x10b59\""
                }),
            });
        };
        let Some(alk) = parse_alkane_from_str(alk_raw) else {
            return Ok(RpcGetKeysResult {
                value: json!({
                    "ok": false,
                    "error": "missing_or_invalid_alkane",
                    "hint": "alkane should be a string like \"2:68441\" or \"0x2:0x10b59\""
                }),
            });
        };

        let try_decode_utf8 = params.try_decode_utf8.unwrap_or(true);
        let limit = params.limit.unwrap_or(100).max(1) as usize;
        let page = params.page.unwrap_or(1).max(1) as usize;

        let table = self.table();
        let all_keys: Vec<Vec<u8>> = if let Some(arr) = params.keys {
            let mut v = Vec::with_capacity(arr.len());
            for it in arr {
                if let Some(bytes) = parse_key_str_to_bytes(&it) {
                    v.push(bytes);
                }
            }
            dedup_sort_keys(v)
        } else {
            let scan_pref = table.dir_scan_prefix(&alk);
            let rel_keys = match self.get_scan_prefix(GetScanPrefixParams { prefix: scan_pref }) {
                Ok(v) => v.keys,
                Err(_) => Vec::new(),
            };

            let mut extracted: Vec<Vec<u8>> = Vec::with_capacity(rel_keys.len());
            for rel in rel_keys {
                if rel.len() < 1 + 4 + 8 + 2 || rel[0] != 0x03 {
                    continue;
                }
                let key_len = u16::from_be_bytes([rel[13], rel[14]]) as usize;
                if rel.len() < 1 + 4 + 8 + 2 + key_len {
                    continue;
                }
                extracted.push(rel[15..15 + key_len].to_vec());
            }
            dedup_sort_keys(extracted)
        };

        let total = all_keys.len();
        let offset = limit.saturating_mul(page.saturating_sub(1));
        let end = (offset + limit).min(total);
        let window = if offset >= total { &[][..] } else { &all_keys[offset..end] };
        let has_more = end < total;

        let mut items: Map<String, Value> = Map::with_capacity(window.len());
        for k in window.iter() {
            let kv_key = table.kv_row_key(&alk, k);
            let (last_txid_val, value_hex, value_str_val, value_u128_val) =
                match self.get_raw_value(GetRawValueParams { key: kv_key }) {
                    Ok(resp) => {
                        if let Some(v) = resp.value {
                            let (ltxid_opt, raw) = split_txid_value(&v);
                            (
                                ltxid_opt.map(Value::String).unwrap_or(Value::Null),
                                fmt_bytes_hex(raw),
                                utf8_or_null(raw),
                                u128_le_or_null(raw),
                            )
                        } else {
                            (Value::Null, "0x".to_string(), Value::Null, Value::Null)
                        }
                    }
                    Err(_) => (Value::Null, "0x".to_string(), Value::Null, Value::Null),
                };

            let key_hex = fmt_bytes_hex(k);
            let key_str_val = utf8_or_null(k);

            let top_key = if try_decode_utf8 {
                if let Value::String(s) = &key_str_val { s.clone() } else { key_hex.clone() }
            } else {
                key_hex.clone()
            };

            items.insert(
                top_key,
                json!({
                    "key_hex":    key_hex,
                    "key_str":    key_str_val,
                    "value_hex":  value_hex,
                    "value_str":  value_str_val,
                    "value_u128": value_u128_val,
                    "last_txid":  last_txid_val
                }),
            );
        }

        Ok(RpcGetKeysResult {
            value: json!({
                "ok": true,
                "alkane": format!("{}:{}", alk.block, alk.tx),
                "page": page,
                "limit": limit,
                "total": total,
                "has_more": has_more,
                "items": Value::Object(items)
            }),
        })
    }

    pub fn rpc_get_all_alkanes(
        &self,
        params: RpcGetAllAlkanesParams,
    ) -> Result<RpcGetAllAlkanesResult> {
        let page = params.page.unwrap_or(1).max(1) as usize;
        let limit = params.limit.unwrap_or(100).max(1) as usize;
        let offset = limit.saturating_mul(page.saturating_sub(1));

        let table = self.table();
        let total = self.get_creation_count(GetCreationCountParams).map(|r| r.count).unwrap_or(0);

        let mut items: Vec<Value> = Vec::new();
        let mut seen: usize = 0;
        let entries = match self.get_iter_prefix_rev(GetIterPrefixRevParams {
            prefix: table.alkane_creation_ordered_prefix(),
        }) {
            Ok(v) => v.entries,
            Err(_) => Vec::new(),
        };
        for (_k_rel, v) in entries {
            if seen < offset {
                seen += 1;
                continue;
            }
            if items.len() >= limit {
                break;
            }
            match decode_creation_record(&v) {
                Ok(rec) => {
                    let holder_count = self
                        .get_raw_value(GetRawValueParams {
                            key: table.holders_count_key(&rec.alkane),
                        })
                        .ok()
                        .and_then(|resp| resp.value)
                        .and_then(|b| HoldersCountEntry::try_from_slice(&b).ok())
                        .map(|hc| hc.count)
                        .unwrap_or(0);
                    let inspection_json = rec.inspection.as_ref().map(inspection_to_json);
                    let name = rec.names.first().cloned();
                    let symbol = rec.symbols.first().cloned();
                    items.push(json!({
                        "alkane": format!("{}:{}", rec.alkane.block, rec.alkane.tx),
                        "creation_txid": hex::encode(rec.txid),
                        "creation_height": rec.creation_height,
                        "creation_timestamp": rec.creation_timestamp,
                        "tx_index_in_block": rec.tx_index_in_block,
                        "name": name,
                        "symbol": symbol,
                        "names": rec.names,
                        "symbols": rec.symbols,
                        "holder_count": holder_count,
                        "inspection": inspection_json,
                    }));
                }
                Err(_) => {}
            }
            seen += 1;
        }

        Ok(RpcGetAllAlkanesResult {
            value: json!({
                "ok": true,
                "page": page,
                "limit": limit,
                "total": total,
                "items": items,
            }),
        })
    }

    pub fn rpc_get_alkane_info(
        &self,
        params: RpcGetAlkaneInfoParams,
    ) -> Result<RpcGetAlkaneInfoResult> {
        let Some(alk_raw) = params.alkane.as_deref() else {
            return Ok(RpcGetAlkaneInfoResult {
                value: json!({
                    "ok": false,
                    "error": "missing_or_invalid_alkane",
                    "hint": "provide alkane as \"<block>:<tx>\" (hex ok)"
                }),
            });
        };
        let Some(alk) = parse_alkane_from_str(alk_raw) else {
            return Ok(RpcGetAlkaneInfoResult {
                value: json!({
                    "ok": false,
                    "error": "missing_or_invalid_alkane",
                    "hint": "provide alkane as \"<block>:<tx>\" (hex ok)"
                }),
            });
        };

        let record = match self.get_creation_record(GetCreationRecordParams { alkane: alk }) {
            Ok(resp) => match resp.record {
                Some(r) => r,
                None => {
                    return Ok(RpcGetAlkaneInfoResult {
                        value: json!({"ok": false, "error": "not_found"}),
                    });
                }
            },
            Err(_) => {
                return Ok(RpcGetAlkaneInfoResult {
                    value: json!({"ok": false, "error": "lookup_failed"}),
                });
            }
        };

        let table = self.table();
        let holder_count = get_holders_for_alkane(self, alk, 1, 1)
            .map(|(total, _, _)| total as u64)
            .unwrap_or_else(|_| {
                self.get_raw_value(GetRawValueParams { key: table.holders_count_key(&alk) })
                    .ok()
                    .and_then(|resp| resp.value)
                    .and_then(|b| HoldersCountEntry::try_from_slice(&b).ok())
                    .map(|hc| hc.count)
                    .unwrap_or(0)
            });
        let inspection_json = record.inspection.as_ref().map(inspection_to_json);
        let name = record.names.first().cloned();
        let symbol = record.symbols.first().cloned();

        Ok(RpcGetAlkaneInfoResult {
            value: json!({
                "ok": true,
                "alkane": format!("{}:{}", record.alkane.block, record.alkane.tx),
                "creation_txid": hex::encode(record.txid),
                "creation_height": record.creation_height,
                "creation_timestamp": record.creation_timestamp,
                "tx_index_in_block": record.tx_index_in_block,
                "name": name,
                "symbol": symbol,
                "names": record.names,
                "symbols": record.symbols,
                "holder_count": holder_count,
                "inspection": inspection_json,
            }),
        })
    }

    pub fn rpc_get_block_summary(
        &self,
        params: RpcGetBlockSummaryParams,
    ) -> Result<RpcGetBlockSummaryResult> {
        let Some(height) = params.height else {
            return Ok(RpcGetBlockSummaryResult {
                value: json!({"ok": false, "error": "missing_or_invalid_height"}),
            });
        };
        let height = height as u32;
        let summary = self
            .get_block_summary(GetBlockSummaryParams { height })
            .ok()
            .and_then(|resp| resp.summary);

        let (trace_count, header_hex, found) = if let Some(summary) = summary {
            (summary.trace_count, Some(hex::encode(summary.header)), true)
        } else {
            (0, None, false)
        };

        Ok(RpcGetBlockSummaryResult {
            value: json!({
                "ok": true,
                "height": height,
                "found": found,
                "trace_count": trace_count,
                "header_hex": header_hex,
            }),
        })
    }

    pub fn rpc_get_holders(&self, params: RpcGetHoldersParams) -> Result<RpcGetHoldersResult> {
        let Some(alk_raw) = params.alkane.as_deref() else {
            return Ok(RpcGetHoldersResult {
                value: json!({"ok": false, "error": "missing_or_invalid_alkane"}),
            });
        };
        let Some(alk) = parse_alkane_from_str(alk_raw) else {
            return Ok(RpcGetHoldersResult {
                value: json!({"ok": false, "error": "missing_or_invalid_alkane"}),
            });
        };

        let limit = params.limit.unwrap_or(100).max(1) as usize;
        let page = params.page.unwrap_or(1).max(1) as usize;

        let (total, _supply, slice) = match get_holders_for_alkane(self, alk, page, limit) {
            Ok(tup) => tup,
            Err(_) => {
                return Ok(RpcGetHoldersResult {
                    value: json!({"ok": false, "error": "internal_error"}),
                });
            }
        };

        let has_more = page.saturating_mul(limit) < total;
        let items: Vec<Value> = slice
            .into_iter()
            .map(|h| match h.holder {
                HolderId::Address(addr) => json!({
                    "type": "address",
                    "address": addr,
                    "amount": h.amount.to_string()
                }),
                HolderId::Alkane(id) => json!({
                    "type": "alkane",
                    "alkane": format!("{}:{}", id.block, id.tx),
                    "amount": h.amount.to_string()
                }),
            })
            .collect();

        Ok(RpcGetHoldersResult {
            value: json!({
                "ok": true,
                "alkane": format!("{}:{}", alk.block, alk.tx),
                "page": page,
                "limit": limit,
                "total": total,
                "has_more": has_more,
                "items": items
            }),
        })
    }

    pub fn rpc_get_transfer_volume(
        &self,
        params: RpcGetTransferVolumeParams,
    ) -> Result<RpcGetTransferVolumeResult> {
        let Some(alk_raw) = params.alkane.as_deref() else {
            return Ok(RpcGetTransferVolumeResult {
                value: json!({"ok": false, "error": "missing_or_invalid_alkane"}),
            });
        };
        let Some(alk) = parse_alkane_from_str(alk_raw) else {
            return Ok(RpcGetTransferVolumeResult {
                value: json!({"ok": false, "error": "missing_or_invalid_alkane"}),
            });
        };

        let limit = params.limit.unwrap_or(100).max(1) as usize;
        let page = params.page.unwrap_or(1).max(1) as usize;

        let (total, slice) = match get_transfer_volume_for_alkane(self, alk, page, limit) {
            Ok(tup) => tup,
            Err(_) => {
                return Ok(RpcGetTransferVolumeResult {
                    value: json!({"ok": false, "error": "internal_error"}),
                });
            }
        };

        let has_more = page.saturating_mul(limit) < total;
        let items: Vec<Value> = slice
            .into_iter()
            .map(|entry| {
                json!({
                    "address": entry.address,
                    "amount": entry.amount.to_string()
                })
            })
            .collect();

        Ok(RpcGetTransferVolumeResult {
            value: json!({
                "ok": true,
                "alkane": format!("{}:{}", alk.block, alk.tx),
                "page": page,
                "limit": limit,
                "total": total,
                "has_more": has_more,
                "items": items
            }),
        })
    }

    pub fn rpc_get_total_received(
        &self,
        params: RpcGetTotalReceivedParams,
    ) -> Result<RpcGetTotalReceivedResult> {
        let Some(alk_raw) = params.alkane.as_deref() else {
            return Ok(RpcGetTotalReceivedResult {
                value: json!({"ok": false, "error": "missing_or_invalid_alkane"}),
            });
        };
        let Some(alk) = parse_alkane_from_str(alk_raw) else {
            return Ok(RpcGetTotalReceivedResult {
                value: json!({"ok": false, "error": "missing_or_invalid_alkane"}),
            });
        };

        let limit = params.limit.unwrap_or(100).max(1) as usize;
        let page = params.page.unwrap_or(1).max(1) as usize;

        let (total, slice) = match get_total_received_for_alkane(self, alk, page, limit) {
            Ok(tup) => tup,
            Err(_) => {
                return Ok(RpcGetTotalReceivedResult {
                    value: json!({"ok": false, "error": "internal_error"}),
                });
            }
        };

        let has_more = page.saturating_mul(limit) < total;
        let items: Vec<Value> = slice
            .into_iter()
            .map(|entry| {
                json!({
                    "address": entry.address,
                    "amount": entry.amount.to_string()
                })
            })
            .collect();

        Ok(RpcGetTotalReceivedResult {
            value: json!({
                "ok": true,
                "alkane": format!("{}:{}", alk.block, alk.tx),
                "page": page,
                "limit": limit,
                "total": total,
                "has_more": has_more,
                "items": items
            }),
        })
    }

    pub fn rpc_get_circulating_supply(
        &self,
        params: RpcGetCirculatingSupplyParams,
    ) -> Result<RpcGetCirculatingSupplyResult> {
        let Some(alk_raw) = params.alkane.as_deref() else {
            return Ok(RpcGetCirculatingSupplyResult {
                value: json!({"ok": false, "error": "missing_or_invalid_alkane"}),
            });
        };
        let Some(alkane) = parse_alkane_from_str(alk_raw) else {
            return Ok(RpcGetCirculatingSupplyResult {
                value: json!({"ok": false, "error": "missing_or_invalid_alkane"}),
            });
        };

        if params.height_present && params.height.is_none() {
            return Ok(RpcGetCirculatingSupplyResult {
                value: json!({"ok": false, "error": "missing_or_invalid_height"}),
            });
        }

        let (supply, height_value) = if params.height_present {
            let height_val = params.height.unwrap();
            let height_u32 = match u32::try_from(height_val) {
                Ok(v) => v,
                Err(_) => {
                    return Ok(RpcGetCirculatingSupplyResult {
                        value: json!({"ok": false, "error": "height_out_of_range"}),
                    });
                }
            };
            let supply = self
                .get_circulating_supply(GetCirculatingSupplyParams { alkane, height: height_u32 })?
                .supply;
            (supply, json!(height_val))
        } else {
            let supply = self
                .get_latest_circulating_supply(GetLatestCirculatingSupplyParams { alkane })?
                .supply;
            (supply, Value::String("latest".to_string()))
        };

        let mut body = Map::new();
        body.insert("ok".to_string(), Value::Bool(true));
        body.insert("alkane".to_string(), Value::String(format!("{}:{}", alkane.block, alkane.tx)));
        body.insert("supply".to_string(), Value::String(supply.to_string()));
        body.insert("height".to_string(), height_value);

        Ok(RpcGetCirculatingSupplyResult { value: Value::Object(body) })
    }

    pub fn rpc_get_address_activity(
        &self,
        params: RpcGetAddressActivityParams,
    ) -> Result<RpcGetAddressActivityResult> {
        let Some(address_raw) = params.address.as_deref().map(str::trim).filter(|s| !s.is_empty())
        else {
            return Ok(RpcGetAddressActivityResult {
                value: json!({"ok": false, "error": "missing_or_invalid_address"}),
            });
        };
        let Some(address) = normalize_address(address_raw) else {
            return Ok(RpcGetAddressActivityResult {
                value: json!({"ok": false, "error": "invalid_address_format"}),
            });
        };

        let activity = match get_address_activity_for_address(self, &address) {
            Ok(entry) => entry,
            Err(_) => {
                return Ok(RpcGetAddressActivityResult {
                    value: json!({"ok": false, "error": "internal_error"}),
                });
            }
        };

        let mut transfer_volume: Map<String, Value> = Map::new();
        for (alk, amt) in activity.transfer_volume {
            transfer_volume
                .insert(format!("{}:{}", alk.block, alk.tx), Value::String(amt.to_string()));
        }
        let mut total_received: Map<String, Value> = Map::new();
        for (alk, amt) in activity.total_received {
            total_received
                .insert(format!("{}:{}", alk.block, alk.tx), Value::String(amt.to_string()));
        }

        Ok(RpcGetAddressActivityResult {
            value: json!({
                "ok": true,
                "address": address,
                "transfer_volume": Value::Object(transfer_volume),
                "total_received": Value::Object(total_received),
            }),
        })
    }

    pub fn rpc_get_address_balances(
        &self,
        params: RpcGetAddressBalancesParams,
    ) -> Result<RpcGetAddressBalancesResult> {
        let Some(address_raw) = params.address.as_deref().map(str::trim).filter(|s| !s.is_empty())
        else {
            return Ok(RpcGetAddressBalancesResult {
                value: json!({"ok": false, "error": "missing_or_invalid_address"}),
            });
        };
        let Some(address) = normalize_address(address_raw) else {
            return Ok(RpcGetAddressBalancesResult {
                value: json!({"ok": false, "error": "invalid_address_format"}),
            });
        };

        let include_outpoints = params.include_outpoints.unwrap_or(false);

        let agg = match get_balance_for_address(self, &address) {
            Ok(m) => m,
            Err(_) => {
                return Ok(RpcGetAddressBalancesResult {
                    value: json!({"ok": false, "error": "internal_error"}),
                });
            }
        };

        let mut balances: Map<String, Value> = Map::new();
        for (id, amt) in agg {
            balances.insert(format!("{}:{}", id.block, id.tx), Value::String(amt.to_string()));
        }

        let mut resp = json!({
            "ok": true,
            "address": address,
            "balances": Value::Object(balances),
        });

        if include_outpoints {
            let mut pref = b"/balances/".to_vec();
            pref.extend_from_slice(resp["address"].as_str().unwrap().as_bytes());
            pref.push(b'/');

            let keys = match self.get_scan_prefix(GetScanPrefixParams { prefix: pref.clone() }) {
                Ok(v) => v.keys,
                Err(_) => Vec::new(),
            };

            let mut outpoints = Vec::with_capacity(keys.len());
            for k in keys {
                let val = match self.get_raw_value(GetRawValueParams { key: k.clone() }) {
                    Ok(resp) => match resp.value {
                        Some(v) => v,
                        None => continue,
                    },
                    Err(_) => continue,
                };
                let entries = match decode_balances_vec(&val) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let op = match std::str::from_utf8(&k[pref.len()..]) {
                    Ok(s) => s.to_string(),
                    Err(_) => continue,
                };
                let entry_list: Vec<Value> = entries
                    .into_iter()
                    .map(|be| {
                        json!({
                            "alkane": format!("{}:{}", be.alkane.block, be.alkane.tx),
                            "amount": be.amount.to_string()
                        })
                    })
                    .collect();

                outpoints.push(json!({ "outpoint": op, "entries": entry_list }));
            }

            resp.as_object_mut()
                .unwrap()
                .insert("outpoints".to_string(), Value::Array(outpoints));
        }

        Ok(RpcGetAddressBalancesResult { value: resp })
    }

    pub fn rpc_get_alkane_balances(
        &self,
        params: RpcGetAlkaneBalancesParams,
    ) -> Result<RpcGetAlkaneBalancesResult> {
        let Some(alk_raw) = params.alkane.as_deref() else {
            return Ok(RpcGetAlkaneBalancesResult {
                value: json!({"ok": false, "error": "missing_or_invalid_alkane"}),
            });
        };
        let Some(alk) = parse_alkane_from_str(alk_raw) else {
            return Ok(RpcGetAlkaneBalancesResult {
                value: json!({"ok": false, "error": "missing_or_invalid_alkane"}),
            });
        };

        if params.height_present && params.height.is_none() {
            return Ok(RpcGetAlkaneBalancesResult {
                value: json!({"ok": false, "error": "missing_or_invalid_height"}),
            });
        }

        let mut resolved_height: Option<u32> = None;
        let agg = if params.height_present {
            let height_val = params.height.unwrap();
            let height_u32 = match u32::try_from(height_val) {
                Ok(v) => v,
                Err(_) => {
                    return Ok(RpcGetAlkaneBalancesResult {
                        value: json!({"ok": false, "error": "height_out_of_range"}),
                    });
                }
            };
            match get_alkane_balances_at_or_before(self, &alk, height_u32) {
                Ok((m, found_height)) => {
                    resolved_height = found_height;
                    m
                }
                Err(_) => {
                    return Ok(RpcGetAlkaneBalancesResult {
                        value: json!({"ok": false, "error": "internal_error"}),
                    });
                }
            }
        } else {
            match get_alkane_balances(self, &alk) {
                Ok(m) => m,
                Err(_) => {
                    return Ok(RpcGetAlkaneBalancesResult {
                        value: json!({"ok": false, "error": "internal_error"}),
                    });
                }
            }
        };

        let mut balances: Map<String, Value> = Map::new();
        for (id, amt) in agg {
            balances.insert(format!("{}:{}", id.block, id.tx), Value::String(amt.to_string()));
        }

        let mut body = Map::new();
        body.insert("ok".to_string(), Value::Bool(true));
        body.insert("alkane".to_string(), Value::String(format!("{}:{}", alk.block, alk.tx)));
        body.insert("balances".to_string(), Value::Object(balances));
        if params.height_present {
            body.insert("requested_height".to_string(), json!(params.height.unwrap()));
            body.insert(
                "resolved_height".to_string(),
                resolved_height.map(|h| json!(h)).unwrap_or(Value::Null),
            );
        }

        Ok(RpcGetAlkaneBalancesResult { value: Value::Object(body) })
    }

    pub fn rpc_get_alkane_balance_metashrew(
        &self,
        params: RpcGetAlkaneBalanceMetashrewParams,
    ) -> Result<RpcGetAlkaneBalanceMetashrewResult> {
        let Some(owner_raw) = params.owner.as_deref() else {
            return Ok(RpcGetAlkaneBalanceMetashrewResult {
                value: json!({"ok": false, "error": "missing_or_invalid_owner"}),
            });
        };
        let Some(owner) = parse_alkane_from_str(owner_raw) else {
            return Ok(RpcGetAlkaneBalanceMetashrewResult {
                value: json!({"ok": false, "error": "missing_or_invalid_owner"}),
            });
        };

        let Some(target_raw) = params.target.as_deref() else {
            return Ok(RpcGetAlkaneBalanceMetashrewResult {
                value: json!({"ok": false, "error": "missing_or_invalid_target"}),
            });
        };
        let Some(target) = parse_alkane_from_str(target_raw) else {
            return Ok(RpcGetAlkaneBalanceMetashrewResult {
                value: json!({"ok": false, "error": "missing_or_invalid_target"}),
            });
        };

        if params.height_present && params.height.is_none() {
            return Ok(RpcGetAlkaneBalanceMetashrewResult {
                value: json!({"ok": false, "error": "missing_or_invalid_height"}),
            });
        }

        match get_metashrew().get_reserves_for_alkane(&owner, &target, params.height) {
            Ok(Some(bal)) => Ok(RpcGetAlkaneBalanceMetashrewResult {
                value: json!({
                    "ok": true,
                    "owner": format!("{}:{}", owner.block, owner.tx),
                    "alkane": format!("{}:{}", target.block, target.tx),
                    "balance": bal.to_string(),
                }),
            }),
            Ok(None) => Ok(RpcGetAlkaneBalanceMetashrewResult {
                value: json!({
                    "ok": true,
                    "owner": format!("{}:{}", owner.block, owner.tx),
                    "alkane": format!("{}:{}", target.block, target.tx),
                    "balance": "0",
                }),
            }),
            Err(_) => Ok(RpcGetAlkaneBalanceMetashrewResult {
                value: json!({"ok": false, "error": "metashrew_error"}),
            }),
        }
    }

    pub fn rpc_get_alkane_balance_txs(
        &self,
        params: RpcGetAlkaneBalanceTxsParams,
    ) -> Result<RpcGetAlkaneBalanceTxsResult> {
        let Some(alk_raw) = params.alkane.as_deref() else {
            return Ok(RpcGetAlkaneBalanceTxsResult {
                value: json!({"ok": false, "error": "missing_or_invalid_alkane"}),
            });
        };
        let Some(alk) = parse_alkane_from_str(alk_raw) else {
            return Ok(RpcGetAlkaneBalanceTxsResult {
                value: json!({"ok": false, "error": "missing_or_invalid_alkane"}),
            });
        };

        let limit = params.limit.unwrap_or(100).max(1) as usize;
        let page = params.page.unwrap_or(1).max(1) as usize;

        let table = self.table();
        let total: usize;
        let mut slice: Vec<AlkaneBalanceTxEntry> = Vec::new();

        let meta = self
            .get_raw_value(GetRawValueParams { key: table.alkane_balance_txs_meta_key(&alk) })
            .ok()
            .and_then(|resp| resp.value)
            .and_then(|bytes| AlkaneBalanceTxsMeta::try_from_slice(&bytes).ok());

        if let Some(meta) = meta {
            let page_size = meta.page_size.max(1) as usize;
            total = meta.total_len as usize;
            let off = limit.saturating_mul(page.saturating_sub(1));
            let end = (off + limit).min(total);
            if off < total {
                let start_page = off / page_size;
                let mut end_page = (end.saturating_sub(1)) / page_size;
                let max_page = meta.last_page as usize;
                if end_page > max_page {
                    end_page = max_page;
                }
                for page_idx in start_page..=end_page {
                    let key = table.alkane_balance_txs_page_key(&alk, page_idx as u64);
                    if let Ok(resp) = self.get_raw_value(GetRawValueParams { key }) {
                        if let Some(bytes) = resp.value {
                            if let Ok(list) = decode_alkane_balance_tx_entries(&bytes) {
                                let page_start = page_idx * page_size;
                                let local_start = off.saturating_sub(page_start);
                                let local_end = (end.saturating_sub(page_start)).min(list.len());
                                if local_start < local_end && local_start < list.len() {
                                    slice.extend_from_slice(&list[local_start..local_end]);
                                }
                            }
                        }
                    }
                }
            }
        } else {
            let mut txs: Vec<AlkaneBalanceTxEntry> = Vec::new();
            if let Ok(resp) =
                self.get_raw_value(GetRawValueParams { key: table.alkane_balance_txs_key(&alk) })
            {
                if let Some(bytes) = resp.value {
                    if let Ok(list) = decode_alkane_balance_tx_entries(&bytes) {
                        txs = list;
                    }
                }
            }
            total = txs.len();
            let off = limit.saturating_mul(page.saturating_sub(1));
            let end = (off + limit).min(total);
            if off < total {
                slice = txs[off..end].to_vec();
            }
        }

        let items: Vec<Value> = slice
            .into_iter()
            .map(|entry| {
                let mut outflow: Map<String, Value> = Map::new();
                for (id, delta) in entry.outflow {
                    outflow.insert(
                        format!("{}:{}", id.block, id.tx),
                        Value::String(delta.to_string()),
                    );
                }
                json!({
                    "txid": Txid::from_byte_array(entry.txid).to_string(),
                    "height": entry.height,
                    "outflow": Value::Object(outflow),
                })
            })
            .collect();

        Ok(RpcGetAlkaneBalanceTxsResult {
            value: json!({
                "ok": true,
                "alkane": format!("{}:{}", alk.block, alk.tx),
                "page": page,
                "limit": limit,
                "total": total,
                "has_more": limit.saturating_mul(page.saturating_sub(1)) + items.len() < total,
                "txids": items
            }),
        })
    }

    pub fn rpc_get_alkane_balance_txs_by_token(
        &self,
        params: RpcGetAlkaneBalanceTxsByTokenParams,
    ) -> Result<RpcGetAlkaneBalanceTxsByTokenResult> {
        let Some(owner_raw) = params.owner.as_deref() else {
            return Ok(RpcGetAlkaneBalanceTxsByTokenResult {
                value: json!({"ok": false, "error": "missing_or_invalid_owner"}),
            });
        };
        let Some(owner) = parse_alkane_from_str(owner_raw) else {
            return Ok(RpcGetAlkaneBalanceTxsByTokenResult {
                value: json!({"ok": false, "error": "missing_or_invalid_owner"}),
            });
        };
        let Some(token_raw) = params.token.as_deref() else {
            return Ok(RpcGetAlkaneBalanceTxsByTokenResult {
                value: json!({"ok": false, "error": "missing_or_invalid_token"}),
            });
        };
        let Some(token) = parse_alkane_from_str(token_raw) else {
            return Ok(RpcGetAlkaneBalanceTxsByTokenResult {
                value: json!({"ok": false, "error": "missing_or_invalid_token"}),
            });
        };

        let limit = params.limit.unwrap_or(100).max(1) as usize;
        let page = params.page.unwrap_or(1).max(1) as usize;

        let table = self.table();
        let total: usize;
        let mut slice: Vec<AlkaneBalanceTxEntry> = Vec::new();

        let meta = self
            .get_raw_value(GetRawValueParams {
                key: table.alkane_balance_txs_by_token_meta_key(&owner, &token),
            })
            .ok()
            .and_then(|resp| resp.value)
            .and_then(|bytes| AlkaneBalanceTxsMeta::try_from_slice(&bytes).ok());

        if let Some(meta) = meta {
            let page_size = meta.page_size.max(1) as usize;
            total = meta.total_len as usize;
            let off = limit.saturating_mul(page.saturating_sub(1));
            let end = (off + limit).min(total);
            if off < total {
                let start_page = off / page_size;
                let mut end_page = (end.saturating_sub(1)) / page_size;
                let max_page = meta.last_page as usize;
                if end_page > max_page {
                    end_page = max_page;
                }
                for page_idx in start_page..=end_page {
                    let key =
                        table.alkane_balance_txs_by_token_page_key(&owner, &token, page_idx as u64);
                    if let Ok(resp) = self.get_raw_value(GetRawValueParams { key }) {
                        if let Some(bytes) = resp.value {
                            if let Ok(list) = decode_alkane_balance_tx_entries(&bytes) {
                                let page_start = page_idx * page_size;
                                let local_start = off.saturating_sub(page_start);
                                let local_end = (end.saturating_sub(page_start)).min(list.len());
                                if local_start < local_end && local_start < list.len() {
                                    slice.extend_from_slice(&list[local_start..local_end]);
                                }
                            }
                        }
                    }
                }
            }
        } else {
            let mut txs: Vec<AlkaneBalanceTxEntry> = Vec::new();
            if let Ok(resp) = self.get_raw_value(GetRawValueParams {
                key: table.alkane_balance_txs_by_token_key(&owner, &token),
            }) {
                if let Some(bytes) = resp.value {
                    if let Ok(list) = decode_alkane_balance_tx_entries(&bytes) {
                        txs = list;
                    }
                }
            }

            total = txs.len();
            let off = limit.saturating_mul(page.saturating_sub(1));
            let end = (off + limit).min(total);
            if off < total {
                slice = txs[off..end].to_vec();
            }
        }

        let items: Vec<Value> = slice
            .into_iter()
            .map(|entry| {
                let mut outflow: Map<String, Value> = Map::new();
                for (id, delta) in entry.outflow {
                    outflow.insert(
                        format!("{}:{}", id.block, id.tx),
                        Value::String(delta.to_string()),
                    );
                }
                json!({
                    "txid": Txid::from_byte_array(entry.txid).to_string(),
                    "height": entry.height,
                    "outflow": Value::Object(outflow),
                })
            })
            .collect();

        Ok(RpcGetAlkaneBalanceTxsByTokenResult {
            value: json!({
                "ok": true,
                "owner": format!("{}:{}", owner.block, owner.tx),
                "token": format!("{}:{}", token.block, token.tx),
                "page": page,
                "limit": limit,
                "total": total,
                "has_more": limit.saturating_mul(page.saturating_sub(1)) + items.len() < total,
                "txids": items
            }),
        })
    }

    pub fn rpc_get_outpoint_balances(
        &self,
        params: RpcGetOutpointBalancesParams,
    ) -> Result<RpcGetOutpointBalancesResult> {
        let Some(outpoint) = params.outpoint.as_deref().map(str::trim).filter(|s| !s.is_empty())
        else {
            return Ok(RpcGetOutpointBalancesResult {
                value: json!({
                    "ok": false,
                    "error": "missing_or_invalid_outpoint",
                    "hint": "expected \"<txid>:<vout>\""
                }),
            });
        };

        let (txid, vout_u32) = match parse_outpoint_str(outpoint) {
            Ok(tup) => tup,
            Err(err_val) => {
                return Ok(RpcGetOutpointBalancesResult { value: err_val });
            }
        };

        let table = self.table();
        let entries = match get_outpoint_balances_index(self, &txid, vout_u32) {
            Ok(v) => v,
            Err(_) => {
                return Ok(RpcGetOutpointBalancesResult {
                    value: json!({"ok": false, "error": "internal_error"}),
                });
            }
        };

        let addr = {
            let pref = table.outpoint_balances_prefix(txid.to_byte_array().as_slice(), vout_u32);
            if let Ok(pref) = pref {
                if let Ok(keys_resp) =
                    self.get_scan_prefix(GetScanPrefixParams { prefix: pref.clone() })
                {
                    let keys = keys_resp.keys;
                    if let Some(full_key) = keys.first() {
                        let raw = &full_key[b"/outpoint_balances/".len()..];
                        if let Ok(op) = EspoOutpoint::try_from_slice(raw) {
                            let key_new = table.outpoint_addr_key(&op).ok();
                            key_new
                                .and_then(|k| {
                                    self.get_raw_value(GetRawValueParams { key: k })
                                        .ok()
                                        .and_then(|resp| resp.value)
                                })
                                .and_then(|b| std::str::from_utf8(&b).ok().map(|s| s.to_string()))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        };

        let entry_list: Vec<Value> = entries
            .into_iter()
            .map(|be| {
                json!({
                    "alkane": format!("{}:{}", be.alkane.block, be.alkane.tx),
                    "amount": be.amount.to_string()
                })
            })
            .collect();

        let mut item = json!({
            "outpoint": outpoint,
            "entries": entry_list
        });
        if let Some(a) = addr {
            item.as_object_mut().unwrap().insert("address".to_string(), Value::String(a));
        }

        Ok(RpcGetOutpointBalancesResult {
            value: json!({
                "ok": true,
                "outpoint": item["outpoint"],
                "items": [item]
            }),
        })
    }

    pub fn rpc_get_block_traces(
        &self,
        params: RpcGetBlockTracesParams,
    ) -> Result<RpcGetBlockTracesResult> {
        let Some(height) = params.height else {
            return Ok(RpcGetBlockTracesResult {
                value: json!({
                    "ok": false,
                    "error": "missing_or_invalid_height",
                    "hint": "expected {\"height\": <u64>}"
                }),
            });
        };

        let partials = match get_metashrew().traces_for_block_as_prost(height) {
            Ok(v) => v,
            Err(_) => {
                return Ok(RpcGetBlockTracesResult {
                    value: json!({"ok": false, "error": "metashrew_fetch_failed"}),
                });
            }
        };

        let mut traces: Vec<Value> = Vec::with_capacity(partials.len());
        for p in partials {
            if p.outpoint.len() < 36 {
                continue;
            }
            let (txid_le, vout_le) = p.outpoint.split_at(32);
            let mut txid_be = txid_le.to_vec();
            txid_be.reverse();
            let txid_hex = hex::encode(&txid_be);
            let vout = u32::from_le_bytes(vout_le[..4].try_into().expect("vout 4 bytes"));

            let events_str = match prettyify_protobuf_trace_json(&p.protobuf_trace) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let events: Value = serde_json::from_str(&events_str).unwrap_or(Value::Null);

            traces.push(json!({
                "outpoint": format!("{txid_hex}:{vout}"),
                "events": events
            }));
        }

        Ok(RpcGetBlockTracesResult {
            value: json!({
                "ok": true,
                "height": height,
                "traces": traces
            }),
        })
    }

    pub fn rpc_get_holders_count(
        &self,
        params: RpcGetHoldersCountParams,
    ) -> Result<RpcGetHoldersCountResult> {
        let Some(alk_raw) = params.alkane.as_deref() else {
            return Ok(RpcGetHoldersCountResult {
                value: json!({"ok": false, "error": "missing_or_invalid_alkane"}),
            });
        };
        let Some(alkane) = parse_alkane_from_str(alk_raw) else {
            return Ok(RpcGetHoldersCountResult {
                value: json!({"ok": false, "error": "missing_or_invalid_alkane"}),
            });
        };

        let table = self.table();
        let count: u64 = match HoldersCountEntry::try_from_slice(
            &self
                .get_raw_value(GetRawValueParams { key: table.holders_count_key(&alkane) })
                .ok()
                .and_then(|resp| resp.value)
                .unwrap_or_else(Vec::new),
        ) {
            Ok(count_value) => count_value.count,
            Err(_) => {
                return Ok(RpcGetHoldersCountResult {
                    value: json!({"ok": false, "error": "missing_or_invalid_outpoint"}),
                });
            }
        };

        Ok(RpcGetHoldersCountResult {
            value: json!({
                "ok": true,
                "count": count,
            }),
        })
    }

    pub fn rpc_get_address_outpoints(
        &self,
        params: RpcGetAddressOutpointsParams,
    ) -> Result<RpcGetAddressOutpointsResult> {
        let Some(address_raw) = params.address.as_deref().map(str::trim).filter(|s| !s.is_empty())
        else {
            return Ok(RpcGetAddressOutpointsResult {
                value: json!({"ok": false, "error": "missing_or_invalid_address"}),
            });
        };
        let Some(address) = normalize_address(address_raw) else {
            return Ok(RpcGetAddressOutpointsResult {
                value: json!({"ok": false, "error": "invalid_address_format"}),
            });
        };

        let mut pref = b"/balances/".to_vec();
        pref.extend_from_slice(address.as_bytes());
        pref.push(b'/');

        let keys = match self.get_scan_prefix(GetScanPrefixParams { prefix: pref.clone() }) {
            Ok(v) => v.keys,
            Err(_) => Vec::new(),
        };

        let mut outpoints: Vec<Value> = Vec::with_capacity(keys.len());
        for k in keys {
            if k.len() <= pref.len() {
                continue;
            }

            let decoded = EspoOutpoint::try_from_slice(&k[pref.len()..]);
            let espo_out = match decoded {
                Ok(op) => op,
                Err(_) => continue,
            };

            if espo_out.tx_spent.is_some() {
                continue;
            }

            let outpoint_str = espo_out.as_outpoint_string();
            let (txid, vout) = match outpoint_str.split_once(':') {
                Some((txid_hex, vout_s)) => {
                    let tid = match bitcoin::Txid::from_str(txid_hex) {
                        Ok(t) => t,
                        Err(_) => continue,
                    };
                    let v = match vout_s.parse::<u32>() {
                        Ok(n) => n,
                        Err(_) => continue,
                    };
                    (tid, v)
                }
                None => continue,
            };

            let entries_vec = match get_outpoint_balances_index(self, &txid, vout) {
                Ok(v) => v,
                Err(_) => Vec::new(),
            };

            let entry_list: Vec<Value> = entries_vec
                .into_iter()
                .map(|be| {
                    json!({
                        "alkane": format!("{}:{}", be.alkane.block, be.alkane.tx),
                        "amount": be.amount.to_string()
                    })
                })
                .collect();

            outpoints.push(json!({
                "outpoint": outpoint_str,
                "entries": entry_list
            }));
        }

        outpoints.sort_by(|a, b| {
            let sa = a.get("outpoint").and_then(|v| v.as_str()).unwrap_or_default();
            let sb = b.get("outpoint").and_then(|v| v.as_str()).unwrap_or_default();
            sa.cmp(sb)
        });
        outpoints.dedup_by(|a, b| {
            a.get("outpoint").and_then(|v| v.as_str()) == b.get("outpoint").and_then(|v| v.as_str())
        });

        Ok(RpcGetAddressOutpointsResult {
            value: json!({
                "ok": true,
                "address": address,
                "outpoints": outpoints
            }),
        })
    }

    pub fn rpc_get_alkane_tx_summary(
        &self,
        params: RpcGetAlkaneTxSummaryParams,
    ) -> Result<RpcGetAlkaneTxSummaryResult> {
        let Some(txid_hex) = params.txid.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(RpcGetAlkaneTxSummaryResult {
                value: json!({"ok": false, "error": "missing_or_invalid_txid"}),
            });
        };
        let txid = match Txid::from_str(txid_hex) {
            Ok(t) => t,
            Err(_) => {
                return Ok(RpcGetAlkaneTxSummaryResult {
                    value: json!({"ok": false, "error": "invalid_txid_format"}),
                });
            }
        };

        let table = self.table();
        let key = table.alkane_tx_summary_key(&txid.to_byte_array());
        let Some(bytes) =
            self.get_raw_value(GetRawValueParams { key }).ok().and_then(|resp| resp.value)
        else {
            return Ok(RpcGetAlkaneTxSummaryResult {
                value: json!({"ok": false, "error": "not_found"}),
            });
        };
        let summary = match AlkaneTxSummary::try_from_slice(&bytes) {
            Ok(s) => s,
            Err(_) => {
                return Ok(RpcGetAlkaneTxSummaryResult {
                    value: json!({"ok": false, "error": "decode_failed"}),
                });
            }
        };

        let traces_json = serde_json::to_value(&summary.traces).unwrap_or(Value::Null);
        let mut outflows_json: Vec<Value> = Vec::new();
        for entry in &summary.outflows {
            let mut outflow_map = Map::new();
            for (alk, delta) in &entry.outflow {
                outflow_map
                    .insert(format!("{}:{}", alk.block, alk.tx), Value::String(delta.to_string()));
            }
            outflows_json.push(json!({
                "txid": Txid::from_byte_array(entry.txid).to_string(),
                "height": entry.height,
                "outflow": outflow_map,
            }));
        }

        Ok(RpcGetAlkaneTxSummaryResult {
            value: json!({
                "ok": true,
                "txid": txid.to_string(),
                "height": summary.height,
                "traces": traces_json,
                "outflows": outflows_json,
            }),
        })
    }

    pub fn rpc_get_alkane_block_txs(
        &self,
        params: RpcGetAlkaneBlockTxsParams,
    ) -> Result<RpcGetAlkaneBlockTxsResult> {
        let Some(height) = params.height else {
            return Ok(RpcGetAlkaneBlockTxsResult {
                value: json!({"ok": false, "error": "missing_or_invalid_height"}),
            });
        };
        let page = params.page.unwrap_or(1).max(1) as usize;
        let limit = params.limit.unwrap_or(50).max(1) as usize;
        let off = limit.saturating_mul(page.saturating_sub(1));

        let table = self.table();
        let total = self
            .get_raw_value(GetRawValueParams { key: table.alkane_block_len_key(height) })
            .ok()
            .and_then(|resp| resp.value)
            .and_then(|b| {
                if b.len() == 8 {
                    let mut arr = [0u8; 8];
                    arr.copy_from_slice(&b);
                    Some(u64::from_le_bytes(arr) as usize)
                } else {
                    None
                }
            })
            .unwrap_or(0);

        if total == 0 {
            return Ok(RpcGetAlkaneBlockTxsResult {
                value: json!({
                    "ok": true,
                    "height": height,
                    "page": page,
                    "limit": limit,
                    "total": 0,
                    "txids": []
                }),
            });
        }

        let end = (off + limit).min(total);
        let mut keys: Vec<Vec<u8>> = Vec::new();
        for idx in off..end {
            keys.push(table.alkane_block_txid_key(height, idx as u64));
        }
        let vals = self
            .get_multi_values(GetMultiValuesParams { keys })
            .map(|r| r.values)
            .unwrap_or_default();
        let mut txids: Vec<String> = Vec::new();
        for v in vals {
            let Some(bytes) = v else { continue };
            if bytes.len() != 32 {
                continue;
            }
            if let Ok(txid) = Txid::from_slice(&bytes) {
                txids.push(txid.to_string());
            }
        }

        Ok(RpcGetAlkaneBlockTxsResult {
            value: json!({
                "ok": true,
                "height": height,
                "page": page,
                "limit": limit,
                "total": total,
                "txids": txids
            }),
        })
    }

    pub fn rpc_get_alkane_address_txs(
        &self,
        params: RpcGetAlkaneAddressTxsParams,
    ) -> Result<RpcGetAlkaneAddressTxsResult> {
        let Some(address_raw) = params.address.as_deref().map(str::trim).filter(|s| !s.is_empty())
        else {
            return Ok(RpcGetAlkaneAddressTxsResult {
                value: json!({"ok": false, "error": "missing_or_invalid_address"}),
            });
        };
        let Some(address) = normalize_address(address_raw) else {
            return Ok(RpcGetAlkaneAddressTxsResult {
                value: json!({"ok": false, "error": "invalid_address_format"}),
            });
        };

        let page = params.page.unwrap_or(1).max(1) as usize;
        let limit = params.limit.unwrap_or(50).max(1) as usize;
        let off = limit.saturating_mul(page.saturating_sub(1));

        let table = self.table();
        let total = self
            .get_raw_value(GetRawValueParams { key: table.alkane_address_len_key(&address) })
            .ok()
            .and_then(|resp| resp.value)
            .and_then(|b| {
                if b.len() == 8 {
                    let mut arr = [0u8; 8];
                    arr.copy_from_slice(&b);
                    Some(u64::from_le_bytes(arr) as usize)
                } else {
                    None
                }
            })
            .unwrap_or(0);

        if total == 0 {
            return Ok(RpcGetAlkaneAddressTxsResult {
                value: json!({
                    "ok": true,
                    "address": address,
                    "page": page,
                    "limit": limit,
                    "total": 0,
                    "txids": []
                }),
            });
        }

        let end = (off + limit).min(total);
        let mut keys: Vec<Vec<u8>> = Vec::new();
        for idx in off..end {
            keys.push(table.alkane_address_txid_key(&address, idx as u64));
        }
        let vals = self
            .get_multi_values(GetMultiValuesParams { keys })
            .map(|r| r.values)
            .unwrap_or_default();
        let mut txids: Vec<String> = Vec::new();
        for v in vals {
            let Some(bytes) = v else { continue };
            if bytes.len() != 32 {
                continue;
            }
            if let Ok(txid) = Txid::from_slice(&bytes) {
                txids.push(txid.to_string());
            }
        }

        Ok(RpcGetAlkaneAddressTxsResult {
            value: json!({
                "ok": true,
                "address": address,
                "page": page,
                "limit": limit,
                "total": total,
                "txids": txids
            }),
        })
    }

    pub fn rpc_get_address_transactions(
        &self,
        params: RpcGetAddressTransactionsParams,
    ) -> Result<RpcGetAddressTransactionsResult> {
        const DEFAULT_PAGE_LIMIT: usize = 25;
        const MAX_PAGE_LIMIT: usize = 200;
        let Some(address_raw) = params.address.as_deref().map(str::trim).filter(|s| !s.is_empty())
        else {
            return Ok(RpcGetAddressTransactionsResult {
                value: json!({"ok": false, "error": "missing_or_invalid_address"}),
            });
        };
        let Some(address) = normalize_address(address_raw) else {
            return Ok(RpcGetAddressTransactionsResult {
                value: json!({"ok": false, "error": "invalid_address_format"}),
            });
        };

        let page = params.page.unwrap_or(1).max(1);
        let limit = params
            .limit
            .unwrap_or(DEFAULT_PAGE_LIMIT as u64)
            .max(1)
            .min(MAX_PAGE_LIMIT as u64) as usize;
        let only_alkane_txs = params.only_alkane_txs.unwrap_or(true);
        let network = get_network();
        let page_offset = page.saturating_sub(1).try_into().unwrap_or(usize::MAX);
        let off = limit.saturating_mul(page_offset);

        let electrum_like = get_electrum_like();
        let address_obj =
            match Address::from_str(&address).and_then(|addr| addr.require_network(network)) {
                Ok(addr) => addr,
                Err(_) => {
                    return Ok(RpcGetAddressTransactionsResult {
                        value: json!({"ok": false, "error": "invalid_address_format"}),
                    });
                }
            };

        let mut pending_entries = pending_for_address(&address);
        pending_entries.sort_by(|a, b| b.txid.cmp(&a.txid));
        let pending_filtered: Vec<MempoolEntry> = pending_entries
            .into_iter()
            .filter(|entry| {
                !only_alkane_txs || entry.traces.as_ref().map_or(false, |t| !t.is_empty())
            })
            .collect();
        let pending_total = pending_filtered.len();
        let pending_slice_start = off.min(pending_total);
        let pending_slice_end = (off + limit).min(pending_total);
        let pending_set: HashSet<Txid> = pending_filtered.iter().map(|entry| entry.txid).collect();

        let mut tx_renders: Vec<AddressTxRender> = Vec::new();
        for entry in pending_filtered
            .iter()
            .skip(pending_slice_start)
            .take(pending_slice_end.saturating_sub(pending_slice_start))
        {
            tx_renders.push(AddressTxRender {
                txid: entry.txid,
                tx: entry.tx.clone(),
                traces: entry.traces.clone(),
                confirmations: None,
                is_mempool: true,
                summary: None,
            });
        }

        let remaining_slots = limit.saturating_sub(tx_renders.len());
        let chain_tip = get_bitcoind_rpc_client()
            .get_blockchain_info()
            .ok()
            .map(|info| info.blocks as u64);
        let table = self.table();

        let mut confirmed_total = if only_alkane_txs {
            self.get_raw_value(GetRawValueParams { key: table.alkane_address_len_key(&address) })
                .ok()
                .and_then(|resp| resp.value)
                .and_then(|b| {
                    if b.len() == 8 {
                        let mut arr = [0u8; 8];
                        arr.copy_from_slice(&b);
                        Some(u64::from_le_bytes(arr) as usize)
                    } else {
                        None
                    }
                })
                .unwrap_or(0)
        } else {
            0
        };

        if only_alkane_txs {
            let confirmed_offset = off.saturating_sub(pending_total);
            if remaining_slots > 0 {
                let confirmed_slice_start = confirmed_offset.min(confirmed_total);
                let confirmed_slice_end = (confirmed_offset + remaining_slots).min(confirmed_total);

                if confirmed_slice_end > confirmed_slice_start {
                    let mut txid_keys: Vec<Vec<u8>> = Vec::new();
                    for idx in confirmed_slice_start..confirmed_slice_end {
                        let rev_idx = confirmed_total - 1 - idx;
                        txid_keys.push(table.alkane_address_txid_key(&address, rev_idx as u64));
                    }
                    let txid_vals = self
                        .get_multi_values(GetMultiValuesParams { keys: txid_keys })
                        .ok()
                        .map(|resp| resp.values)
                        .unwrap_or_default();
                    let mut txids: Vec<Txid> = Vec::new();
                    for bytes in txid_vals {
                        if let Some(b) = bytes {
                            if b.len() == 32 {
                                if let Ok(txid) = Txid::from_slice(&b) {
                                    txids.push(txid);
                                }
                            }
                        }
                    }

                    if !txids.is_empty() {
                        let summary_keys: Vec<Vec<u8>> = txids
                            .iter()
                            .map(|t| table.alkane_tx_summary_key(&t.to_byte_array()))
                            .collect();
                        let summary_vals = self
                            .get_multi_values(GetMultiValuesParams { keys: summary_keys })
                            .ok()
                            .map(|resp| resp.values)
                            .unwrap_or_default();
                        let raw_txs =
                            electrum_like.batch_transaction_get_raw(&txids).unwrap_or_default();

                        for (idx, txid) in txids.iter().enumerate() {
                            let raw = raw_txs.get(idx).cloned().unwrap_or_default();
                            if raw.is_empty() {
                                continue;
                            }
                            let tx: Transaction = match deserialize(&raw) {
                                Ok(value) => value,
                                Err(e) => {
                                    eprintln!(
                                        "[rpc_get_address_transactions] failed to decode tx {}: {e}",
                                        txid
                                    );
                                    continue;
                                }
                            };
                            let summary = summary_vals
                                .get(idx)
                                .and_then(|v| v.as_ref())
                                .and_then(|b| AlkaneTxSummary::try_from_slice(b).ok());
                            let confirmations =
                                summary.as_ref().and_then(|s| {
                                    let h = s.height as u64;
                                    chain_tip.and_then(|tip| {
                                        if tip >= h { Some(tip - h + 1) } else { None }
                                    })
                                });
                            let traces = summary
                                .as_ref()
                                .map(|s| traces_from_summary(txid, s))
                                .filter(|v| !v.is_empty());
                            tx_renders.push(AddressTxRender {
                                txid: *txid,
                                tx,
                                traces,
                                confirmations,
                                is_mempool: false,
                                summary,
                            });
                        }
                    }
                }
            }
        } else {
            let confirmed_offset = off.saturating_sub(pending_total);
            let fetch_limit = remaining_slots.max(1);
            match electrum_like.address_history_page(&address_obj, confirmed_offset, fetch_limit) {
                Ok(hist_page) => {
                    let mut entries: Vec<AddressHistoryEntry> = hist_page
                        .entries
                        .into_iter()
                        .filter(|entry| !pending_set.contains(&entry.txid))
                        .collect();
                    confirmed_total = hist_page
                        .total
                        .unwrap_or(confirmed_offset + entries.len())
                        .max(entries.len());
                    if remaining_slots > 0 {
                        let to_take = remaining_slots.min(entries.len());
                        let entries_for_page = entries.drain(..to_take).collect::<Vec<_>>();
                        let txids: Vec<Txid> = entries_for_page.iter().map(|e| e.txid).collect();
                        if !txids.is_empty() {
                            let summary_keys: Vec<Vec<u8>> = txids
                                .iter()
                                .map(|t| table.alkane_tx_summary_key(&t.to_byte_array()))
                                .collect();
                            let summary_vals = self
                                .get_multi_values(GetMultiValuesParams { keys: summary_keys })
                                .ok()
                                .map(|resp| resp.values)
                                .unwrap_or_default();
                            let raw_txs =
                                electrum_like.batch_transaction_get_raw(&txids).unwrap_or_default();
                            for (idx, txid) in txids.iter().enumerate() {
                                let raw = raw_txs.get(idx).cloned().unwrap_or_default();
                                if raw.is_empty() {
                                    continue;
                                }
                                let tx: Transaction = match deserialize(&raw) {
                                    Ok(value) => value,
                                    Err(e) => {
                                        eprintln!(
                                            "[rpc_get_address_transactions] failed to decode tx {}: {e}",
                                            txid
                                        );
                                        continue;
                                    }
                                };
                                let summary = summary_vals
                                    .get(idx)
                                    .and_then(|v| v.as_ref())
                                    .and_then(|b| AlkaneTxSummary::try_from_slice(b).ok());
                                let confirmations = entries_for_page[idx].height.and_then(|h| {
                                    chain_tip.and_then(|tip| {
                                        if tip >= h { Some(tip - h + 1) } else { None }
                                    })
                                });
                                let traces = summary
                                    .as_ref()
                                    .map(|s| traces_from_summary(txid, s))
                                    .filter(|v| !v.is_empty());
                                tx_renders.push(AddressTxRender {
                                    txid: *txid,
                                    tx,
                                    traces,
                                    confirmations,
                                    is_mempool: false,
                                    summary,
                                });
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!(
                        "[rpc_get_address_transactions] failed to fetch history for {}: {e}",
                        address
                    );
                }
            }
        }
        let tx_total = pending_total + confirmed_total;
        let mut prev_txids: Vec<Txid> = Vec::new();
        for render in &tx_renders {
            for vin in &render.tx.input {
                if !vin.previous_output.is_null() {
                    prev_txids.push(vin.previous_output.txid);
                }
            }
        }
        prev_txids.sort();
        prev_txids.dedup();
        let mut prev_map: HashMap<Txid, Transaction> = HashMap::new();
        if !prev_txids.is_empty() {
            let raw_prev = electrum_like.batch_transaction_get_raw(&prev_txids).unwrap_or_default();
            for (i, raw) in raw_prev.into_iter().enumerate() {
                if raw.is_empty() {
                    if let Some(mempool_prev) = pending_by_txid(&prev_txids[i]) {
                        prev_map.insert(prev_txids[i], mempool_prev.tx);
                    }
                    continue;
                }
                if let Ok(prev_tx) = deserialize::<Transaction>(&raw) {
                    prev_map.insert(prev_txids[i], prev_tx);
                } else if let Some(mempool_prev) = pending_by_txid(&prev_txids[i]) {
                    prev_map.insert(prev_txids[i], mempool_prev.tx);
                }
            }
        }

        let transactions: Vec<Value> = tx_renders
            .iter()
            .map(|render| enriched_transaction_json(render, &prev_map, network))
            .collect();

        Ok(RpcGetAddressTransactionsResult {
            value: json!({
                "ok": true,
                "address": address,
                "page": page,
                "limit": limit,
                "total": tx_total,
                "has_more": (off + tx_renders.len()) < tx_total,
                "transactions": transactions,
            }),
        })
    }

    pub fn rpc_get_alkane_latest_traces(
        &self,
        _params: RpcGetAlkaneLatestTracesParams,
    ) -> Result<RpcGetAlkaneLatestTracesResult> {
        let table = self.table();
        let list: Vec<[u8; 32]> = self
            .get_raw_value(GetRawValueParams { key: table.alkane_latest_traces_key() })
            .ok()
            .and_then(|resp| resp.value)
            .and_then(|b| Vec::<[u8; 32]>::try_from_slice(&b).ok())
            .unwrap_or_default();
        let txids: Vec<String> = list
            .into_iter()
            .filter_map(|b| Txid::from_slice(&b).ok())
            .map(|t| t.to_string())
            .collect();

        Ok(RpcGetAlkaneLatestTracesResult {
            value: json!({
                "ok": true,
                "txids": txids
            }),
        })
    }

    pub fn rpc_ping(&self, _params: RpcPingParams) -> Result<RpcPingResult> {
        Ok(RpcPingResult { value: Value::String("pong".to_string()) })
    }
}

pub struct GetRawValueParams {
    pub key: Vec<u8>,
}

pub struct GetRawValueResult {
    pub value: Option<Vec<u8>>,
}

pub struct GetMultiValuesParams {
    pub keys: Vec<Vec<u8>>,
}

pub struct GetMultiValuesResult {
    pub values: Vec<Option<Vec<u8>>>,
}

pub struct GetScanPrefixParams {
    pub prefix: Vec<u8>,
}

pub struct GetScanPrefixResult {
    pub keys: Vec<Vec<u8>>,
}

pub struct GetIterPrefixRevParams {
    pub prefix: Vec<u8>,
}

pub struct GetIterPrefixRevResult {
    pub entries: Vec<(Vec<u8>, Vec<u8>)>,
}

pub struct GetIterFromParams {
    pub start: Vec<u8>,
}

pub struct GetIterFromResult {
    pub entries: Vec<(Vec<u8>, Vec<u8>)>,
}

pub struct SetRawValueParams {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

pub struct SetBatchParams {
    pub puts: Vec<(Vec<u8>, Vec<u8>)>,
    pub deletes: Vec<Vec<u8>>,
}

pub struct GetIndexHeightParams;

pub struct GetIndexHeightResult {
    pub height: Option<u32>,
}

pub struct SetIndexHeightParams {
    pub height: u32,
}

pub struct GetCreationRecordParams {
    pub alkane: SchemaAlkaneId,
}

pub struct GetCreationRecordResult {
    pub record: Option<AlkaneCreationRecord>,
}

pub struct GetCreationRecordsByIdParams {
    pub alkanes: Vec<SchemaAlkaneId>,
}

pub struct GetCreationRecordsByIdResult {
    pub records: Vec<Option<AlkaneCreationRecord>>,
}

pub struct GetCreationRecordsOrderedParams;

pub struct GetCreationRecordsOrderedResult {
    pub records: Vec<AlkaneCreationRecord>,
}

pub struct GetCreationRecordsOrderedPageParams {
    pub offset: u64,
    pub limit: u64,
    pub desc: bool,
}

pub struct GetCreationRecordsOrderedPageResult {
    pub records: Vec<AlkaneCreationRecord>,
}

pub struct GetAlkaneIdsByNamePrefixParams {
    pub prefix: String,
}

pub struct GetAlkaneIdsByNamePrefixResult {
    pub ids: Vec<SchemaAlkaneId>,
}

pub struct GetAlkaneIdsByNamePrefixPageParams {
    pub prefix: String,
    pub offset: u64,
    pub limit: u64,
}

pub struct GetAlkaneIdsBySymbolPrefixParams {
    pub prefix: String,
}

pub struct GetAlkaneIdsBySymbolPrefixResult {
    pub ids: Vec<SchemaAlkaneId>,
}

pub struct GetAlkaneIdsBySymbolPrefixPageParams {
    pub prefix: String,
    pub offset: u64,
    pub limit: u64,
}

pub struct GetCreationCountParams;

pub struct GetCreationCountResult {
    pub count: u64,
}

pub struct GetHoldersCountParams {
    pub alkane: SchemaAlkaneId,
}

pub struct GetHoldersCountResult {
    pub count: u64,
}

pub struct GetHoldersCountsByIdParams {
    pub alkanes: Vec<SchemaAlkaneId>,
}

pub struct GetHoldersCountsByIdResult {
    pub counts: Vec<u64>,
}

pub struct GetHoldersOrderedPageParams {
    pub offset: u64,
    pub limit: u64,
    pub desc: bool,
}

pub struct GetHoldersOrderedPageResult {
    pub ids: Vec<SchemaAlkaneId>,
}

pub struct GetCirculatingSupplyParams {
    pub alkane: SchemaAlkaneId,
    pub height: u32,
}

pub struct GetCirculatingSupplyResult {
    pub supply: u128,
}

pub struct GetLatestCirculatingSupplyParams {
    pub alkane: SchemaAlkaneId,
}

pub struct GetLatestCirculatingSupplyResult {
    pub supply: u128,
}

pub struct GetLatestTotalMintedParams {
    pub alkane: SchemaAlkaneId,
}

pub struct GetLatestTotalMintedResult {
    pub total_minted: u128,
}

pub struct GetAlkaneStorageValueParams {
    pub alkane: SchemaAlkaneId,
    pub key: Vec<u8>,
}

pub struct GetAlkaneStorageValueResult {
    pub value: Option<Vec<u8>>,
}

pub struct GetBlockSummaryParams {
    pub height: u32,
}

pub struct GetBlockSummaryResult {
    pub summary: Option<BlockSummary>,
}

pub struct GetMempoolSeenPageParams {
    pub page: usize,
    pub limit: usize,
}

pub struct GetMempoolSeenPageResult {
    pub txids: Vec<Txid>,
    pub has_more: bool,
}

pub struct GetMempoolEntryParams {
    pub txid: Txid,
}

pub struct GetMempoolEntryResult {
    pub entry: Option<MempoolEntry>,
}

pub struct GetMempoolPendingForAddressParams {
    pub address: String,
}

pub struct GetMempoolPendingForAddressResult {
    pub entries: Vec<MempoolEntry>,
}

pub struct RpcGetMempoolTracesParams {
    pub page: Option<u64>,
    pub limit: Option<u64>,
    pub address: Option<String>,
}

pub struct RpcGetMempoolTracesResult {
    pub value: Value,
}

pub struct RpcGetKeysParams {
    pub alkane: Option<String>,
    pub try_decode_utf8: Option<bool>,
    pub limit: Option<u64>,
    pub page: Option<u64>,
    pub keys: Option<Vec<String>>,
}

pub struct RpcGetKeysResult {
    pub value: Value,
}

pub struct RpcGetAllAlkanesParams {
    pub page: Option<u64>,
    pub limit: Option<u64>,
}

pub struct RpcGetAllAlkanesResult {
    pub value: Value,
}

pub struct RpcGetAlkaneInfoParams {
    pub alkane: Option<String>,
}

pub struct RpcGetAlkaneInfoResult {
    pub value: Value,
}

pub struct RpcGetBlockSummaryParams {
    pub height: Option<u64>,
}

pub struct RpcGetBlockSummaryResult {
    pub value: Value,
}

pub struct RpcGetHoldersParams {
    pub alkane: Option<String>,
    pub page: Option<u64>,
    pub limit: Option<u64>,
}

pub struct RpcGetHoldersResult {
    pub value: Value,
}

pub struct RpcGetTransferVolumeParams {
    pub alkane: Option<String>,
    pub page: Option<u64>,
    pub limit: Option<u64>,
}

pub struct RpcGetTransferVolumeResult {
    pub value: Value,
}

pub struct RpcGetTotalReceivedParams {
    pub alkane: Option<String>,
    pub page: Option<u64>,
    pub limit: Option<u64>,
}

pub struct RpcGetTotalReceivedResult {
    pub value: Value,
}

pub struct RpcGetCirculatingSupplyParams {
    pub alkane: Option<String>,
    pub height: Option<u64>,
    pub height_present: bool,
}

pub struct RpcGetCirculatingSupplyResult {
    pub value: Value,
}

pub struct RpcGetAddressActivityParams {
    pub address: Option<String>,
}

pub struct RpcGetAddressActivityResult {
    pub value: Value,
}

pub struct RpcGetAddressBalancesParams {
    pub address: Option<String>,
    pub include_outpoints: Option<bool>,
}

pub struct RpcGetAddressBalancesResult {
    pub value: Value,
}

pub struct RpcGetAlkaneBalancesParams {
    pub alkane: Option<String>,
    pub height: Option<u64>,
    pub height_present: bool,
}

pub struct RpcGetAlkaneBalancesResult {
    pub value: Value,
}

pub struct RpcGetAlkaneBalanceMetashrewParams {
    pub owner: Option<String>,
    pub target: Option<String>,
    pub height: Option<u64>,
    pub height_present: bool,
}

pub struct RpcGetAlkaneBalanceMetashrewResult {
    pub value: Value,
}

pub struct RpcGetAlkaneBalanceTxsParams {
    pub alkane: Option<String>,
    pub page: Option<u64>,
    pub limit: Option<u64>,
}

pub struct RpcGetAlkaneBalanceTxsResult {
    pub value: Value,
}

pub struct RpcGetAlkaneBalanceTxsByTokenParams {
    pub owner: Option<String>,
    pub token: Option<String>,
    pub page: Option<u64>,
    pub limit: Option<u64>,
}

pub struct RpcGetAlkaneBalanceTxsByTokenResult {
    pub value: Value,
}

pub struct RpcGetOutpointBalancesParams {
    pub outpoint: Option<String>,
}

pub struct RpcGetOutpointBalancesResult {
    pub value: Value,
}

pub struct RpcGetBlockTracesParams {
    pub height: Option<u64>,
}

pub struct RpcGetBlockTracesResult {
    pub value: Value,
}

pub struct RpcGetHoldersCountParams {
    pub alkane: Option<String>,
}

pub struct RpcGetHoldersCountResult {
    pub value: Value,
}

pub struct RpcGetAddressOutpointsParams {
    pub address: Option<String>,
}

pub struct RpcGetAddressOutpointsResult {
    pub value: Value,
}

pub struct RpcGetAlkaneTxSummaryParams {
    pub txid: Option<String>,
}

pub struct RpcGetAlkaneTxSummaryResult {
    pub value: Value,
}

pub struct RpcGetAlkaneBlockTxsParams {
    pub height: Option<u64>,
    pub page: Option<u64>,
    pub limit: Option<u64>,
}

pub struct RpcGetAlkaneBlockTxsResult {
    pub value: Value,
}

pub struct RpcGetAlkaneAddressTxsParams {
    pub address: Option<String>,
    pub page: Option<u64>,
    pub limit: Option<u64>,
}

pub struct RpcGetAlkaneAddressTxsResult {
    pub value: Value,
}

pub struct RpcGetAddressTransactionsParams {
    pub address: Option<String>,
    pub page: Option<u64>,
    pub limit: Option<u64>,
    pub only_alkane_txs: Option<bool>,
}

pub struct RpcGetAddressTransactionsResult {
    pub value: Value,
}

pub struct RpcGetAlkaneLatestTracesParams;

pub struct RpcGetAlkaneLatestTracesResult {
    pub value: Value,
}

pub struct RpcPingParams;

pub struct RpcPingResult {
    pub value: Value,
}

/// Identifier for a holder: either a Bitcoin address or another Alkane.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize)]
pub enum HolderId {
    Address(String),
    Alkane(SchemaAlkaneId),
}

/// Entry in holders index (holder id + amount for one alkane)
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct HolderEntry {
    pub holder: HolderId,
    pub amount: u128,
}

/// Entry in per-alkane address activity indexes.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct AddressAmountEntry {
    pub address: String,
    pub amount: u128,
}

/// Per-address activity summary across alkanes.
#[derive(Clone, Debug, Default, BorshSerialize, BorshDeserialize)]
pub struct AddressActivityEntry {
    pub transfer_volume: BTreeMap<SchemaAlkaneId, u128>,
    pub total_received: BTreeMap<SchemaAlkaneId, u128>,
}

/// One alkane balance record inside a single outpoint (BORSH)
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct BalanceEntry {
    pub alkane: SchemaAlkaneId,
    pub amount: u128,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct AlkaneBalanceTxEntry {
    pub txid: [u8; 32],
    pub height: u32,
    pub outflow: BTreeMap<SchemaAlkaneId, SignedU128>,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct AlkaneTxSummary {
    pub txid: [u8; 32],
    pub traces: Vec<EspoSandshrewLikeTrace>,
    pub outflows: Vec<AlkaneBalanceTxEntry>,
    pub height: u32,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct HoldersCountEntry {
    pub count: u64,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct BlockSummary {
    pub trace_count: u32,
    pub header: Vec<u8>,
}

const BLOCK_SUMMARY_CACHE_CAP: usize = 100;

struct BlockSummaryCache {
    order: VecDeque<u32>,
    map: HashMap<u32, BlockSummary>,
}

impl BlockSummaryCache {
    fn insert(&mut self, height: u32, summary: BlockSummary) {
        if self.map.contains_key(&height) {
            self.order.retain(|h| *h != height);
        }
        self.map.insert(height, summary);
        self.order.push_back(height);
        while self.order.len() > BLOCK_SUMMARY_CACHE_CAP {
            if let Some(oldest) = self.order.pop_front() {
                self.map.remove(&oldest);
            }
        }
    }

    fn get(&self, height: u32) -> Option<BlockSummary> {
        self.map.get(&height).cloned()
    }
}

static BLOCK_SUMMARY_CACHE: OnceLock<Arc<RwLock<BlockSummaryCache>>> = OnceLock::new();

fn block_summary_cache() -> &'static Arc<RwLock<BlockSummaryCache>> {
    BLOCK_SUMMARY_CACHE.get_or_init(|| {
        Arc::new(RwLock::new(BlockSummaryCache { order: VecDeque::new(), map: HashMap::new() }))
    })
}

pub fn cache_block_summary(height: u32, summary: BlockSummary) {
    if let Ok(mut cache) = block_summary_cache().write() {
        cache.insert(height, summary);
    }
}

pub fn get_cached_block_summary(height: u32) -> Option<BlockSummary> {
    crate::debug_timer_log!("get_cached_block_summary");
    block_summary_cache().read().ok().and_then(|cache| cache.get(height))
}

pub fn preload_block_summary_cache(mdb: &Mdb) -> usize {
    let table = EssentialsTable::new(mdb);
    let prefix = table.block_summary_prefix();
    let prefix_full = mdb.prefixed(&prefix);
    let mut loaded = 0usize;

    for res in mdb.iter_prefix_rev(&prefix_full) {
        if loaded >= BLOCK_SUMMARY_CACHE_CAP {
            break;
        }
        let Ok((k, v)) = res else { continue };
        let rel = &k[mdb.prefix().len()..];
        if !rel.starts_with(&prefix) {
            break;
        }
        let height_bytes = &rel[prefix.len()..];
        if height_bytes.len() != 4 {
            continue;
        }
        let mut arr = [0u8; 4];
        arr.copy_from_slice(height_bytes);
        let height = u32::from_be_bytes(arr);
        if let Ok(summary) = BlockSummary::try_from_slice(&v) {
            cache_block_summary(height, summary);
            loaded += 1;
        }
    }

    loaded
}

/// Creation metadata for an alkane.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AlkaneInfo {
    pub creation_txid: [u8; 32],
    pub creation_height: u32,
    pub creation_timestamp: u32,
}

pub const BALANCE_TXS_PAGE_SIZE: u32 = 2048;

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AlkaneBalanceTxsMeta {
    pub page_size: u32,
    pub last_page: u64,
    pub total_len: u64,
}
#[derive(BorshSerialize)]
struct OutpointPrefix {
    txid: Vec<u8>,
    vout: u32,
}

/// Helper to build an outpoint with optional spending txid for lookups.
pub fn mk_outpoint(txid: Vec<u8>, vout: u32, tx_spent: Option<Vec<u8>>) -> EspoOutpoint {
    EspoOutpoint { txid, vout, tx_spent }
}

pub fn spk_to_address_str(spk: &ScriptBuf, net: Network) -> Option<String> {
    Address::from_script(spk.as_script(), net).ok().map(|a| a.to_string())
}

pub fn encode_vec<T: BorshSerialize>(v: &Vec<T>) -> Result<Vec<u8>> {
    Ok(borsh::to_vec(v)?)
}

pub fn decode_balances_vec(bytes: &[u8]) -> Result<Vec<BalanceEntry>> {
    Ok(Vec::<BalanceEntry>::try_from_slice(bytes)?)
}

pub fn decode_alkane_balance_tx_entries(bytes: &[u8]) -> Result<Vec<AlkaneBalanceTxEntry>> {
    if let Ok(parsed) = Vec::<AlkaneBalanceTxEntry>::try_from_slice(bytes) {
        return Ok(parsed);
    }

    #[derive(BorshDeserialize)]
    struct LegacyAlkaneBalanceTxEntry {
        txid: [u8; 32],
        outflow: BTreeMap<SchemaAlkaneId, SignedU128>,
    }

    if let Ok(legacy) = Vec::<LegacyAlkaneBalanceTxEntry>::try_from_slice(bytes) {
        return Ok(legacy
            .into_iter()
            .map(|entry| AlkaneBalanceTxEntry {
                txid: entry.txid,
                height: 0,
                outflow: entry.outflow,
            })
            .collect());
    }

    let legacy: Vec<[u8; 32]> = Vec::<[u8; 32]>::try_from_slice(bytes)?;
    Ok(legacy
        .into_iter()
        .map(|txid| AlkaneBalanceTxEntry { txid, height: 0, outflow: BTreeMap::new() })
        .collect())
}

pub fn decode_holders_vec(bytes: &[u8]) -> Result<Vec<HolderEntry>> {
    if let Ok(parsed) = Vec::<HolderEntry>::try_from_slice(bytes) {
        return Ok(parsed);
    }

    #[derive(BorshDeserialize)]
    struct LegacyHolderEntry {
        address: String,
        amount: u128,
    }

    let legacy: Vec<LegacyHolderEntry> = Vec::<LegacyHolderEntry>::try_from_slice(bytes)?;
    Ok(legacy
        .into_iter()
        .map(|h| HolderEntry { holder: HolderId::Address(h.address), amount: h.amount })
        .collect())
}

pub fn decode_address_amount_vec(bytes: &[u8]) -> Result<Vec<AddressAmountEntry>> {
    Ok(Vec::<AddressAmountEntry>::try_from_slice(bytes)?)
}

pub fn encode_address_amount_vec(entries: &Vec<AddressAmountEntry>) -> Result<Vec<u8>> {
    encode_vec(entries)
}

pub fn decode_address_activity_entry(bytes: &[u8]) -> Result<AddressActivityEntry> {
    Ok(AddressActivityEntry::try_from_slice(bytes)?)
}

pub fn encode_address_activity_entry(entry: &AddressActivityEntry) -> Result<Vec<u8>> {
    Ok(borsh::to_vec(entry)?)
}

pub fn encode_alkane_info(info: &AlkaneInfo) -> Result<Vec<u8>> {
    Ok(borsh::to_vec(info)?)
}

pub fn decode_alkane_info(bytes: &[u8]) -> Result<AlkaneInfo> {
    Ok(AlkaneInfo::try_from_slice(bytes)?)
}

pub fn encode_u128_value(value: u128) -> Result<Vec<u8>> {
    Ok(borsh::to_vec(&value)?)
}

pub fn decode_u128_value(bytes: &[u8]) -> Result<u128> {
    Ok(u128::try_from_slice(bytes)?)
}

pub fn encode_creation_record(record: &AlkaneCreationRecord) -> Result<Vec<u8>> {
    Ok(borsh::to_vec(record)?)
}

pub fn decode_creation_record(bytes: &[u8]) -> Result<AlkaneCreationRecord> {
    // Try new schema first; fall back to legacy Option name/symbol layout.
    if let Ok(rec) = AlkaneCreationRecord::try_from_slice(bytes) {
        return Ok(rec);
    }

    #[derive(BorshDeserialize)]
    struct LegacyCreationRecordV2 {
        alkane: SchemaAlkaneId,
        txid: [u8; 32],
        creation_height: u32,
        creation_timestamp: u32,
        tx_index_in_block: u32,
        inspection: Option<crate::modules::essentials::utils::inspections::StoredInspectionResult>,
        names: Vec<String>,
        symbols: Vec<String>,
    }

    if let Ok(legacy) = LegacyCreationRecordV2::try_from_slice(bytes) {
        return Ok(AlkaneCreationRecord {
            alkane: legacy.alkane,
            txid: legacy.txid,
            creation_height: legacy.creation_height,
            creation_timestamp: legacy.creation_timestamp,
            tx_index_in_block: legacy.tx_index_in_block,
            inspection: legacy.inspection,
            names: legacy.names,
            symbols: legacy.symbols,
            cap: 0,
            mint_amount: 0,
        });
    }

    #[derive(BorshDeserialize)]
    struct LegacyCreationRecord {
        alkane: SchemaAlkaneId,
        txid: [u8; 32],
        creation_height: u32,
        creation_timestamp: u32,
        tx_index_in_block: u32,
        inspection: Option<crate::modules::essentials::utils::inspections::StoredInspectionResult>,
        name: Option<String>,
        symbol: Option<String>,
    }

    let legacy = LegacyCreationRecord::try_from_slice(bytes)?;
    let mut names = Vec::new();
    let mut symbols = Vec::new();
    if let Some(n) = legacy.name {
        names.push(n);
    }
    if let Some(s) = legacy.symbol {
        symbols.push(s);
    }
    Ok(AlkaneCreationRecord {
        alkane: legacy.alkane,
        txid: legacy.txid,
        creation_height: legacy.creation_height,
        creation_timestamp: legacy.creation_timestamp,
        tx_index_in_block: legacy.tx_index_in_block,
        inspection: legacy.inspection,
        names,
        symbols,
        cap: 0,
        mint_amount: 0,
    })
}

pub fn load_creation_record(
    mdb: &crate::runtime::mdb::Mdb,
    alkane: &SchemaAlkaneId,
) -> Result<Option<AlkaneCreationRecord>> {
    let table = EssentialsTable::new(mdb);
    let key = table.alkane_creation_by_id_key(alkane);
    if let Some(bytes) = mdb.get(&key)? {
        let record = decode_creation_record(&bytes)?;
        Ok(Some(record))
    } else {
        Ok(None)
    }
}

pub fn get_holders_count_encoded(count: u64) -> Result<Vec<u8>> {
    crate::debug_timer_log!("get_holders_count_encoded");
    let count_value = HoldersCountEntry { count };

    Ok(borsh::to_vec(&count_value)?)
}

pub fn get_holders_values_encoded(holders: Vec<HolderEntry>) -> Result<(Vec<u8>, Vec<u8>)> {
    crate::debug_timer_log!("get_holders_values_encoded");
    Ok((encode_vec(&holders)?, get_holders_count_encoded(holders.len().try_into()?)?))
}

/// Build the key for alkane balances (public helper for strict mode validation)
pub fn build_alkane_balances_key(owner: &SchemaAlkaneId) -> Vec<u8> {
    let mut key = b"/alkane_balances/".to_vec();
    key.extend_from_slice(&owner.block.to_be_bytes());
    key.extend_from_slice(&owner.tx.to_be_bytes());
    key
}

fn mem_entry_to_json(entry: &MempoolEntry) -> Value {
    let mut traces_json: Vec<Value> = Vec::new();
    if let Some(traces) = entry.traces.as_ref() {
        for t in traces {
            let events_val = prettyify_protobuf_trace_json(&t.protobuf_trace)
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .unwrap_or(Value::Null);
            traces_json.push(json!({
                "outpoint": format!("{}:{}", entry.txid, t.outpoint.vout),
                "events": events_val,
            }));
        }
    }

    json!({
        "txid": entry.txid.to_string(),
        "first_seen": entry.first_seen,
        "traces": traces_json,
    })
}

fn normalize_address(s: &str) -> Option<String> {
    let network = get_network();
    Address::from_str(s)
        .ok()
        .and_then(|a| a.require_network(network).ok())
        .map(|a| a.to_string())
}

struct AddressTxRender {
    txid: Txid,
    tx: Transaction,
    traces: Option<Vec<EspoTrace>>,
    confirmations: Option<u64>,
    is_mempool: bool,
    summary: Option<AlkaneTxSummary>,
}

fn enriched_transaction_json(
    render: &AddressTxRender,
    prev_map: &HashMap<Txid, Transaction>,
    network: Network,
) -> Value {
    let tx = &render.tx;
    let mut input_sum: u64 = 0;
    let mut inputs: Vec<Value> = Vec::new();

    for vin in &tx.input {
        let mut obj = Map::new();
        obj.insert("txid".to_string(), json!(vin.previous_output.txid.to_string()));
        obj.insert("vout".to_string(), json!(vin.previous_output.vout));
        if vin.previous_output.is_null() {
            obj.insert("isCoinbase".to_string(), json!(true));
        } else if let Some(prev_tx) = prev_map.get(&vin.previous_output.txid) {
            if let Some(prev_out) = prev_tx.output.get(vin.previous_output.vout as usize) {
                input_sum = input_sum.saturating_add(prev_out.value.to_sat());
                obj.insert("amount".to_string(), json!(prev_out.value.to_sat()));
                if let Ok(addr) = Address::from_script(prev_out.script_pubkey.as_script(), network)
                {
                    obj.insert("address".to_string(), json!(addr.to_string()));
                }
            }
        }
        inputs.push(Value::Object(obj));
    }

    let mut output_sum: u64 = 0;
    let mut outputs: Vec<Value> = Vec::new();
    for out in &tx.output {
        let mut obj = Map::new();
        obj.insert("amount".to_string(), json!(out.value.to_sat()));
        obj.insert("scriptPubKey".to_string(), json!(hex::encode(out.script_pubkey.as_bytes())));
        if let Ok(addr) = Address::from_script(out.script_pubkey.as_script(), network) {
            obj.insert("address".to_string(), json!(addr.to_string()));
        }
        if let Some(script_type) = script_type_label(&out.script_pubkey, network) {
            obj.insert("scriptPubKeyType".to_string(), json!(script_type));
        }
        outputs.push(Value::Object(obj));
        output_sum = output_sum.saturating_add(out.value.to_sat());
    }

    let fee = if tx.is_coinbase() || input_sum < output_sum {
        None
    } else {
        Some(input_sum - output_sum)
    };
    let (runestone, protostones) = runestone_data(tx);
    let has_protostones = !protostones.is_empty();
    let alkanes_traces = render.traces.as_ref().and_then(|traces| {
        let vals = traces.iter().map(enriched_trace_to_value).collect::<Vec<_>>();
        if vals.is_empty() { None } else { Some(Value::Array(vals)) }
    });

    let mut out = Map::new();
    out.insert("txid".to_string(), json!(render.txid.to_string()));
    out.insert("blockHeight".to_string(), json!(render.summary.as_ref().map(|s| s.height as u64)));
    out.insert("confirmations".to_string(), json!(render.confirmations));
    out.insert("blockTime".to_string(), Value::Null);
    out.insert("confirmed".to_string(), json!(!render.is_mempool));
    out.insert("fee".to_string(), fee.map(|value| json!(value)).unwrap_or(Value::Null));
    out.insert("weight".to_string(), json!(tx.weight().to_wu()));
    out.insert("size".to_string(), json!(serialize(tx).len() as u64));
    out.insert("inputs".to_string(), Value::Array(inputs));
    out.insert("outputs".to_string(), Value::Array(outputs));
    out.insert("hasOpReturn".to_string(), json!(tx_has_op_return(tx)));
    out.insert("hasProtostones".to_string(), json!(has_protostones));
    out.insert("isRbf".to_string(), json!(tx.is_explicitly_rbf()));
    out.insert("isCoinbase".to_string(), json!(tx.is_coinbase()));
    if let Some(runestone) = runestone {
        out.insert("runestone".to_string(), runestone);
    }
    if let Some(alkane_traces) = alkanes_traces {
        out.insert("alkanesTraces".to_string(), alkane_traces);
    }

    Value::Object(out)
}

fn runestone_data(tx: &Transaction) -> (Option<Value>, Vec<Value>) {
    if let Some(Artifact::Runestone(runestone)) = Runestone::decipher(tx) {
        let protostones = Protostone::from_runestone(&runestone).unwrap_or_default();
        let protos_json = protostones.iter().map(protostone_to_value).collect::<Vec<_>>();
        if let Value::Object(mut map) = serde_json::to_value(&runestone).unwrap_or(Value::Null) {
            map.insert("protostones".to_string(), Value::Array(protos_json.clone()));
            return (Some(Value::Object(map)), protos_json);
        }
        let mut map = Map::new();
        map.insert("protostones".to_string(), Value::Array(protos_json.clone()));
        return (Some(Value::Object(map)), protos_json);
    }
    (None, Vec::new())
}

fn protostone_to_value(protostone: &Protostone) -> Value {
    let edicts = protostone
        .edicts
        .iter()
        .map(|edict| {
            json!({
                "id": {
                    "block": edict.id.block,
                    "tx": edict.id.tx,
                },
                "amount": edict.amount,
                "output": edict.output,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "burn": protostone.burn,
        "message": hex::encode(&protostone.message),
        "edicts": edicts,
        "refund": protostone.refund,
        "pointer": protostone.pointer,
        "from": protostone.from,
        "protocol_tag": protostone.protocol_tag,
    })
}

fn enriched_trace_to_value(trace: &EspoTrace) -> Value {
    let txid = Txid::from_slice(&trace.outpoint.txid)
        .map(|t| t.to_string())
        .unwrap_or_default();
    let protostone_index = trace
        .sandshrew_trace
        .events
        .iter()
        .filter_map(|event| match event {
            EspoSandshrewLikeTraceEvent::Invoke(inv) => Some(inv.context.vout),
            _ => None,
        })
        .next()
        .unwrap_or(0);
    let trace_events = if trace.protobuf_trace.events.is_empty() {
        serde_json::to_value(&trace.sandshrew_trace.events).unwrap_or(Value::Null)
    } else {
        serde_json::to_value(&trace.protobuf_trace.events).unwrap_or(Value::Null)
    };
    json!({
        "vout": trace.outpoint.vout,
        "outpoint": format!("{txid}:{}", trace.outpoint.vout),
        "protostone_index": protostone_index,
        "trace": {
            "events": trace_events,
        },
    })
}

fn script_type_label(spk: &ScriptBuf, network: Network) -> Option<&'static str> {
    Address::from_script(spk.as_script(), network)
        .ok()
        .and_then(|a| match a.address_type() {
            Some(AddressType::P2pkh) => Some("P2PKH"),
            Some(AddressType::P2sh) => Some("P2SH"),
            Some(AddressType::P2wpkh) => Some("P2WPKH"),
            Some(AddressType::P2wsh) => Some("P2WSH"),
            Some(AddressType::P2tr) => Some("P2TR"),
            _ => None,
        })
}

fn tx_has_op_return(tx: &Transaction) -> bool {
    tx.output.iter().any(|out| {
        let bytes = out.script_pubkey.as_bytes();
        !bytes.is_empty() && bytes[0] == bitcoin::opcodes::all::OP_RETURN.to_u8()
    })
}

fn traces_from_summary(txid: &Txid, summary: &AlkaneTxSummary) -> Vec<EspoTrace> {
    summary
        .traces
        .iter()
        .filter_map(|trace| sandshrew_to_espo_trace(txid, trace))
        .collect()
}

fn sandshrew_to_espo_trace(txid: &Txid, trace: &EspoSandshrewLikeTrace) -> Option<EspoTrace> {
    let (txid_hex, vout_s) = trace.outpoint.split_once(':')?;
    let vout = vout_s.parse::<u32>().ok()?;
    let trace_txid = Txid::from_str(txid_hex).unwrap_or(*txid);
    Some(EspoTrace {
        sandshrew_trace: trace.clone(),
        protobuf_trace: AlkanesTrace::default(),
        storage_changes: HashMap::new(),
        outpoint: EspoOutpoint { txid: trace_txid.to_byte_array().to_vec(), vout, tx_spent: None },
    })
}

fn parse_alkane_from_str(s: &str) -> Option<SchemaAlkaneId> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let parse_u32 = |t: &str| {
        if let Some(x) = t.strip_prefix("0x") {
            u32::from_str_radix(x, 16).ok()
        } else {
            t.parse::<u32>().ok()
        }
    };
    let parse_u64 = |t: &str| {
        if let Some(x) = t.strip_prefix("0x") {
            u64::from_str_radix(x, 16).ok()
        } else {
            t.parse::<u64>().ok()
        }
    };
    Some(SchemaAlkaneId { block: parse_u32(parts[0])?, tx: parse_u64(parts[1])? })
}

fn parse_key_str_to_bytes(s: &str) -> Option<Vec<u8>> {
    if let Some(hex) = s.strip_prefix("0x") {
        if hex.len() % 2 == 0 && !hex.is_empty() {
            return hex::decode(hex).ok();
        }
    }
    Some(s.as_bytes().to_vec())
}

fn dedup_sort_keys(mut v: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    v.sort();
    v.dedup();
    v
}

fn parse_outpoint_str(s: &str) -> std::result::Result<(Txid, u32), Value> {
    let (txid_hex, vout_str) = match s.split_once(':') {
        Some(parts) => parts,
        None => {
            return Err(json!({
                "ok": false,
                "error": "invalid_outpoint_format",
                "hint": "expected \"<txid>:<vout>\""
            }));
        }
    };
    let txid = match Txid::from_str(txid_hex) {
        Ok(t) => t,
        Err(_) => {
            return Err(json!({"ok": false, "error": "invalid_txid"}));
        }
    };
    let vout_u32 = match vout_str.parse::<u32>() {
        Ok(n) => n,
        Err(_) => {
            return Err(json!({"ok": false, "error": "invalid_vout"}));
        }
    };
    Ok((txid, vout_u32))
}

/// Split the stored value row into `(last_txid_be_hex, raw_value_bytes)`.
/// First 32 bytes = txid in LE; we flip to BE for explorers.
/// Returns (Some("deadbeef…"), tail) or (None, whole) if no txid present.
fn split_txid_value(v: &[u8]) -> (Option<String>, &[u8]) {
    if v.len() >= 32 {
        let txid_le = &v[..32];
        let mut txid_be = txid_le.to_vec();
        txid_be.reverse();
        (Some(fmt_bytes_hex_noprefix(&txid_be)), &v[32..])
    } else {
        (None, v)
    }
}

fn fmt_bytes_hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(2 + b.len() * 2);
    s.push_str("0x");
    for byte in b {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", byte);
    }
    s
}

fn fmt_bytes_hex_noprefix(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for byte in b {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", byte);
    }
    s
}

fn utf8_or_null(b: &[u8]) -> Value {
    match std::str::from_utf8(b) {
        Ok(s) => Value::String(s.to_string()),
        Err(_) => Value::Null,
    }
}

fn u128_le_or_null(b: &[u8]) -> Value {
    if b.len() > 16 {
        return Value::Null;
    }
    let mut acc: u128 = 0;
    for (i, &byte) in b.iter().enumerate() {
        acc |= (byte as u128) << (i * 8);
    }
    Value::String(acc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alkane_info_round_trip() {
        let info = AlkaneInfo {
            creation_txid: [7u8; 32],
            creation_height: 42,
            creation_timestamp: 1_700_000_000,
        };

        let encoded = encode_alkane_info(&info).expect("encode");
        let decoded = decode_alkane_info(&encoded).expect("decode");
        assert_eq!(info, decoded);
    }

    #[test]
    fn creation_record_round_trip() {
        let rec = AlkaneCreationRecord {
            alkane: SchemaAlkaneId { block: 5, tx: 10 },
            txid: [9u8; 32],
            creation_height: 123,
            creation_timestamp: 99,
            tx_index_in_block: 3,
            inspection: None,
            names: vec!["demo".to_string(), "demo2".to_string()],
            symbols: vec!["DMO".to_string()],
            cap: 500,
            mint_amount: 25,
        };

        let encoded = encode_creation_record(&rec).expect("encode");
        let decoded = decode_creation_record(&encoded).expect("decode");
        assert_eq!(rec, decoded);
    }
}
