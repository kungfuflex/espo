mod api;
pub mod components;
pub mod consts;
mod pages;
pub mod paths;

use std::net::SocketAddr;

use api::{alkane_chart, carousel_blocks, search_guess, simulate_contract};
use axum::Router;
use axum::routing::{get, post};
use pages::address::address_page;
use pages::alkane::alkane_page;
use pages::alkanes::alkanes_page;
use pages::block::block_page;
use pages::home::home_page;
use pages::search::search;
use pages::state::ExplorerState;
use pages::tx::tx_page;
use tokio::net::TcpListener;

use crate::config::get_explorer_base_path;
use components::layout::style;

pub fn explorer_router(state: ExplorerState) -> Router {
    Router::new()
        .route("/", get(home_page))
        .route("/search", get(search))
        .route("/block/{height}", get(block_page))
        .route("/tx/{txid}", get(tx_page))
        .route("/address/{address}", get(address_page))
        .route("/alkane/{alkane}", get(alkane_page))
        .route("/alkanes", get(alkanes_page))
        .route("/api/blocks/carousel", get(carousel_blocks))
        .route("/api/search/guess", get(search_guess))
        .route("/api/alkane/simulate", post(simulate_contract))
        .route("/api/alkane/chart", get(alkane_chart))
        .route("/static/style.css", get(style))
        .with_state(state)
}

pub async fn run_explorer(addr: SocketAddr) -> anyhow::Result<()> {
    let state = ExplorerState::new();
    let base_path = get_explorer_base_path();
    let app = if base_path == "/" {
        explorer_router(state)
    } else {
        Router::new().nest(base_path, explorer_router(state))
    };
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}
