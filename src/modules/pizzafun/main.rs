use crate::alkanes::trace::EspoBlock;
use crate::config::{debug_enabled, get_espo_module_mdb};
use crate::debug;
use crate::explorer::components::tx_view::alkane_icon_url;
use crate::modules::ammdata::storage::{
    AmmDataProvider, GetTokenMarketUpdatedAlkanesInBlockParams, GetTokenMetricsParams,
};
use crate::modules::defs::{EspoModule, RpcNsRegistrar};
use crate::modules::essentials::storage::{
    EssentialsProvider, GetCreationIdsInBlockParams, GetCreationRecordParams,
    GetCreationRecordsByIdParams, GetHoldersCountParams,
    GetIndexHeightParams as EssentialsGetIndexHeightParams, GetRawValueParams,
};
use crate::modules::essentials::utils::names::normalize_alkane_name;
use crate::runtime::mdb::Mdb;
use crate::runtime::state_at::StateAt;
use crate::schemas::SchemaAlkaneId;
use anyhow::Result;
use bitcoin::Network;
use borsh::BorshDeserialize;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

use super::config::PizzafunConfig;
use super::consts::PRIORITY_SERIES_ALKANES;
use super::rpc;
use super::server::{SnapshotHttpState, run as run_snapshot_server};
use super::snapshot::{BondedSnapshotRowV1, PizzafunChainMetadataV1, SnapshotTokenStatus};
use super::storage::{
    GetIndexHeightParams as PizzafunGetIndexHeightParams, GetSeriesByAlkaneParams,
    GetSeriesEntriesByNameParams, PizzafunProvider, SeriesEntry, series_id_base_from_name,
};

fn parse_alkane_id_str(s: &str) -> Option<SchemaAlkaneId> {
    let (block_raw, tx_raw) = s.split_once(':')?;
    let parse_u32 = |v: &str| {
        if let Some(hex) = v.strip_prefix("0x") {
            u32::from_str_radix(hex, 16).ok()
        } else {
            v.parse::<u32>().ok()
        }
    };
    let parse_u64 = |v: &str| {
        if let Some(hex) = v.strip_prefix("0x") {
            u64::from_str_radix(hex, 16).ok()
        } else {
            v.parse::<u64>().ok()
        }
    };
    Some(SchemaAlkaneId { block: parse_u32(block_raw)?, tx: parse_u64(tx_raw)? })
}

pub struct Pizzafun {
    config: Option<PizzafunConfig>,
    essentials_provider: Option<Arc<EssentialsProvider>>,
    ammdata_provider: Option<Arc<AmmDataProvider>>,
    provider: Option<Arc<PizzafunProvider>>,
}

impl Pizzafun {
    pub fn new() -> Self {
        Self { config: None, essentials_provider: None, ammdata_provider: None, provider: None }
    }

    #[inline]
    fn essentials_provider(&self) -> &EssentialsProvider {
        self.essentials_provider
            .as_ref()
            .expect("ModuleRegistry must call set_mdb()")
            .as_ref()
    }

    #[inline]
    fn provider(&self) -> &PizzafunProvider {
        self.provider.as_ref().expect("ModuleRegistry must call set_mdb()").as_ref()
    }

    #[inline]
    fn ammdata_provider(&self) -> &AmmDataProvider {
        self.ammdata_provider
            .as_ref()
            .expect("ModuleRegistry must call set_mdb()")
            .as_ref()
    }

    #[inline]
    fn config(&self) -> &PizzafunConfig {
        self.config.as_ref().expect("ModuleRegistry must call set_config()")
    }

    fn load_essentials_index_height(&self) -> Option<u32> {
        let resp = self
            .essentials_provider()
            .get_index_height(EssentialsGetIndexHeightParams { blockhash: StateAt::Latest })
            .ok()?;
        resp.height
    }

    fn load_index_height(&self) -> Option<u32> {
        if let Ok(resp) = self
            .provider()
            .get_index_height(PizzafunGetIndexHeightParams { blockhash: StateAt::Latest })
            && resp.height.is_some()
        {
            return resp.height;
        }
        self.load_essentials_index_height()
    }

