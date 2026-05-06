use super::defs::PriceFeed;
use crate::modules::ammdata::config::AmmDataConfig;
use crate::modules::ammdata::consts::PRICE_SCALE_DECIMALS;
use anyhow::{Context, Result, anyhow};
use reqwest::blocking::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

const METHOD_GET_BTC_PRICE_AT_HEIGHT: &str = "get_btc_price_at_height";

#[derive(Clone)]
pub struct EspoPricerPriceFeed {
    rpc_url: String,
    client: Client,
}

#[derive(Serialize)]
struct RpcRequest<'a> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    params: serde_json::Value,
}

#[derive(Deserialize)]
struct RpcResponse<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

#[derive(Deserialize)]
struct BtcPriceAtHeightResponse {
    price_scaled: String,
}

impl EspoPricerPriceFeed {
    pub fn new(rpc_url: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("build reqwest client");
        Self { rpc_url: rpc_url.into(), client }
    }

    pub fn from_global_config() -> Result<Self> {
        let cfg = AmmDataConfig::load_from_global_config()?;
        Ok(Self::new(cfg.espo_pricer_host))
    }

    fn rpc_call<T: DeserializeOwned>(&self, method: &str, params: serde_json::Value) -> Result<T> {
        let req = RpcRequest { jsonrpc: "2.0", id: 1, method, params };
        let resp = self
            .client
            .post(&self.rpc_url)
            .json(&req)
            .send()
            .with_context(|| format!("espo pricer rpc request failed for method {method}"))?
            .error_for_status()
            .with_context(|| format!("espo pricer rpc HTTP error for method {method}"))?;
        let resp: RpcResponse<T> = resp.json().with_context(|| {
            format!("espo pricer rpc response decode failed for method {method}")
        })?;
        if let Some(err) = resp.error {
            anyhow::bail!("espo pricer rpc error {} from {method}: {}", err.code, err.message);
        }
        resp.result
            .ok_or_else(|| anyhow!("missing result for espo pricer method {method}"))
    }

    fn price_at_bitcoin_height(&self, height: u64) -> Result<u128> {
        let response: BtcPriceAtHeightResponse = self
            .rpc_call(METHOD_GET_BTC_PRICE_AT_HEIGHT, json!({ "height": height }))
            .with_context(|| {
                format!("failed to get BTC/USD price from espo pricer at height {height}")
            })?;
        parse_price_scaled_decimal(&response.price_scaled)
            .with_context(|| format!("invalid espo pricer price_scaled at height {height}"))
    }
}

impl PriceFeed for EspoPricerPriceFeed {
    fn get_bitcoin_price_usd_at_block_height(&self, height: u64) -> Result<u128> {
        self.price_at_bitcoin_height(height)
    }
}

fn parse_price_scaled_decimal(raw: &str) -> Result<u128> {
    let raw = raw.trim();
    if raw.is_empty() {
        anyhow::bail!("price_scaled must not be empty");
    }
    if raw.starts_with('-') || raw.starts_with('+') {
        anyhow::bail!("price_scaled must be an unsigned decimal");
    }
    let (whole_raw, frac_raw) = raw.split_once('.').unwrap_or((raw, ""));
    if whole_raw.is_empty() || !whole_raw.chars().all(|c| c.is_ascii_digit()) {
        anyhow::bail!("price_scaled has invalid whole-number digits");
    }
    if !frac_raw.chars().all(|c| c.is_ascii_digit()) {
        anyhow::bail!("price_scaled has invalid fractional digits");
    }

    let whole = whole_raw.parse::<u128>().context("failed to parse price_scaled whole number")?;
    let decimals = PRICE_SCALE_DECIMALS as usize;
    let mut frac_scaled = frac_raw.chars().take(decimals).collect::<String>();
    while frac_scaled.len() < decimals {
        frac_scaled.push('0');
    }
    let frac = if frac_scaled.is_empty() {
        0
    } else {
        frac_scaled.parse::<u128>().context("failed to parse price_scaled fraction")?
    };
    let scale = 10u128
        .checked_pow(PRICE_SCALE_DECIMALS)
        .ok_or_else(|| anyhow!("price scale overflow"))?;
    whole
        .checked_mul(scale)
        .and_then(|v| v.checked_add(frac))
        .ok_or_else(|| anyhow!("price_scaled exceeds u128 range"))
}

#[cfg(test)]
mod tests {
    use super::parse_price_scaled_decimal;

    #[test]
    fn parses_espo_pricer_decimal_to_ammdata_scale() {
        assert_eq!(
            parse_price_scaled_decimal("61043.56000000").unwrap(),
            610_435_600_000_000_000_000
        );
        assert_eq!(parse_price_scaled_decimal("1").unwrap(), 10_000_000_000_000_000);
    }
}
