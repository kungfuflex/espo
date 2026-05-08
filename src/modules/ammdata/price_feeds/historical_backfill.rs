use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

const BTC_USD_HISTORICAL_REL_PATH: &str = "resources/btc_usd_historical.json";
static BTC_USD_HISTORICAL_BACKFILL: OnceLock<Result<BTreeMap<u64, u128>, String>> = OnceLock::new();

#[derive(Deserialize)]
struct BtcUsdHistoricalPoint {
    height: u64,
    price_scaled: String,
}

#[derive(Deserialize)]
struct BtcUsdHistoricalFile {
    points: Vec<BtcUsdHistoricalPoint>,
}

pub fn get_historical_btc_usd_price(height: u64) -> Result<Option<u128>> {
    let result = BTC_USD_HISTORICAL_BACKFILL
        .get_or_init(|| load_historical_backfill().map_err(|e| e.to_string()));
    match result {
        Ok(prices) => Ok(historical_btc_usd_price_from_points(prices, height)),
        Err(err) => Err(anyhow!("historical btc/usd backfill load failed: {err}")),
    }
}

fn historical_btc_usd_price_from_points(prices: &BTreeMap<u64, u128>, height: u64) -> Option<u128> {
    if prices.keys().next_back().is_some_and(|max_height| height > *max_height) {
        return None;
    }
    prices.range(..=height).next_back().map(|(_h, p)| *p)
}

fn load_historical_backfill() -> Result<BTreeMap<u64, u128>> {
    let path = historical_backfill_path();
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let parsed: BtcUsdHistoricalFile = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;

    let mut prices = BTreeMap::new();
    for point in parsed.points {
        let price = point.price_scaled.parse::<u128>().with_context(|| {
            format!(
                "invalid price_scaled '{}' at bitcoin height {}",
                point.price_scaled, point.height
            )
        })?;
        prices.insert(point.height, price);
    }
    Ok(prices)
}

fn historical_backfill_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(BTC_USD_HISTORICAL_REL_PATH)
}

#[cfg(test)]
mod tests {
    use super::historical_btc_usd_price_from_points;
    use std::collections::BTreeMap;

    #[test]
    fn historical_backfill_uses_previous_point_inside_file_range() {
        let prices = BTreeMap::from([(100, 10), (200, 20), (300, 30)]);

        assert_eq!(historical_btc_usd_price_from_points(&prices, 250), Some(20));
    }

    #[test]
    fn historical_backfill_does_not_extend_past_file_range() {
        let prices = BTreeMap::from([(100, 10), (200, 20), (300, 30)]);

        assert_eq!(historical_btc_usd_price_from_points(&prices, 301), None);
    }
}
