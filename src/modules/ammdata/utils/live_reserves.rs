use crate::runtime::state_at::StateAt;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::config::get_metashrew;
use crate::modules::ammdata::schemas::{SchemaMarketDefs, SchemaPoolSnapshot};
use crate::modules::ammdata::storage::{
    AmmDataProvider, GetAmmFactoriesParams, GetFactoryPoolsParams, GetIndexHeightParams,
    GetPoolDefsParams,
};
use crate::schemas::SchemaAlkaneId;

#[derive(Clone)]
struct LiveReservesCache {
    index_height: Option<u32>,
    pools: HashMap<SchemaAlkaneId, SchemaPoolSnapshot>,
}

static LIVE_RESERVES_CACHE: OnceLock<Mutex<Option<LiveReservesCache>>> = OnceLock::new();

fn live_reserves_cache() -> &'static Mutex<Option<LiveReservesCache>> {
    LIVE_RESERVES_CACHE.get_or_init(|| Mutex::new(None))
}

/// Fetch real-time reserves for all pools in `pools` by querying Metashrew balances:
/// - base_reserve = balance of {what = base_id} held by {who = pool_alkane_id}
/// - quote_reserve = balance of {what = quote_id} held by {who = pool_alkane_id}
///
/// Returns a snapshot map identical to your in-memory schema.
pub fn fetch_latest_reserves_for_pools(
    pools: &HashMap<SchemaAlkaneId, SchemaMarketDefs>,
) -> Result<HashMap<SchemaAlkaneId, SchemaPoolSnapshot>> {
    let metashrew = get_metashrew();
    let mut out: HashMap<SchemaAlkaneId, SchemaPoolSnapshot> = HashMap::with_capacity(pools.len());

    for (pool_id, defs) in pools {
        let base_bal = metashrew
            .get_reserves_for_alkane(pool_id, &defs.base_alkane_id, None)?
            .unwrap_or(0);
        let quote_bal = metashrew
            .get_reserves_for_alkane(pool_id, &defs.quote_alkane_id, None)?
            .unwrap_or(0);

        eprintln!(
            "[AMMDATA-LIVE] pool {}/{} live reserves: base={}, quote={}",
            pool_id.block, pool_id.tx, base_bal, quote_bal
        );

        out.insert(
            *pool_id,
            SchemaPoolSnapshot {
                base_reserve: base_bal,
                quote_reserve: quote_bal,
                base_id: defs.base_alkane_id,
                quote_id: defs.quote_alkane_id,
            },
        );
    }

    Ok(out)
}

pub fn fetch_all_pools(
    provider: &AmmDataProvider,
) -> Result<HashMap<SchemaAlkaneId, SchemaPoolSnapshot>> {
    let index_height = provider
        .get_index_height(GetIndexHeightParams { blockhash: StateAt::Latest })
        .ok()
        .and_then(|res| res.height);
    let mut cache = live_reserves_cache().lock().unwrap_or_else(|err| err.into_inner());
    if let Some(cached) = cache.as_ref() {
        if cached.index_height == index_height {
            return Ok(cached.pools.clone());
        }
    }

    let factories = provider
        .get_amm_factories(GetAmmFactoriesParams { blockhash: StateAt::Latest })?
        .factories;
    let mut pools: HashMap<SchemaAlkaneId, SchemaMarketDefs> = HashMap::new();

    for factory in factories {
        let factory_pools = match provider
            .get_factory_pools(GetFactoryPoolsParams { blockhash: StateAt::Latest, factory })
        {
            Ok(v) => v.pools,
            Err(_) => continue,
        };
        for pool in factory_pools {
            if pools.contains_key(&pool) {
                continue;
            }
            if let Ok(v) =
                provider.get_pool_defs(GetPoolDefsParams { blockhash: StateAt::Latest, pool })
            {
                if let Some(defs) = v.defs {
                    pools.insert(pool, defs);
                }
            }
        }
    }

    let live_pools = fetch_latest_reserves_for_pools(&pools)?;
    *cache = Some(LiveReservesCache { index_height, pools: live_pools.clone() });
    Ok(live_pools)
}
