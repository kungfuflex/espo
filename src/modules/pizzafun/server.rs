use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;

use crate::runtime::state_at::StateAt;

use super::config::PizzafunConfig;
use super::snapshot::{BondedSnapshotMetaV1, BondedSnapshotPageV1, BondedSnapshotRowV1};
use super::storage::PizzafunProvider;

#[derive(Clone)]
pub struct SnapshotHttpState {
    pub config: PizzafunConfig,
    pub provider: Arc<PizzafunProvider>,
}

#[derive(Deserialize)]
struct PageQuery {
    root_hash: String,
    offset: Option<u64>,
    limit: Option<u64>,
}

pub async fn run(addr: SocketAddr, state: SnapshotHttpState) -> Result<()> {
    let meta_path = format!("{}/meta", state.config.snapshot_http_base_path);
    let page_path = format!("{}/page", state.config.snapshot_http_base_path);

    let app = Router::new()
        .route(meta_path.as_str(), get(get_meta))
        .route(page_path.as_str(), get(get_page))
        .with_state(state);

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn get_meta(State(state): State<SnapshotHttpState>) -> Response {
    match snapshot_meta(&state) {
        Ok(meta) => bytes_response(StatusCode::OK, &borsh::to_vec(&meta).unwrap_or_default()),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

async fn get_page(
    State(state): State<SnapshotHttpState>,
    Query(query): Query<PageQuery>,
) -> Response {
    let requested_hash = match parse_root_hash_hex(&query.root_hash) {
        Some(value) => value,
        None => return (StatusCode::BAD_REQUEST, "invalid_root_hash").into_response(),
    };

    let rows = match state.provider.get_all_bonded_rows(StateAt::Latest) {
        Ok(rows) => rows,
        Err(err) => return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    };
    let actual_hash = compute_root_hash(&rows);
    if actual_hash != requested_hash {
        return (StatusCode::CONFLICT, "root_hash_mismatch").into_response();
    }

    let offset = query.offset.unwrap_or(0);
    let mut limit = query.limit.unwrap_or(state.config.snapshot_page_limit).max(1);
    if limit > state.config.snapshot_page_limit_max {
        limit = state.config.snapshot_page_limit_max;
    }

    let total = rows.len() as u64;
    let start = usize::try_from(offset.min(total)).unwrap_or(usize::MAX);
    let end = start.saturating_add(limit as usize).min(rows.len());
    let entries = if start >= rows.len() { Vec::new() } else { rows[start..end].to_vec() };

    let page = BondedSnapshotPageV1 { root_hash: actual_hash, offset, limit, total, entries };
    match borsh::to_vec(&page) {
        Ok(bytes) => bytes_response(StatusCode::OK, &bytes),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

pub fn snapshot_meta(state: &SnapshotHttpState) -> Result<BondedSnapshotMetaV1> {
    let rows = state.provider.get_all_bonded_rows(StateAt::Latest)?;
    let height = state
        .provider
        .get_index_height(super::storage::GetIndexHeightParams { blockhash: StateAt::Latest })?
        .height
        .unwrap_or(0) as u64;
    Ok(BondedSnapshotMetaV1 {
        root_hash: compute_root_hash(&rows),
        height,
        total: rows.len() as u64,
    })
}

fn compute_root_hash(rows: &[BondedSnapshotRowV1]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for row in rows {
        if let Ok(bytes) = borsh::to_vec(row) {
            hasher.update((bytes.len() as u64).to_be_bytes());
            hasher.update(bytes);
        }
    }
    hasher.finalize().into()
}

fn parse_root_hash_hex(raw: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(raw.trim()).ok()?;
    let arr: [u8; 32] = bytes.try_into().ok()?;
    Some(arr)
}

fn bytes_response(status: StatusCode, bytes: &[u8]) -> Response {
    let mut response = (status, bytes.to_vec()).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    response
}
