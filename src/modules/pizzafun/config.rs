use crate::schemas::SchemaAlkaneId;
use anyhow::{Result, anyhow};
use serde::Deserialize;
use std::net::{IpAddr, Ipv4Addr};

fn default_snapshot_page_limit() -> u64 {
    512
}

fn default_snapshot_page_limit_max() -> u64 {
    2048
}

fn default_snapshot_http_host() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

fn default_snapshot_http_port() -> u16 {
    8081
}

fn default_snapshot_http_base_path() -> String {
    "/pizzafun/snapshot".to_string()
}

#[derive(Debug, Clone, Deserialize)]
struct RawPizzafunConfig {
    factory_id: String,
    #[serde(default = "default_snapshot_page_limit")]
    snapshot_page_limit: u64,
    #[serde(default = "default_snapshot_page_limit_max")]
    snapshot_page_limit_max: u64,
    #[serde(default = "default_snapshot_http_host")]
    snapshot_http_host: IpAddr,
    #[serde(default = "default_snapshot_http_port")]
    snapshot_http_port: u16,
    #[serde(default = "default_snapshot_http_base_path")]
    snapshot_http_base_path: String,
}

#[derive(Debug, Clone)]
pub struct PizzafunConfig {
    pub factory_id: SchemaAlkaneId,
    pub snapshot_page_limit: u64,
    pub snapshot_page_limit_max: u64,
    pub snapshot_http_host: IpAddr,
    pub snapshot_http_port: u16,
    pub snapshot_http_base_path: String,
}

impl PizzafunConfig {
    pub fn spec() -> &'static str {
        "{ factory_id: \"<block>:<tx>\", snapshot_page_limit?: number, snapshot_page_limit_max?: number, snapshot_http_host?: string, snapshot_http_port?: number, snapshot_http_base_path?: string }"
    }

    pub fn from_value(value: &serde_json::Value) -> Result<Self> {
        let raw: RawPizzafunConfig = serde_json::from_value(value.clone())
            .map_err(|e| anyhow!("invalid pizzafun config: {e}"))?;
        let factory_id = parse_alkane_id(&raw.factory_id)
            .ok_or_else(|| anyhow!("invalid factory_id, expected <block>:<tx>"))?;
        let snapshot_page_limit = raw.snapshot_page_limit.max(1);
        let snapshot_page_limit_max = raw.snapshot_page_limit_max.max(snapshot_page_limit);
        let mut snapshot_http_base_path = raw.snapshot_http_base_path.trim().to_string();
        if snapshot_http_base_path.is_empty() || snapshot_http_base_path == "/" {
            snapshot_http_base_path = "/pizzafun/snapshot".to_string();
        } else {
            if !snapshot_http_base_path.starts_with('/') {
                snapshot_http_base_path.insert(0, '/');
            }
            while snapshot_http_base_path.ends_with('/') && snapshot_http_base_path.len() > 1 {
                snapshot_http_base_path.pop();
            }
        }

        Ok(Self {
            factory_id,
            snapshot_page_limit,
            snapshot_page_limit_max,
            snapshot_http_host: raw.snapshot_http_host,
            snapshot_http_port: raw.snapshot_http_port,
            snapshot_http_base_path,
        })
    }
}

fn parse_alkane_id(value: &str) -> Option<SchemaAlkaneId> {
    let (block_raw, tx_raw) = value.trim().split_once(':')?;
    let parse_u32 = |v: &str| {
        if let Some(hex) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
            u32::from_str_radix(hex, 16).ok()
        } else {
            v.parse::<u32>().ok()
        }
    };
    let parse_u64 = |v: &str| {
        if let Some(hex) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
            u64::from_str_radix(hex, 16).ok()
        } else {
            v.parse::<u64>().ok()
        }
    };
    Some(SchemaAlkaneId { block: parse_u32(block_raw)?, tx: parse_u64(tx_raw)? })
}
