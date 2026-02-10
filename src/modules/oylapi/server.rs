use crate::modules::oylapi::storage::{
    GetAlkanesParams, OylApiState, get_address_pool_burn_history,
    get_address_pool_creation_history, get_address_pool_mint_history, get_address_positions,
    get_address_swap_history_for_pool, get_address_swap_history_for_token,
    get_address_unwrap_history, get_address_utxos_portfolio, get_address_wrap_history,
    get_alkane_details, get_alkane_swap_pair_details, get_alkanes, get_alkanes_by_address,
    get_alkanes_utxo, get_all_address_amm_tx_history, get_all_amm_tx_history,
    get_all_pools_details, get_all_token_pairs, get_all_unwrap_history, get_all_wrap_history,
    get_amm_utxos, get_bitcoin_price, get_global_alkanes_search, get_pool_burn_history,
    get_pool_creation_history, get_pool_details, get_pool_mint_history, get_pool_swap_history,
    get_pools, get_token_pairs, get_token_swap_history, get_total_unwrap_amount,
};
use axum::{Json, Router, extract::State, routing::post};
use serde::Deserialize;
use serde_json::Value;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};

#[derive(Deserialize)]
struct AddressRequest {
    address: String,
}

#[derive(Deserialize)]
struct AmmUtxosRequest {
    address: String,
    #[serde(rename = "spendStrategy")]
    spend_strategy: Option<Value>,
}

#[derive(Deserialize)]
struct AddressUtxosRequest {
    address: String,
    #[serde(rename = "spendStrategy")]
    spend_strategy: Option<Value>,
}

#[derive(Deserialize)]
struct GetAlkanesRequest {
    limit: u64,
    offset: Option<u64>,
    sort_by: Option<String>,
    order: Option<String>,
    #[serde(rename = "searchQuery")]
    search_query: Option<String>,
}

#[derive(Deserialize)]
struct SearchRequest {
    #[serde(rename = "searchQuery")]
    search_query: String,
}

#[derive(Deserialize)]
struct AlkaneIdRequest {
    block: String,
    tx: String,
}

#[derive(Deserialize)]
struct AlkaneDetailsRequest {
    #[serde(rename = "alkaneId")]
    alkane_id: AlkaneIdRequest,
}

#[derive(Deserialize)]
struct GetPoolsRequest {
    #[serde(rename = "factoryId")]
    factory_id: AlkaneIdRequest,
    limit: Option<u64>,
    offset: Option<u64>,
}

#[derive(Deserialize)]
struct PoolDetailsRequest {
    #[serde(rename = "factoryId")]
    factory_id: AlkaneIdRequest,
    #[serde(rename = "poolId")]
    pool_id: AlkaneIdRequest,
}

#[derive(Deserialize)]
struct AddressPositionsRequest {
    address: String,
    #[serde(rename = "factoryId")]
    factory_id: AlkaneIdRequest,
}

#[derive(Deserialize)]
struct PoolHistoryRequest {
    #[serde(rename = "poolId")]
    pool_id: AlkaneIdRequest,
    count: Option<u64>,
    offset: Option<u64>,
    successful: Option<bool>,
    #[serde(rename = "includeTotal")]
    include_total: Option<bool>,
}

#[derive(Deserialize)]
struct TokenHistoryRequest {
    #[serde(rename = "tokenId")]
    token_id: AlkaneIdRequest,
    count: Option<u64>,
    offset: Option<u64>,
    successful: Option<bool>,
    #[serde(rename = "includeTotal")]
    include_total: Option<bool>,
}

#[derive(Deserialize)]
struct PoolCreationHistoryRequest {
    #[serde(rename = "poolId")]
    pool_id: Option<AlkaneIdRequest>,
    count: Option<u64>,
    offset: Option<u64>,
    successful: Option<bool>,
    #[serde(rename = "includeTotal")]
    include_total: Option<bool>,
}

#[derive(Deserialize)]
struct AddressPoolSwapHistoryRequest {
    address: String,
    #[serde(rename = "poolId")]
    pool_id: AlkaneIdRequest,
    count: Option<u64>,
    offset: Option<u64>,
    successful: Option<bool>,
    #[serde(rename = "includeTotal")]
    include_total: Option<bool>,
}

