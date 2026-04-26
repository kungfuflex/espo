mod consts;

use anyhow::{Context, anyhow};
use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use bitcoin::{Address, BlockHash, Network, Transaction};
use bitcoincore_rpc::RpcApi;
use serde::{Deserialize, Serialize};

use crate::config::{get_bitcoind_rpc_client, get_network};
use crate::explorer::paths::explorer_path;

use self::consts::{MINING_POOL_DEFINITIONS, MiningPoolDefinition};

const DEFAULT_POOL_NAME: &str = "Unknown";
const DEFAULT_POOL_SLUG: &str = "unknown";

const ANTPOOL_SVG: &str = include_str!("assets/antpool.svg");
const BINANCEPOOL_SVG: &str = include_str!("assets/binancepool.svg");
const BRAIINSPOOL_SVG: &str = include_str!("assets/braiinspool.svg");
const DEFAULT_SVG: &str = include_str!("assets/default.svg");
const F2POOL_SVG: &str = include_str!("assets/f2pool.svg");
const FOUNDRYUSA_SVG: &str = include_str!("assets/foundryusa.svg");
const LUXOR_SVG: &str = include_str!("assets/luxor.svg");
const MARAPOOL_SVG: &str = include_str!("assets/marapool.svg");
const OCEAN_SVG: &str = include_str!("assets/ocean.svg");
const SBICRYPTO_SVG: &str = include_str!("assets/sbicrypto.svg");
const SECPOOL_SVG: &str = include_str!("assets/secpool.svg");
const SPIDERPOOL_SVG: &str = include_str!("assets/spiderpool.svg");
const VIABTC_SVG: &str = include_str!("assets/viabtc.svg");

#[derive(Clone, Serialize)]
pub struct MiningPoolDisplay {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u16>,
    pub name: String,
    pub slug: String,
    pub matched: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mempool_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct BlockMiningPoolResult {
    pub height: u64,
    pub block_hash: String,
    pub pool: MiningPoolDisplay,
}

pub(crate) struct ResolvedBlockMiningPool {
    pub(crate) block_hash: BlockHash,
    pub(crate) pool: MiningPoolDisplay,
    pub(crate) tx_count: usize,
}

#[derive(Deserialize)]
pub struct BlockMiningPoolQuery {
    pub height: Option<u64>,
}

#[derive(Serialize)]
pub struct BlockMiningPoolResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<BlockMiningPoolResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn block_mining_pool_api(
    Query(q): Query<BlockMiningPoolQuery>,
) -> Json<BlockMiningPoolResponse> {
    let Some(height) = q.height else {
        return Json(BlockMiningPoolResponse {
            ok: false,
            result: None,
            error: Some("missing required query parameter: height".to_string()),
        });
    };

    match resolve_block_mining_pool(height) {
        Ok(result) => Json(BlockMiningPoolResponse { ok: true, result: Some(result), error: None }),
        Err(err) => {
            Json(BlockMiningPoolResponse { ok: false, result: None, error: Some(err.to_string()) })
        }
    }
}

pub async fn mining_pool_icon(Path(slug): Path<String>) -> impl IntoResponse {
    let body = match pool_icon_svg(slug.as_str()) {
        Some(svg) => svg,
        None => DEFAULT_SVG,
    };
    (StatusCode::OK, [(CONTENT_TYPE, "image/svg+xml; charset=utf-8")], body)
}

pub fn resolve_block_mining_pool(height: u64) -> anyhow::Result<BlockMiningPoolResult> {
    let resolved = resolve_block_mining_pool_with_tx_count(height)?;
    Ok(BlockMiningPoolResult {
        height,
        block_hash: resolved.block_hash.to_string(),
        pool: resolved.pool,
    })
}

pub(crate) fn resolve_block_mining_pool_with_tx_count(
    height: u64,
) -> anyhow::Result<ResolvedBlockMiningPool> {
    let rpc = get_bitcoind_rpc_client();
    let block_hash = rpc
        .get_block_hash(height)
        .with_context(|| format!("bitcoind get_block_hash({height})"))?;
    let block = rpc
        .get_block(&block_hash)
        .with_context(|| format!("bitcoind get_block({block_hash})"))?;
    let pool = detect_pool_from_block(&block_hash, &block, get_network())
        .context("detect mining pool from coinbase transaction")?;
    Ok(ResolvedBlockMiningPool { block_hash, pool, tx_count: block.txdata.len() })
}

fn detect_pool_from_block(
    _block_hash: &BlockHash,
    block: &bitcoin::Block,
    network: Network,
) -> anyhow::Result<MiningPoolDisplay> {
    let coinbase = block
        .txdata
        .first()
        .ok_or_else(|| anyhow!("block has no coinbase transaction"))?;
    Ok(detect_pool_from_coinbase_tx(coinbase, network))
}

fn detect_pool_from_coinbase_tx(coinbase: &Transaction, network: Network) -> MiningPoolDisplay {
    let coinbase_ascii = coinbase
        .input
        .first()
        .map(|input| String::from_utf8_lossy(input.script_sig.as_bytes()).into_owned())
        .unwrap_or_default();
    let output_addresses = coinbase_output_addresses(coinbase, network);

    match match_definition_from_coinbase_data(&coinbase_ascii, &output_addresses) {
        Some(pool) => MiningPoolDisplay {
            id: Some(pool.id),
            name: pool.name.to_string(),
            slug: pool.slug.to_string(),
            matched: true,
            link: pool.link.map(str::to_string),
            mempool_url: mempool_pool_url(network, pool.slug),
            icon_url: pool_icon_url(pool.slug),
        },
        None => MiningPoolDisplay {
            id: None,
            name: DEFAULT_POOL_NAME.to_string(),
            slug: DEFAULT_POOL_SLUG.to_string(),
            matched: false,
            link: None,
            mempool_url: None,
            icon_url: pool_icon_url(DEFAULT_POOL_SLUG),
        },
    }
}

fn coinbase_output_addresses(coinbase: &Transaction, network: Network) -> Vec<String> {
    let mut addresses: Vec<String> = coinbase
        .output
        .iter()
        .filter_map(|out| Address::from_script(out.script_pubkey.as_script(), network).ok())
        .map(|addr| addr.to_string())
        .collect();
    addresses.sort();
    addresses.dedup();
    addresses
}

fn match_definition_from_coinbase_data(
    coinbase_ascii: &str,
    output_addresses: &[String],
) -> Option<&'static MiningPoolDefinition> {
    for pool in MINING_POOL_DEFINITIONS {
        if !pool.addresses.is_empty()
            && output_addresses
                .iter()
                .any(|addr| pool.addresses.iter().any(|candidate| addr == candidate))
        {
            return Some(pool);
        }
    }