    fn priority_index_map() -> HashMap<SchemaAlkaneId, usize> {
        let mut priority_index: HashMap<SchemaAlkaneId, usize> = HashMap::new();
        for (idx, raw) in PRIORITY_SERIES_ALKANES.iter().enumerate() {
            if let Some(id) = parse_alkane_id_str(raw) {
                priority_index.entry(id).or_insert(idx);
            }
        }
        priority_index
    }

    fn sort_series_entries(
        entries: &mut [SeriesEntry],
        priority_index: &HashMap<SchemaAlkaneId, usize>,
    ) {
        entries.sort_by(|a, b| {
            let a_pri = priority_index.get(&a.alkane_id);
            let b_pri = priority_index.get(&b.alkane_id);
            match (a_pri, b_pri) {
                (Some(ai), Some(bi)) => ai
                    .cmp(bi)
                    .then_with(|| a.creation_height.cmp(&b.creation_height))
                    .then_with(|| a.alkane_id.cmp(&b.alkane_id)),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => a
                    .creation_height
                    .cmp(&b.creation_height)
                    .then_with(|| a.alkane_id.cmp(&b.alkane_id)),
            }
        });
    }

    fn load_chain_metadata(
        &self,
        alkane: &SchemaAlkaneId,
        blockhash: StateAt,
    ) -> Option<PizzafunChainMetadataV1> {
        let table = self.essentials_provider().table();
        let key = table.kv_row_key(alkane, b"/metadata");
        let raw = self
            .essentials_provider()
            .get_raw_value(GetRawValueParams { blockhash, key })
            .ok()?
            .value?;
        let payload = if raw.len() >= 32 { &raw[32..] } else { raw.as_slice() };
        PizzafunChainMetadataV1::try_from_slice(payload).ok()
    }

    fn build_bonded_row_for_alkane(
        &self,
        alkane: &SchemaAlkaneId,
        blockhash: StateAt,
        last_traded_at: u64,
    ) -> Result<Option<BondedSnapshotRowV1>> {
        let Some(series) = self
            .provider()
            .get_series_by_alkane(GetSeriesByAlkaneParams { blockhash, alkane: *alkane })?
        else {
            return Ok(None);
        };

        let Some(rec) = self
            .essentials_provider()
            .get_creation_record(GetCreationRecordParams { blockhash, alkane: *alkane })?
            .record
        else {
            return Ok(None);
        };

        let metrics = self
            .ammdata_provider()
            .get_token_metrics(GetTokenMetricsParams { blockhash, token: *alkane })?
            .metrics;
        let holders = self
            .essentials_provider()
            .get_holders_count(GetHoldersCountParams { blockhash, alkane: *alkane })?
            .count;
        let metadata = self.load_chain_metadata(alkane, blockhash);

        let name = metadata
            .as_ref()
            .map(|v| v.name.clone())
            .filter(|v| !v.trim().is_empty())
            .or_else(|| rec.names.first().cloned())
            .unwrap_or_else(|| series.series_id.clone());
        let symbol = metadata
            .as_ref()
            .map(|v| v.symbol.clone())
            .filter(|v| !v.trim().is_empty())
            .or_else(|| rec.symbols.first().cloned())
            .unwrap_or_else(|| name.clone());
        let description = metadata.as_ref().map(|v| v.description.clone()).unwrap_or_default();
        let icon_url = metadata
            .as_ref()
            .map(|v| v.icon_url.clone())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| alkane_icon_url(alkane, self.essentials_provider().mdb()));
        let created_at = metadata
            .as_ref()
            .map(|v| v.timestamp)
            .unwrap_or((rec.creation_timestamp as u64).saturating_mul(1000));
        let last_traded_at_ms = last_traded_at
            .saturating_mul(1000)
            .max((rec.creation_timestamp as u64).saturating_mul(1000));

