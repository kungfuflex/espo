use crate::config::{debug_enabled, get_config, get_espo_db};
use crate::debug;
use crate::modules::ammdata::consts::ammdata_genesis_block;
use crate::modules::ammdata::storage::{
    AmmDataProvider, GetIndexHeightParams as AmmDataGetIndexHeightParams,
};
use crate::modules::defs::{EspoModule, RpcNsRegistrar};
use crate::modules::essentials::storage::{
    EssentialsProvider, GetIndexHeightParams as EssentialsGetIndexHeightParams,
};
use crate::modules::oylapi::config::OylApiConfig;
use crate::modules::oylapi::server::run as run_oylapi;
use crate::modules::oylapi::storage::{BtcUsdPriceCache, OylApiState, refresh_btc_usd_price_cache};
use crate::modules::subfrost::storage::SubfrostProvider;
use crate::runtime::mdb::Mdb;
use crate::runtime::state_at::StateAt;
use anyhow::{Result, anyhow};
use bitcoin::Network;
use std::net::SocketAddr;
use std::sync::Arc;

pub struct OylApi {
    config: Option<OylApiConfig>,
    essentials: Option<Arc<EssentialsProvider>>,
    ammdata: Option<Arc<AmmDataProvider>>,
    subfrost: Option<Arc<SubfrostProvider>>,
    btc_usd_price_cache: Arc<BtcUsdPriceCache>,
}

impl OylApi {
    pub fn new() -> Self {
        Self {
            config: None,
            essentials: None,
            ammdata: None,
            subfrost: None,
            btc_usd_price_cache: Arc::new(BtcUsdPriceCache::new()),
        }
    }
}

impl Default for OylApi {
    fn default() -> Self {
        Self::new()
    }
}

impl EspoModule for OylApi {
    fn get_name(&self) -> &'static str {
        "oylapi"
    }

    fn set_mdb(&mut self, _mdb: Arc<Mdb>) {
        let essentials_mdb = Mdb::from_db(get_espo_db(), b"essentials:");
        let essentials = Arc::new(EssentialsProvider::new(Arc::new(essentials_mdb)));
        let amm_mdb = Mdb::from_db(get_espo_db(), b"ammdata:");
        let ammdata = Arc::new(AmmDataProvider::new(Arc::new(amm_mdb), essentials.clone()));
        let subfrost_mdb = Mdb::from_db(get_espo_db(), b"subfrost:");
        let subfrost = Arc::new(SubfrostProvider::new(Arc::new(subfrost_mdb)));
        self.essentials = Some(essentials);
        self.ammdata = Some(ammdata);
        self.subfrost = Some(subfrost);
    }

    fn get_genesis_block(&self, network: Network) -> u32 {
        ammdata_genesis_block(network)
    }

    fn index_block(&self, block: crate::alkanes::trace::EspoBlock) -> Result<()> {
        if !block.is_latest {
            return Ok(());
        }

        let t0 = std::time::Instant::now();
        let debug = debug_enabled();
        let module = self.get_name();

        let timer = debug::start_if(debug);
        let has_config = self.config.is_some();
        debug::log_elapsed(module, "check_config", timer);

        let timer = debug::start_if(debug);
        let has_essentials = self.essentials.is_some();
        debug::log_elapsed(module, "check_essentials", timer);

        let timer = debug::start_if(debug);
        let has_ammdata = self.ammdata.is_some();
        debug::log_elapsed(module, "check_ammdata", timer);

        let timer = debug::start_if(debug);
        let has_subfrost = self.subfrost.is_some();
        debug::log_elapsed(module, "check_subfrost", timer);

        let timer = debug::start_if(debug);
        let _state = (has_config, has_essentials, has_ammdata, has_subfrost);
        debug::log_elapsed(module, "finalize", timer);

        if let Some(ammdata) = self.ammdata.as_ref() {
            match refresh_btc_usd_price_cache(ammdata.as_ref(), self.btc_usd_price_cache.as_ref()) {
                Ok(Some(entry)) if debug => eprintln!(
                    "[oylapi] refreshed btc/usd cache tip={} price_height={} price_scaled={}",
                    entry.tip_height, entry.price_height, entry.price
                ),
                Ok(_) => {}
                Err(e) => eprintln!(
                    "[oylapi] failed to refresh btc/usd cache at height {}: {e:?}",
                    block.height
                ),
            }
        }
        eprintln!(
            "[indexer] module={} height={} index_block done in {:?}",
            self.get_name(),
            block.height,
            t0.elapsed()
        );
        Ok(())
    }

    fn get_index_height(&self) -> Option<u32> {
        if let Some(ammdata) = self.ammdata.as_ref() {
            if let Ok(result) =
                ammdata.get_index_height(AmmDataGetIndexHeightParams { blockhash: StateAt::Latest })
            {
                if result.height.is_some() {
                    return result.height;
                }
            }
        }

        self.essentials.as_ref().and_then(|essentials| {
            essentials
                .get_index_height(EssentialsGetIndexHeightParams { blockhash: StateAt::Latest })
                .ok()
                .and_then(|result| result.height)
        })
    }

    fn handle_reorg(&self, next_height: u32) -> Result<()> {
        self.btc_usd_price_cache.clear()?;
        eprintln!("[OYLAPI] cleared BTC/USD cache after reorg; next_height={next_height}");
        Ok(())
    }

    fn register_rpc(&self, _reg: &RpcNsRegistrar) {
        let Some(cfg) = self.config.clone() else {
            return;
        };
        let essentials = self
            .essentials
            .as_ref()
            .expect("oylapi module missing essentials provider")
            .clone();
        let ammdata =
            self.ammdata.as_ref().expect("oylapi module missing ammdata provider").clone();
        let subfrost =
            self.subfrost.as_ref().expect("oylapi module missing subfrost provider").clone();
        let btc_usd_price_cache = self.btc_usd_price_cache.clone();
        match refresh_btc_usd_price_cache(ammdata.as_ref(), btc_usd_price_cache.as_ref()) {
            Ok(Some(entry)) => eprintln!(
                "[oylapi] loaded btc/usd cache tip={} price_height={} price_scaled={}",
                entry.tip_height, entry.price_height, entry.price
            ),
            Ok(None) => eprintln!("[oylapi] btc/usd cache empty (ammdata price index empty)"),
            Err(e) => eprintln!("[oylapi] failed to load btc/usd cache: {e:?}"),
        }

        let addr: SocketAddr = format!("{}:{}", cfg.host, cfg.port)
            .parse()
            .unwrap_or_else(|e| panic!("invalid oylapi host/port: {e}"));

        let state = OylApiState {
            config: cfg,
            essentials,
            ammdata,
            subfrost,
            http_client: reqwest::Client::new(),
            btc_usd_price_cache,
        };

        tokio::spawn(async move {
            if let Err(e) = run_oylapi(addr, state).await {
                eprintln!("[oylapi] server error: {e:?}");
            }
        });
        eprintln!("[oylapi] listening on {}", addr);
    }

    fn config_spec(&self) -> Option<&'static str> {
        Some(OylApiConfig::spec())
    }

    fn set_config(&mut self, config: &serde_json::Value) -> Result<()> {
        if get_config().electrs_esplora_url.is_none() {
            return Err(anyhow!("oylapi requires electrs_esplora_url (script-hash UTXO support)"));
        }
        let parsed = OylApiConfig::from_value(config)?;
        self.config = Some(parsed);
        Ok(())
    }
}
