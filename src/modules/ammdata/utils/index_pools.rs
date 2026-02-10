use crate::alkanes::trace::{EspoBlock, EspoSandshrewLikeTraceEvent};
use crate::config::get_electrum_like;
use crate::modules::ammdata::consts::CanonicalQuoteUnit;
use crate::modules::ammdata::schemas::{
    ActivityKind, SchemaActivityV1, SchemaCanonicalPoolEntry, SchemaPoolCreationInfoV1,
};
use crate::modules::ammdata::storage::{
    AmmDataProvider, encode_pool_creation_info, encode_u128_value,
};
use crate::modules::ammdata::utils::index_state::IndexState;
use crate::modules::ammdata::utils::reserves::{
    NewPoolInfo, extract_new_pools_from_espo_transaction,
};
use crate::modules::essentials::storage::{
    EssentialsProvider, GetCreationRecordParams, GetLatestCirculatingSupplyParams,
};
use crate::modules::essentials::utils::balances::{
    clean_espo_sandshrew_like_trace, get_alkane_balances,
};
use crate::schemas::SchemaAlkaneId;
use anyhow::Result;
use bitcoin::consensus::encode::deserialize;
use bitcoin::hashes::Hash;
use bitcoin::{ScriptBuf, Transaction, Txid};
use std::collections::{HashMap, HashSet};

pub struct PoolDiscoveryResult {
    pub tx_meta: HashMap<Txid, (Vec<u8>, bool)>,
}

