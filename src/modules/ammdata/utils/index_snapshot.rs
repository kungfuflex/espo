use crate::modules::ammdata::schemas::{SchemaMarketDefs, SchemaPoolSnapshot};
use crate::modules::ammdata::storage::{AmmDataProvider, GetReservesSnapshotParams};
use crate::runtime::state_at::StateAt;
use crate::schemas::SchemaAlkaneId;
use anyhow::Result;
use std::collections::HashMap;

pub fn load_reserves_snapshot(
    provider: &AmmDataProvider,
) -> Result<HashMap<SchemaAlkaneId, SchemaPoolSnapshot>> {
    let snapshot = provider
        .get_reserves_snapshot(GetReservesSnapshotParams { blockhash: StateAt::Latest })?
        .snapshot
        .unwrap_or_default();
    Ok(snapshot)
}

pub fn pools_map_from_snapshot(
    reserves_snapshot: &HashMap<SchemaAlkaneId, SchemaPoolSnapshot>,
) -> HashMap<SchemaAlkaneId, SchemaMarketDefs> {
    let mut pools_map: HashMap<SchemaAlkaneId, SchemaMarketDefs> = HashMap::new();
    for (pool, snap) in reserves_snapshot.iter() {
        pools_map.insert(
            *pool,
            SchemaMarketDefs {
                pool_alkane_id: *pool,
                base_alkane_id: snap.base_id,
                quote_alkane_id: snap.quote_id,
            },
        );
    }
    pools_map
}
