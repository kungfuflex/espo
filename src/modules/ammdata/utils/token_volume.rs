use crate::modules::ammdata::schemas::Timeframe;
use crate::modules::ammdata::storage::{
    AmmDataProvider, GetListEntriesDescParams, GetRawValueParams, encode_u128_value,
};
use crate::modules::ammdata::utils::candles::bucket_start_for;
use crate::modules::ammdata::{storage::decode_u128_value, utils::index_state::IndexState};
use crate::runtime::state_at::StateAt;
use crate::schemas::SchemaAlkaneId;
use anyhow::Result;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct TokenVolumeKey {
    token: SchemaAlkaneId,
    timeframe: Timeframe,
    bucket_ts: u64,
}

#[derive(Default)]
pub struct TokenVolumeCache {
    buckets: BTreeMap<TokenVolumeKey, u128>,
    block_totals: HashMap<SchemaAlkaneId, u128>,
}

impl TokenVolumeCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply_trade_for_frames(
        &mut self,
        ts: u64,
        token: SchemaAlkaneId,
        frames: &[Timeframe],
        amount: u128,
    ) {
        if amount == 0 {
            return;
        }
        *self.block_totals.entry(token).or_default() =
            self.block_totals.get(&token).copied().unwrap_or(0).saturating_add(amount);

        for &timeframe in frames {
            let key =
                TokenVolumeKey { token, timeframe, bucket_ts: bucket_start_for(ts, timeframe) };
            *self.buckets.entry(key).or_default() =
                self.buckets.get(&key).copied().unwrap_or(0).saturating_add(amount);
        }
    }

    fn into_writes(
        self,
        provider: &AmmDataProvider,
    ) -> Result<(Vec<(Vec<u8>, Vec<u8>)>, HashMap<SchemaAlkaneId, u128>)> {
        let table = provider.table();
        let mut writes = Vec::with_capacity(self.buckets.len());

        for (key, amount) in self.buckets {
            if amount == 0 {
                continue;
            }
            let db_key = table.token_volume_key(&key.token, key.timeframe, key.bucket_ts);
            let previous = provider
                .get_raw_value(GetRawValueParams {
                    blockhash: StateAt::Latest,
                    key: db_key.clone(),
                })?
                .value
                .and_then(|raw| decode_u128_value(&raw).ok())
                .unwrap_or(0);
            writes.push((db_key, encode_u128_value(previous.saturating_add(amount))?));
        }

        Ok((writes, self.block_totals))
    }
}

pub fn prepare_token_volume(
    height: u32,
    provider: &AmmDataProvider,
    state: &mut IndexState,
) -> Result<()> {
    let (writes, block_totals) =
        std::mem::take(&mut state.token_volume_cache).into_writes(provider)?;
    state.token_volume_candle_writes = writes;

    if block_totals.is_empty() {
        return Ok(());
    }

    let table = provider.table();
    for (token, amount) in block_totals {
        if amount == 0 {
            continue;
        }
        let previous = provider
            .get_token_total_volume_at_or_before_height(&token, height.saturating_sub(1))?
            .map(|(_, value)| value)
            .unwrap_or(0);
        state.token_volume_total_writes.push((
            table.token_total_volume_key(&token, u64::from(height)),
            encode_u128_value(previous.saturating_add(amount))?,
        ));
    }

    Ok(())
}

pub struct TokenVolumeSlice {
    pub points_newest_first: Vec<(u64, u128)>,
    pub newest_ts: u64,
}