        Ok(Some(BondedSnapshotRowV1 {
            series_id: series.series_id.clone(),
            protocol_id: format!("{}:{}", alkane.block, alkane.tx),
            created_at,
            name,
            symbol,
            description,
            icon_url,
            status: SnapshotTokenStatus::Bonded,
            last_traded_at: last_traded_at_ms,
            price_usd: metrics.price_usd,
            market_cap_usd: metrics.marketcap_usd,
            volume_1d: metrics.volume_1d,
            volume_7d: metrics.volume_7d,
            volume_30d: metrics.volume_30d,
            volume_all_time: metrics.volume_all_time,
            change_1d_bps: crate::modules::ammdata::storage::parse_change_basis_points(
                &metrics.change_1d,
            ),
            change_7d_bps: crate::modules::ammdata::storage::parse_change_basis_points(
                &metrics.change_7d,
            ),
            change_30d_bps: crate::modules::ammdata::storage::parse_change_basis_points(
                &metrics.change_30d,
            ),
            change_all_time_bps: crate::modules::ammdata::storage::parse_change_basis_points(
                &metrics.change_all_time,
            ),
            holders,
            telegram_link: metadata.as_ref().and_then(|v| v.telegram_link.clone()),
            x_link: metadata.as_ref().and_then(|v| v.x_link.clone()),
            website_link: metadata.as_ref().and_then(|v| v.website_link.clone()),
        }))
    }
}

impl Default for Pizzafun {
    fn default() -> Self {
        Self::new()
    }
}