#[derive(Deserialize)]
struct AddressTokenSwapHistoryRequest {
    address: String,
    #[serde(rename = "tokenId")]
    token_id: AlkaneIdRequest,
    count: Option<u64>,
    offset: Option<u64>,
    successful: Option<bool>,
    #[serde(rename = "includeTotal")]
    include_total: Option<bool>,
}

#[derive(Deserialize)]
struct AddressWrapHistoryRequest {
    address: String,
    count: Option<u64>,
    offset: Option<u64>,
    successful: Option<bool>,
    #[serde(rename = "includeTotal")]
    include_total: Option<bool>,
}

#[derive(Deserialize)]
struct AllWrapHistoryRequest {
    count: Option<u64>,
    offset: Option<u64>,
    successful: Option<bool>,
    #[serde(rename = "includeTotal")]
    include_total: Option<bool>,
}

#[derive(Deserialize)]
struct AddressPoolCreationHistoryRequest {
    address: String,
    #[serde(rename = "poolId")]
    pool_id: Option<AlkaneIdRequest>,
    count: Option<u64>,
    offset: Option<u64>,
    successful: Option<bool>,
    #[serde(rename = "includeTotal")]
    include_total: Option<bool>,
}

#[derive(Deserialize)]
struct AddressPoolMintBurnHistoryRequest {
    address: String,
    count: Option<u64>,
    offset: Option<u64>,
    successful: Option<bool>,
    #[serde(rename = "includeTotal")]
    include_total: Option<bool>,
}

#[derive(Deserialize)]
struct TotalUnwrapAmountRequest {
    #[serde(rename = "blockHeight")]
    block_height: Option<u32>,
    successful: Option<bool>,
}

#[derive(Deserialize)]
struct GetAllPoolsDetailsRequest {
    #[serde(rename = "factoryId")]
    factory_id: AlkaneIdRequest,
    limit: Option<u64>,
    offset: Option<u64>,
    sort_by: Option<String>,
    order: Option<String>,
    address: Option<String>,
    #[serde(rename = "searchQuery")]
    search_query: Option<String>,
}

#[derive(Deserialize)]
struct AllAddressAmmTxHistoryRequest {
    address: String,
    #[serde(rename = "poolId")]
    pool_id: Option<AlkaneIdRequest>,
    #[serde(rename = "transactionType")]
    transaction_type: Option<String>,
    count: Option<u64>,
    offset: Option<u64>,
    successful: Option<bool>,
    #[serde(rename = "includeTotal")]
    include_total: Option<bool>,
}

#[derive(Deserialize)]
struct AllAmmTxHistoryRequest {
    #[serde(rename = "poolId")]
    pool_id: Option<AlkaneIdRequest>,
    #[serde(rename = "transactionType")]
    transaction_type: Option<String>,
    count: Option<u64>,
    offset: Option<u64>,
    successful: Option<bool>,
    #[serde(rename = "includeTotal")]
    include_total: Option<bool>,
}

#[derive(Deserialize)]
struct GetAllTokenPairsRequest {
    #[serde(rename = "factoryId")]
    factory_id: AlkaneIdRequest,
}

#[derive(Deserialize)]
struct GetTokenPairsRequest {
    #[serde(rename = "factoryId")]
    factory_id: AlkaneIdRequest,
    #[serde(rename = "alkaneId")]
    alkane_id: AlkaneIdRequest,
    sort_by: Option<String>,
    limit: Option<u64>,
    offset: Option<u64>,
    #[serde(rename = "searchQuery")]
    search_query: Option<String>,
}

#[derive(Deserialize)]
struct GetAlkaneSwapPairDetailsRequest {
    #[serde(rename = "factoryId")]
    factory_id: AlkaneIdRequest,
    #[serde(rename = "tokenAId")]
    token_a_id: AlkaneIdRequest,
    #[serde(rename = "tokenBId")]
    token_b_id: AlkaneIdRequest,
}

