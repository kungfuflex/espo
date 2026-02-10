use crate::modules::ammdata::schemas::{SchemaMarketDefs, SchemaPoolSnapshot};
use crate::modules::ammdata::storage::{
    AmmDataProvider, GetRawValueParams, decode_reserves_snapshot,
};
use crate::schemas::SchemaAlkaneId;
use anyhow::Result;
use std::collections::HashMap;

pub fn load_reserves_snapshot(
    provider: &AmmDataProvider,
) -> Result<HashMap<SchemaAlkaneId, SchemaPoolSnapshot>> {
    let table = provider.table();
    let snapshot = if let Some(bytes) = provider
        .get_raw_value(GetRawValueParams { key: table.reserves_snapshot_key() })?
        .value
    {
        match decode_reserves_snapshot(&bytes) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[AMMDATA] WARNING: failed to decode reserves snapshot: {e:?}");
                HashMap::new()
            }
        }
    } else {
        HashMap::new()
    };
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