impl EspoModule for Pizzafun {
    fn get_name(&self) -> &'static str {
        "pizzafun"
    }

    fn set_mdb(&mut self, mdb: Arc<Mdb>) {
        let essentials_provider =
            Arc::new(EssentialsProvider::new(get_espo_module_mdb("essentials")));
        let ammdata_provider = Arc::new(AmmDataProvider::new(
            get_espo_module_mdb("ammdata"),
            essentials_provider.clone(),
        ));
        self.essentials_provider = Some(essentials_provider);
        self.ammdata_provider = Some(ammdata_provider);
        self.provider = Some(Arc::new(PizzafunProvider::new(mdb)));
        eprintln!("[PIZZAFUN] loaded index height: {:?}", self.load_index_height());
    }

    fn get_genesis_block(&self, network: Network) -> u32 {
        crate::modules::essentials::consts::essentials_genesis_block(network)
    }

    fn get_mdb(&self) -> Option<Arc<Mdb>> {
        self.provider.as_ref().map(|provider| Arc::new(provider.mdb().clone()))
    }

    fn index_block(&self, block: EspoBlock) -> Result<()> {
        let t0 = std::time::Instant::now();
        let debug = debug_enabled();
        let module = self.get_name();
        let block_hash = block.block_header.block_hash();
        let block_time = u64::from(block.block_header.time);

        let timer = debug::start_if(debug);
        let mut new_alkanes = self
            .essentials_provider()
            .get_creation_ids_in_block(GetCreationIdsInBlockParams {
                blockhash: StateAt::Block(block_hash),
                height: block.height,
            })?
            .alkanes;
        let mut seen: HashSet<SchemaAlkaneId> = HashSet::new();
        new_alkanes.retain(|a| seen.insert(*a));
        debug::log_elapsed(module, "collect_created_alkanes", timer);

        let timer = debug::start_if(debug);
        let mut rows_to_refresh: HashSet<SchemaAlkaneId> = HashSet::new();
        if !new_alkanes.is_empty() {
            let records = self
                .essentials_provider()
                .get_creation_records_by_id(GetCreationRecordsByIdParams {
                    blockhash: StateAt::Block(block_hash),
                    alkanes: new_alkanes,
                })?
                .records;

            let mut by_name: HashMap<String, Vec<SeriesEntry>> = HashMap::new();
            for rec in records.into_iter().flatten() {
                let matches_factory = rec
                    .inspection
                    .as_ref()
                    .and_then(|inspection| inspection.factory_alkane)
                    .map(|factory| factory == self.config().factory_id)
                    .unwrap_or(false);
                if !matches_factory {
                    continue;
                }
                let Some(raw_name) = rec.names.first() else { continue };
                let Some(name_norm) = normalize_alkane_name(raw_name) else { continue };
                rows_to_refresh.insert(rec.alkane);
                by_name.entry(name_norm).or_default().push(SeriesEntry {
                    series_id: String::new(),
                    alkane_id: rec.alkane,
                    creation_height: rec.creation_height,
                });
            }

            if !by_name.is_empty() {
                let priority_index = Self::priority_index_map();
                for (name, mut new_entries) in by_name {
                    let Some(series_base) = series_id_base_from_name(&name) else { continue };
                    let existing = self.provider().get_series_entries_by_name(
                        GetSeriesEntriesByNameParams {
                            blockhash: StateAt::Block(block_hash),
                            name_norm: name.clone(),
                        },
                    )?;
                    if !existing.is_empty() {
                        let mut existing_ids: HashSet<SchemaAlkaneId> =
                            existing.iter().map(|e| e.alkane_id).collect();
                        new_entries.retain(|e| existing_ids.insert(e.alkane_id));
                    }
                    if new_entries.is_empty() {
                        continue;
                    }

                    let mut combined = existing.clone();
                    combined.extend(new_entries);
                    Self::sort_series_entries(&mut combined, &priority_index);

                    let mut updated: Vec<SeriesEntry> = Vec::with_capacity(combined.len());
                    for (idx, entry) in combined.into_iter().enumerate() {
                        let series_id = if idx == 0 {
                            series_base.clone()
                        } else {
                            format!("{}-{}", series_base, idx + 1)
                        };
                        updated.push(SeriesEntry {
                            series_id,
                            alkane_id: entry.alkane_id,
                            creation_height: entry.creation_height,
                        });
                    }

                    self.provider().update_series_for_name(&existing, &updated)?;
                }
            }
        }

        let changed = self
            .ammdata_provider()
            .get_token_market_updated_alkanes_in_block(GetTokenMarketUpdatedAlkanesInBlockParams {
                blockhash: StateAt::Block(block_hash),
                height: block.height,
            })?
            .alkanes;
        for alkane in changed {
            if self
                .provider()
                .get_series_by_alkane(GetSeriesByAlkaneParams {
                    blockhash: StateAt::Block(block_hash),
                    alkane,
                })?
                .is_some()
            {
                rows_to_refresh.insert(alkane);
            }
        }

        for alkane in rows_to_refresh {
            if let Some(row) =
                self.build_bonded_row_for_alkane(&alkane, StateAt::Block(block_hash), block_time)?
            {
                self.provider().upsert_bonded_row(&row)?;
            }
        }
        debug::log_elapsed(module, "update_series_index", timer);

        let timer = debug::start_if(debug);
        self.provider().set_index_height(super::storage::SetIndexHeightParams {
            blockhash: StateAt::Latest,
            height: block.height,
        })?;
        debug::log_elapsed(module, "store_height", timer);

        let timer = debug::start_if(debug);
        eprintln!(
            "[indexer] module={} height={} index_block done in {:?}",
            self.get_name(),
            block.height,
            t0.elapsed()
        );
        debug::log_elapsed(module, "finalize", timer);
        Ok(())
    }

    fn get_index_height(&self) -> Option<u32> {
        self.load_index_height()
    }

    fn register_rpc(&self, reg: &RpcNsRegistrar) {
        let provider = self.provider.as_ref().expect("ModuleRegistry must call set_mdb()");
        rpc::register_rpc(reg.clone(), Arc::clone(provider));

        let Some(cfg) = self.config.clone() else {
            return;
        };
        let provider = provider.clone();
        let addr = SocketAddr::new(cfg.snapshot_http_host, cfg.snapshot_http_port);
        let state = SnapshotHttpState { config: cfg, provider };
        tokio::spawn(async move {
            if let Err(err) = run_snapshot_server(addr, state).await {
                eprintln!("[pizzafun] snapshot server error: {err:?}");
            }
        });
        eprintln!("[pizzafun] snapshot transport listening on {}", addr);
    }

    fn config_spec(&self) -> Option<&'static str> {
        Some(PizzafunConfig::spec())
    }

    fn set_config(&mut self, config: &serde_json::Value) -> Result<()> {
        self.config = Some(PizzafunConfig::from_value(config)?);
        Ok(())
    }
}