pub fn router(state: OylApiState) -> Router {
    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);
    Router::new()
        .route("/get-alkanes-by-address", post(get_alkanes_by_address_handler))
        .route("/get-bitcoin-price", post(get_bitcoin_price_handler))
        .route("/get-alkanes-utxo", post(get_alkanes_utxo_handler))
        .route("/get-address-utxos", post(get_address_utxos_handler))
        .route("/get-account-utxos", post(get_address_utxos_handler))
        .route("/get-amm-utxos", post(get_amm_utxos_handler))
        .route("/get-alkanes", post(get_alkanes_handler))
        .route("/global-alkanes-search", post(global_alkanes_search_handler))
        .route("/get-alkane-details", post(get_alkane_details_handler))
        .route("/get-pools", post(get_pools_handler))
        .route("/get-pool-details", post(get_pool_details_handler))
        .route("/get-pool-swap-history", post(get_pool_swap_history_handler))
        .route("/get-token-swap-history", post(get_token_swap_history_handler))
        .route("/get-pool-mint-history", post(get_pool_mint_history_handler))
        .route("/get-pool-burn-history", post(get_pool_burn_history_handler))
        .route("/get-pool-creation-history", post(get_pool_creation_history_handler))
        .route(
            "/get-address-swap-history-for-pool",
            post(get_address_swap_history_for_pool_handler),
        )
        .route(
            "/get-address-swap-history-for-token",
            post(get_address_swap_history_for_token_handler),
        )
        .route("/get-address-wrap-history", post(get_address_wrap_history_handler))
        .route("/get-address-unwrap-history", post(get_address_unwrap_history_handler))
        .route("/get-all-wrap-history", post(get_all_wrap_history_handler))
        .route("/get-all-unwrap-history", post(get_all_unwrap_history_handler))
        .route("/get-total-unwrap-amount", post(get_total_unwrap_amount_handler))
        .route(
            "/get-address-pool-creation-history",
            post(get_address_pool_creation_history_handler),
        )
        .route("/get-address-pool-mint-history", post(get_address_pool_mint_history_handler))
        .route("/get-address-pool-burn-history", post(get_address_pool_burn_history_handler))
        .route("/address-positions", post(get_address_positions_handler))
        .route("/get-all-pools-details", post(get_all_pools_details_handler))
        .route("/get-all-address-amm-tx-history", post(get_all_address_amm_tx_history_handler))
        .route("/get-all-amm-tx-history", post(get_all_amm_tx_history_handler))
        .route("/get-all-token-pairs", post(get_all_token_pairs_handler))
        .route("/get-token-pairs", post(get_token_pairs_handler))
        .route("/get-alkane-swap-pair-details", post(get_alkane_swap_pair_details_handler))
        .with_state(state)
        .layer(cors)
}