    let coinbase_ascii_lower = coinbase_ascii.to_lowercase();
    for pool in MINING_POOL_DEFINITIONS {
        if pool
            .tags
            .iter()
            .filter(|tag| !tag.is_empty())
            .any(|tag| coinbase_ascii_lower.contains(&tag.to_lowercase()))
        {
            return Some(pool);
        }
    }

    None
}

pub fn pool_icon_url(slug: &str) -> Option<String> {
    let icon_slug = if slug == DEFAULT_POOL_SLUG || has_pool_icon(slug) { slug } else { "default" };
    Some(explorer_path(&format!("/static/mining-pools/{icon_slug}")))
}

pub(crate) fn bundled_pool_icon_svgs_json() -> String {
    serde_json::json!({
        "antpool": ANTPOOL_SVG,
        "binancepool": BINANCEPOOL_SVG,
        "braiinspool": BRAIINSPOOL_SVG,
        "default": DEFAULT_SVG,
        "f2pool": F2POOL_SVG,
        "foundryusa": FOUNDRYUSA_SVG,
        "luxor": LUXOR_SVG,
        "marapool": MARAPOOL_SVG,
        "ocean": OCEAN_SVG,
        "sbicrypto": SBICRYPTO_SVG,
        "secpool": SECPOOL_SVG,
        "spiderpool": SPIDERPOOL_SVG,
        "unknown": DEFAULT_SVG,
        "viabtc": VIABTC_SVG,
    })
    .to_string()
}

fn mempool_pool_url(network: Network, slug: &str) -> Option<String> {
    let base = match network {
        Network::Bitcoin => "https://mempool.space",
        Network::Testnet => "https://mempool.space/testnet",
        Network::Signet => "https://mempool.space/signet",
        Network::Regtest => return None,
        _ => "https://mempool.space",
    };
    Some(format!("{base}/mining/pool/{slug}"))
}

fn has_pool_icon(slug: &str) -> bool {
    matches!(
        slug,
        "antpool"
            | "binancepool"
            | "braiinspool"
            | "f2pool"
            | "foundryusa"
            | "luxor"
            | "marapool"
            | "ocean"
            | "sbicrypto"
            | "secpool"
            | "spiderpool"
            | "viabtc"
    )
}

fn pool_icon_svg(slug: &str) -> Option<&'static str> {
    match slug {
        "antpool" => Some(ANTPOOL_SVG),
        "binancepool" => Some(BINANCEPOOL_SVG),
        "braiinspool" => Some(BRAIINSPOOL_SVG),
        "default" => Some(DEFAULT_SVG),
        "f2pool" => Some(F2POOL_SVG),
        "foundryusa" => Some(FOUNDRYUSA_SVG),
        "luxor" => Some(LUXOR_SVG),
        "marapool" => Some(MARAPOOL_SVG),
        "ocean" => Some(OCEAN_SVG),
        "sbicrypto" => Some(SBICRYPTO_SVG),
        "secpool" => Some(SECPOOL_SVG),
        "spiderpool" => Some(SPIDERPOOL_SVG),
        "unknown" => Some(DEFAULT_SVG),
        "viabtc" => Some(VIABTC_SVG),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::match_definition_from_coinbase_data;

    #[test]
    fn matches_coinbase_tag_case_insensitively() {
        let matched = match_definition_from_coinbase_data("hello /luxor/ world", &[])
            .expect("matches luxor coinbase tag");
        assert_eq!(matched.slug, "luxor");
    }

    #[test]
    fn matches_coinbase_output_address() {
        let addresses = vec!["bc1qxhmdufsvnuaaaer4ynz88fspdsxq2h9e9cetdj".to_string()];
        let matched = match_definition_from_coinbase_data("", &addresses)
            .expect("matches foundry pool by output address");
        assert_eq!(matched.slug, "foundryusa");
    }
}