pub fn read_token_volume_v1(
    provider: &AmmDataProvider,
    token: SchemaAlkaneId,
    timeframe: Timeframe,
    now_ts: u64,
) -> Result<TokenVolumeSlice> {
    let table = provider.table();
    let prefix = table.token_volume_ns_prefix(&token, timeframe);
    let dur = timeframe.duration_secs();
    let mut per_bucket: BTreeMap<u64, u128> = BTreeMap::new();

    for (key, value) in provider
        .get_list_entries_desc(GetListEntriesDescParams { blockhash: StateAt::Latest, prefix })?
        .entries
    {
        if let Some(ts) = table.parse_token_volume_key(&token, timeframe, &key) {
            if let Ok(amount) = decode_u128_value(&value) {
                per_bucket.entry(ts).or_insert(amount);
            }
        }
    }

    if per_bucket.is_empty() {
        return Ok(TokenVolumeSlice { points_newest_first: Vec::new(), newest_ts: 0 });
    }

    let start_bucket = *per_bucket.keys().next().unwrap();
    let newest_bucket_with_data = *per_bucket.keys().last().unwrap();
    let newest_bucket_now = bucket_start_for(now_ts, timeframe);

    let mut forward = BTreeMap::new();
    let mut ts = start_bucket;
    while ts <= newest_bucket_with_data {
        forward.insert(ts, per_bucket.get(&ts).copied().unwrap_or(0));
        ts = match ts.checked_add(dur) {
            Some(next) => next,
            None => break,
        };
    }

    if newest_bucket_now > newest_bucket_with_data {
        let mut t = newest_bucket_with_data.saturating_add(dur);
        while t <= newest_bucket_now {
            forward.insert(t, 0);
            t = match t.checked_add(dur) {
                Some(next) => next,
                None => break,
            };
        }
    }

    Ok(TokenVolumeSlice {
        points_newest_first: forward.into_iter().rev().collect(),
        newest_ts: newest_bucket_now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::ammdata::schemas::SchemaMarketDefs;
    use crate::modules::ammdata::storage::SetBatchParams;
    use crate::modules::essentials::storage::EssentialsProvider;
    use crate::runtime::mdb::Mdb;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_provider() -> (TempDir, AmmDataProvider) {
        let dir = TempDir::new().expect("temp dir");
        let amm_mdb = Arc::new(Mdb::open(dir.path(), b"ammdata:").expect("open ammdata mdb"));
        let essentials =
            Arc::new(EssentialsProvider::new(Arc::new(amm_mdb.clone_with_prefix(b"essentials:"))));
        (dir, AmmDataProvider::new(amm_mdb, essentials))
    }

    #[test]
    fn token_volume_tracks_both_sides_and_cumulative_totals() {
        let (_dir, provider) = test_provider();
        let token_a = SchemaAlkaneId { block: 2, tx: 0 };
        let token_b = SchemaAlkaneId { block: 2, tx: 16 };
        let pool = SchemaAlkaneId { block: 100, tx: 1 };
        let mut pools = HashMap::new();
        pools.insert(
            pool,
            SchemaMarketDefs {
                base_alkane_id: token_a,
                quote_alkane_id: token_b,
                pool_alkane_id: pool,
            },
        );
        let mut state = IndexState::new(HashMap::new(), pools);
        let frames = [Timeframe::M10, Timeframe::H1];

        state
            .token_volume_cache
            .apply_trade_for_frames(1_700_000_123, token_a, &frames, 100);
        state
            .token_volume_cache
            .apply_trade_for_frames(1_700_000_123, token_b, &frames, 250);
        state
            .token_volume_cache
            .apply_trade_for_frames(1_700_000_456, token_a, &frames, 50);
        prepare_token_volume(10, &provider, &mut state).expect("prepare token volume");
        provider
            .set_batch(SetBatchParams {
                blockhash: StateAt::Latest,
                puts: [
                    state.token_volume_candle_writes.clone(),
                    state.token_volume_total_writes.clone(),
                ]
                .concat(),
                deletes: Vec::new(),
            })
            .expect("write token volume");

        let a_m10 = read_token_volume_v1(&provider, token_a, Timeframe::M10, 1_700_000_456)
            .expect("read m10");
        assert_eq!(a_m10.points_newest_first.first().map(|(_, v)| *v), Some(50));
        assert_eq!(a_m10.points_newest_first.get(1).map(|(_, v)| *v), Some(100));
        let a_h1 = read_token_volume_v1(&provider, token_a, Timeframe::H1, 1_700_000_456)
            .expect("read h1");
        assert_eq!(a_h1.points_newest_first.first().map(|(_, v)| *v), Some(150));
        assert_eq!(
            provider
                .get_latest_token_total_volume(&token_a)
                .expect("latest total")
                .map(|(_, value)| value),
            Some(150)
        );
        assert_eq!(
            provider
                .get_latest_token_total_volume(&token_b)
                .expect("latest total")
                .map(|(_, value)| value),
            Some(250)
        );

        let mut state = IndexState::new(HashMap::new(), HashMap::new());
        state
            .token_volume_cache
            .apply_trade_for_frames(1_700_001_000, token_a, &frames, 25);
        prepare_token_volume(11, &provider, &mut state).expect("prepare second block");
        provider
            .set_batch(SetBatchParams {
                blockhash: StateAt::Latest,
                puts: [
                    state.token_volume_candle_writes.clone(),
                    state.token_volume_total_writes.clone(),
                ]
                .concat(),
                deletes: Vec::new(),
            })
            .expect("write second token volume");

        assert_eq!(
            provider
                .get_latest_token_total_volume(&token_a)
                .expect("latest total")
                .map(|(_, value)| value),
            Some(175)
        );
    }
}