pub async fn run(addr: SocketAddr, state: OylApiState) -> anyhow::Result<()> {
    let app = router(state);
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

async fn get_alkanes_by_address_handler(
    State(state): State<OylApiState>,
    Json(req): Json<AddressRequest>,
) -> Json<Value> {
    Json(get_alkanes_by_address(&state, &req.address).await)
}

async fn get_bitcoin_price_handler(State(state): State<OylApiState>) -> Json<Value> {
    Json(get_bitcoin_price(&state).await)
}

async fn get_alkanes_utxo_handler(
    State(state): State<OylApiState>,
    Json(req): Json<AddressRequest>,
) -> Json<Value> {
    Json(get_alkanes_utxo(&state, &req.address).await)
}

async fn get_address_utxos_handler(
    State(state): State<OylApiState>,
    Json(req): Json<AddressUtxosRequest>,
) -> Json<Value> {
    Json(get_address_utxos_portfolio(&state, &req.address, req.spend_strategy).await)
}

async fn get_amm_utxos_handler(
    State(state): State<OylApiState>,
    Json(req): Json<AmmUtxosRequest>,
) -> Json<Value> {
    Json(get_amm_utxos(&state, &req.address, req.spend_strategy).await)
}

async fn get_alkanes_handler(
    State(state): State<OylApiState>,
    Json(req): Json<GetAlkanesRequest>,
) -> Json<Value> {
    let params = GetAlkanesParams {
        limit: req.limit,
        offset: req.offset,
        sort_by: req.sort_by,
        order: req.order,
        search_query: req.search_query,
    };
    Json(get_alkanes(&state, params).await)
}

async fn global_alkanes_search_handler(
    State(state): State<OylApiState>,
    Json(req): Json<SearchRequest>,
) -> Json<Value> {
    Json(get_global_alkanes_search(&state, &req.search_query).await)
}

async fn get_alkane_details_handler(
    State(state): State<OylApiState>,
    Json(req): Json<AlkaneDetailsRequest>,
) -> Json<Value> {
    Json(get_alkane_details(&state, &req.alkane_id.block, &req.alkane_id.tx).await)
}

async fn get_pools_handler(
    State(state): State<OylApiState>,
    Json(req): Json<GetPoolsRequest>,
) -> Json<Value> {
    Json(get_pools(&state, &req.factory_id.block, &req.factory_id.tx, req.limit, req.offset).await)
}

async fn get_pool_details_handler(
    State(state): State<OylApiState>,
    Json(req): Json<PoolDetailsRequest>,
) -> Json<Value> {
    Json(
        get_pool_details(
            &state,
            &req.factory_id.block,
            &req.factory_id.tx,
            &req.pool_id.block,
            &req.pool_id.tx,
        )
        .await,
    )
}

async fn get_pool_swap_history_handler(
    State(state): State<OylApiState>,
    Json(req): Json<PoolHistoryRequest>,
) -> Json<Value> {
    Json(
        get_pool_swap_history(
            &state,
            &req.pool_id.block,
            &req.pool_id.tx,
            req.count,
            req.offset,
            req.successful,
            req.include_total,
        )
        .await,
    )
}

async fn get_token_swap_history_handler(
    State(state): State<OylApiState>,
    Json(req): Json<TokenHistoryRequest>,
) -> Json<Value> {
    Json(
        get_token_swap_history(
            &state,
            &req.token_id.block,
            &req.token_id.tx,
            req.count,
            req.offset,
            req.successful,
            req.include_total,
        )
        .await,
    )
}

async fn get_pool_mint_history_handler(
    State(state): State<OylApiState>,
    Json(req): Json<PoolHistoryRequest>,
) -> Json<Value> {
    Json(
        get_pool_mint_history(
            &state,
            &req.pool_id.block,
            &req.pool_id.tx,
            req.count,
            req.offset,
            req.successful,
            req.include_total,
        )
        .await,
    )
}

async fn get_pool_burn_history_handler(
    State(state): State<OylApiState>,
    Json(req): Json<PoolHistoryRequest>,
) -> Json<Value> {
    Json(
        get_pool_burn_history(
            &state,
            &req.pool_id.block,
            &req.pool_id.tx,
            req.count,
            req.offset,
            req.successful,
            req.include_total,
        )
        .await,
    )
}

async fn get_pool_creation_history_handler(
    State(state): State<OylApiState>,
    Json(req): Json<PoolCreationHistoryRequest>,
) -> Json<Value> {
    let _ = &req.pool_id;
    Json(
        get_pool_creation_history(&state, req.count, req.offset, req.successful, req.include_total)
            .await,
    )
}

async fn get_address_swap_history_for_pool_handler(
    State(state): State<OylApiState>,
    Json(req): Json<AddressPoolSwapHistoryRequest>,
) -> Json<Value> {
    Json(
        get_address_swap_history_for_pool(
            &state,
            &req.address,
            &req.pool_id.block,
            &req.pool_id.tx,
            req.count,
            req.offset,
            req.successful,
            req.include_total,
        )
        .await,
    )
}

async fn get_address_swap_history_for_token_handler(
    State(state): State<OylApiState>,
    Json(req): Json<AddressTokenSwapHistoryRequest>,
) -> Json<Value> {
    Json(
        get_address_swap_history_for_token(
            &state,
            &req.address,
            &req.token_id.block,
            &req.token_id.tx,
            req.count,
            req.offset,
            req.successful,
            req.include_total,
        )
        .await,
    )
}

async fn get_address_wrap_history_handler(
    State(state): State<OylApiState>,
    Json(req): Json<AddressWrapHistoryRequest>,
) -> Json<Value> {
    Json(
        get_address_wrap_history(
            &state,
            &req.address,
            req.count,
            req.offset,
            req.successful,
            req.include_total,
        )
        .await,
    )
}

async fn get_address_unwrap_history_handler(
    State(state): State<OylApiState>,
    Json(req): Json<AddressWrapHistoryRequest>,
) -> Json<Value> {
    Json(
        get_address_unwrap_history(
            &state,
            &req.address,
            req.count,
            req.offset,
            req.successful,
            req.include_total,
        )
        .await,
    )
}

async fn get_all_wrap_history_handler(
    State(state): State<OylApiState>,
    Json(req): Json<AllWrapHistoryRequest>,
) -> Json<Value> {
    Json(
        get_all_wrap_history(&state, req.count, req.offset, req.successful, req.include_total)
            .await,
    )
}

async fn get_all_unwrap_history_handler(
    State(state): State<OylApiState>,
    Json(req): Json<AllWrapHistoryRequest>,
) -> Json<Value> {
    Json(
        get_all_unwrap_history(&state, req.count, req.offset, req.successful, req.include_total)
            .await,
    )
}

async fn get_total_unwrap_amount_handler(
    State(state): State<OylApiState>,
    Json(req): Json<TotalUnwrapAmountRequest>,
) -> Json<Value> {
    Json(get_total_unwrap_amount(&state, req.block_height, req.successful).await)
}

async fn get_address_pool_creation_history_handler(
    State(state): State<OylApiState>,
    Json(req): Json<AddressPoolCreationHistoryRequest>,
) -> Json<Value> {
    let _ = &req.pool_id;
    Json(
        get_address_pool_creation_history(
            &state,
            &req.address,
            req.count,
            req.offset,
            req.successful,
            req.include_total,
        )
        .await,
    )
}

async fn get_address_pool_mint_history_handler(
    State(state): State<OylApiState>,
    Json(req): Json<AddressPoolMintBurnHistoryRequest>,
) -> Json<Value> {
    Json(
        get_address_pool_mint_history(
            &state,
            &req.address,
            req.count,
            req.offset,
            req.successful,
            req.include_total,
        )
        .await,
    )
}

async fn get_address_pool_burn_history_handler(
    State(state): State<OylApiState>,
    Json(req): Json<AddressPoolMintBurnHistoryRequest>,
) -> Json<Value> {
    Json(
        get_address_pool_burn_history(
            &state,
            &req.address,
            req.count,
            req.offset,
            req.successful,
            req.include_total,
        )
        .await,
    )
}

async fn get_address_positions_handler(
    State(state): State<OylApiState>,
    Json(req): Json<AddressPositionsRequest>,
) -> Json<Value> {
    Json(
        get_address_positions(&state, &req.address, &req.factory_id.block, &req.factory_id.tx)
            .await,
    )
}

async fn get_all_pools_details_handler(
    State(state): State<OylApiState>,
    Json(req): Json<GetAllPoolsDetailsRequest>,
) -> Json<Value> {
    Json(
        get_all_pools_details(
            &state,
            &req.factory_id.block,
            &req.factory_id.tx,
            req.limit,
            req.offset,
            req.sort_by,
            req.order,
            req.address,
            req.search_query,
        )
        .await,
    )
}

async fn get_all_address_amm_tx_history_handler(
    State(state): State<OylApiState>,
    Json(req): Json<AllAddressAmmTxHistoryRequest>,
) -> Json<Value> {
    let _ = req.pool_id;
    Json(
        get_all_address_amm_tx_history(
            &state,
            &req.address,
            req.transaction_type,
            req.count,
            req.offset,
            req.successful,
            req.include_total,
        )
        .await,
    )
}

async fn get_all_amm_tx_history_handler(
    State(state): State<OylApiState>,
    Json(req): Json<AllAmmTxHistoryRequest>,
) -> Json<Value> {
    let _ = req.pool_id;
    Json(
        get_all_amm_tx_history(
            &state,
            req.transaction_type,
            req.count,
            req.offset,
            req.successful,
            req.include_total,
        )
        .await,
    )
}

async fn get_all_token_pairs_handler(
    State(state): State<OylApiState>,
    Json(req): Json<GetAllTokenPairsRequest>,
) -> Json<Value> {
    Json(get_all_token_pairs(&state, &req.factory_id.block, &req.factory_id.tx).await)
}

async fn get_token_pairs_handler(
    State(state): State<OylApiState>,
    Json(req): Json<GetTokenPairsRequest>,
) -> Json<Value> {
    Json(
        get_token_pairs(
            &state,
            &req.factory_id.block,
            &req.factory_id.tx,
            &req.alkane_id.block,
            &req.alkane_id.tx,
            req.sort_by,
            req.limit,
            req.offset,
            req.search_query,
        )
        .await,
    )
}

async fn get_alkane_swap_pair_details_handler(
    State(state): State<OylApiState>,
    Json(req): Json<GetAlkaneSwapPairDetailsRequest>,
) -> Json<Value> {
    Json(
        get_alkane_swap_pair_details(
            &state,
            &req.factory_id.block,
            &req.factory_id.tx,
            &req.token_a_id.block,
            &req.token_a_id.tx,
            &req.token_b_id.block,
            &req.token_b_id.tx,
        )
        .await,
    )
}