pub fn discover_new_pools(
    block: &EspoBlock,
    block_ts: u64,
    height: u32,
    provider: &AmmDataProvider,
    essentials: &EssentialsProvider,
    canonical_quote_units: &HashMap<SchemaAlkaneId, CanonicalQuoteUnit>,
    amm_factories: &HashSet<SchemaAlkaneId>,
    state: &mut IndexState,
) -> Result<PoolDiscoveryResult> {
    let table = provider.table();

    let mut block_tx_map: HashMap<Txid, &Transaction> = HashMap::new();
    for atx in &block.transactions {
        block_tx_map.insert(atx.transaction.compute_txid(), &atx.transaction);
    }
    let mut tx_meta: HashMap<Txid, (Vec<u8>, bool)> = HashMap::new();
    let mut prev_tx_cache: HashMap<Txid, Transaction> = HashMap::new();
    for atx in &block.transactions {
        let txid = atx.transaction.compute_txid();
        let spk_bytes = crate::modules::ammdata::pool_creator_spk_from_protostone(&atx.transaction)
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_default();
        let success = atx.traces.as_ref().map_or(true, |traces| {
            !traces.iter().any(|trace| {
                trace.sandshrew_trace.events.iter().any(|ev| {
                    matches!(
                        ev,
                        EspoSandshrewLikeTraceEvent::Return(r)
                            if r.status == crate::alkanes::trace::EspoSandshrewLikeTraceStatus::Failure
                    )
                })
            })
        });
        tx_meta.insert(txid, (spk_bytes, success));
    }

    // Discover new pools (per-tx) and record pool creation activity.
    let mut seen_new_pools: HashSet<(u32, u64)> = HashSet::new();
    for transaction in block.transactions.iter() {
        if transaction.traces.is_none() {
            continue;
        }

        let mut pool_factory_by_id: HashMap<SchemaAlkaneId, SchemaAlkaneId> = HashMap::new();
        if let Some(traces) = &transaction.traces {
            for trace in traces {
                let Some(cleaned) = clean_espo_sandshrew_like_trace(
                    &trace.sandshrew_trace,
                    &block.host_function_values,
                ) else {
                    continue;
                };
                let mut pending_factory: Option<SchemaAlkaneId> = None;
                for ev in &cleaned.events {
                    match ev {
                        EspoSandshrewLikeTraceEvent::Invoke(inv) => {
                            if let Some(factory) =
                                crate::modules::ammdata::parse_factory_create_call(
                                    inv,
                                    amm_factories,
                                )
                            {
                                pending_factory = Some(factory);
                            }
                        }
                        EspoSandshrewLikeTraceEvent::Create(c) => {
                            if let Some(factory) = pending_factory.take() {
                                if let (Some(block), Some(tx)) = (
                                    crate::modules::ammdata::parse_hex_u32(&c.block),
                                    crate::modules::ammdata::parse_hex_u64(&c.tx),
                                ) {
                                    pool_factory_by_id
                                        .insert(SchemaAlkaneId { block, tx }, factory);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if let Ok(new_pools) =
            extract_new_pools_from_espo_transaction(transaction, &block.host_function_values)
        {
            for NewPoolInfo { pool_id, defs, factory_id } in new_pools {
                if !seen_new_pools.insert((pool_id.block, pool_id.tx)) {
                    continue;
                }
                let factory_from_call = pool_factory_by_id.get(&pool_id).copied();
                let factory_id = factory_from_call.or(factory_id);
                let factory_ok = factory_id.map(|id| amm_factories.contains(&id)).unwrap_or(false);
                if !factory_ok {
                    continue;
                }

                state.pools_map.insert(pool_id, defs);
                if let Ok(encoded_defs) = borsh::to_vec(&defs) {
                    state.pool_defs_writes.push((table.pools_key(&pool_id), encoded_defs));
                }
                state
                    .token_pools_writes
                    .push((table.token_pools_key(&defs.base_alkane_id, &pool_id), Vec::new()));
                state
                    .token_pools_writes
                    .push((table.token_pools_key(&defs.quote_alkane_id, &pool_id), Vec::new()));
                state.reserves_snapshot.entry(pool_id).or_insert(
                    crate::modules::ammdata::schemas::SchemaPoolSnapshot {
                        base_reserve: 0,
                        quote_reserve: 0,
                        base_id: defs.base_alkane_id,
                        quote_id: defs.quote_alkane_id,
                    },
                );
                if canonical_quote_units.contains_key(&defs.quote_alkane_id) {
                    state
                        .canonical_pool_updates
                        .entry(defs.base_alkane_id)
                        .or_default()
                        .push(SchemaCanonicalPoolEntry { pool_id, quote_id: defs.quote_alkane_id });
                }
                if canonical_quote_units.contains_key(&defs.base_alkane_id) {
                    state
                        .canonical_pool_updates
                        .entry(defs.quote_alkane_id)
                        .or_default()
                        .push(SchemaCanonicalPoolEntry { pool_id, quote_id: defs.base_alkane_id });
                }

                let pool_label =
                    get_alkane_label(essentials, &mut state.alkane_label_cache, &pool_id);
                let pool_name = crate::modules::ammdata::strip_lp_suffix(&pool_label);
                let pool_name_norm = pool_name.trim().to_ascii_lowercase();
                if !pool_name_norm.is_empty() {
                    state
                        .pool_name_index_writes
                        .push((table.pool_name_index_key(&pool_name_norm, &pool_id), Vec::new()));
                }

                if let Some(factory_id) = factory_id {
                    state
                        .factory_pools_writes
                        .push((table.factory_pools_key(&factory_id, &pool_id), Vec::new()));
                    let mut factory_bytes = Vec::with_capacity(12);
                    factory_bytes.extend_from_slice(&factory_id.block.to_be_bytes());
                    factory_bytes.extend_from_slice(&factory_id.tx.to_be_bytes());
                    state
                        .pool_factory_writes
                        .push((table.pool_factory_key(&pool_id), factory_bytes));
                }

                // Pool creation info
                let mut creator_spk = crate::modules::ammdata::pool_creator_spk_from_protostone(
                    &transaction.transaction,
                );
                if creator_spk.is_none() {
                    let mut lowest_spk: Option<ScriptBuf> = None;
                    let mut lowest_value: Option<u64> = None;
                    for vin in &transaction.transaction.input {
                        if vin.previous_output.is_null() {
                            continue;
                        }
                        let prev_txid = vin.previous_output.txid;
                        let prev_tx = if let Some(tx) = block_tx_map.get(&prev_txid) {
                            Some((*tx).clone())
                        } else if let Some(tx) = prev_tx_cache.get(&prev_txid) {
                            Some(tx.clone())
                        } else {
                            let raw = get_electrum_like()
                                .batch_transaction_get_raw(&[prev_txid])
                                .unwrap_or_default()
                                .into_iter()
                                .next()
                                .unwrap_or_default();
                            if raw.is_empty() {
                                None
                            } else {
                                deserialize::<Transaction>(&raw).ok().map(|tx| {
                                    prev_tx_cache.insert(prev_txid, tx.clone());
                                    tx
                                })
                            }
                        };
                        let Some(prev_tx) = prev_tx else { continue };
                        let idx = vin.previous_output.vout as usize;
                        let Some(prev_out) = prev_tx.output.get(idx) else { continue };
                        let value = prev_out.value.to_sat();
                        if lowest_value.map_or(true, |v| value < v) {
                            lowest_value = Some(value);
                            lowest_spk = Some(prev_out.script_pubkey.clone());
                        }
                    }
                    creator_spk = lowest_spk;
                }

                let mut pool_balances =
                    get_alkane_balances(essentials, &pool_id).unwrap_or_default();
                let initial_token0_amount = pool_balances.remove(&defs.base_alkane_id).unwrap_or(0);
                let initial_token1_amount =
                    pool_balances.remove(&defs.quote_alkane_id).unwrap_or(0);
                let initial_lp_supply = essentials
                    .get_latest_circulating_supply(GetLatestCirculatingSupplyParams {
                        alkane: pool_id,
                    })
                    .map(|res| res.supply)
                    .unwrap_or(0);

                state.pool_lp_supply_writes.push((
                    table.pool_lp_supply_latest_key(&pool_id),
                    encode_u128_value(initial_lp_supply)?,
                ));

                let creation_info = SchemaPoolCreationInfoV1 {
                    creator_spk: creator_spk.map(|s| s.as_bytes().to_vec()).unwrap_or_default(),
                    creation_height: height,
                    initial_token0_amount,
                    initial_token1_amount,
                    initial_lp_supply,
                };
                state.pool_creation_info_cache.insert(pool_id, creation_info.clone());
                state.pool_creation_info_writes.push((
                    table.pool_creation_info_key(&pool_id),
                    encode_pool_creation_info(&creation_info)?,
                ));

                let txid = transaction.transaction.compute_txid();
                let txid_bytes = txid.to_byte_array();
                let (address_spk, success) =
                    tx_meta.get(&txid).cloned().unwrap_or_else(|| (Vec::new(), true));

                let activity = SchemaActivityV1 {
                    timestamp: block_ts,
                    txid: txid_bytes,
                    kind: ActivityKind::PoolCreate,
                    direction: None,
                    base_delta: 0,
                    quote_delta: 0,
                    address_spk,
                    success,
                };

                if let Ok(seq) = state.activity_acc.push(pool_id, block_ts, activity.clone()) {
                    state.index_acc.add(&pool_id, block_ts, seq, &activity);
                    state
                        .pool_creations_writes
                        .push((table.pool_creations_key(block_ts, seq, &pool_id), Vec::new()));
                    if !activity.address_spk.is_empty() {
                        state.address_pool_creations_writes.push((
                            table.address_pool_creations_key(
                                &activity.address_spk,
                                block_ts,
                                seq,
                                &pool_id,
                            ),
                            Vec::new(),
                        ));
                        state.address_amm_history_writes.push((
                            table.address_amm_history_key(
                                &activity.address_spk,
                                block_ts,
                                seq,
                                activity.kind,
                                &pool_id,
                            ),
                            Vec::new(),
                        ));
                    }
                    state.amm_history_all_writes.push((
                        table.amm_history_all_key(block_ts, seq, activity.kind, &pool_id),
                        Vec::new(),
                    ));
                }

                println!(
                    "[AMMDATA] New pool created @ block #{blk}, ts={ts}\n\
                     [AMMDATA]   Pool:  {pb}:{pt}\n\
                     [AMMDATA]   Base:  {bb}:{bt}\n\
                     [AMMDATA]   Quote: {qb}:{qt}",
                    blk = height,
                    ts = block_ts,
                    pb = pool_id.block,
                    pt = pool_id.tx,
                    bb = defs.base_alkane_id.block,
                    bt = defs.base_alkane_id.tx,
                    qb = defs.quote_alkane_id.block,
                    qt = defs.quote_alkane_id.tx
                );
            }
        }
    }

    Ok(PoolDiscoveryResult { tx_meta })
}

pub(crate) fn get_alkane_label(
    essentials: &EssentialsProvider,
    cache: &mut HashMap<SchemaAlkaneId, String>,
    alkane: &SchemaAlkaneId,
) -> String {
    if let Some(label) = cache.get(alkane) {
        return label.clone();
    }
    let label = essentials
        .get_creation_record(GetCreationRecordParams { alkane: *alkane })
        .ok()
        .and_then(|resp| resp.record)
        .and_then(|rec| rec.symbols.first().cloned().or_else(|| rec.names.first().cloned()))
        .unwrap_or_else(|| format!("{}:{}", alkane.block, alkane.tx));
    cache.insert(*alkane, label.clone());
    label
}
