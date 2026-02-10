/*
Alkanes endpoints and types map
works - /get-alkanes-by-address -> OylApiGetAlkanesByAddressRequestParams, OylApiGetAlkanesByAddressResponse
works - /get-alkanes-utxo -> OylApiGetAlkanesUtxoRequestParams, OylApiGetAlkanesUtxoResponse
works - /get-amm-utxos -> OylApiGetAmmUtxosRequestParams, OylApiGetAmmUtxosResponse
works - /get-alkanes -> OylApiGetAlkanesRequestParams, OylApiGetAlkanesResponse
works - /global-alkanes-search -> OylApiGlobalAlkanesSearchRequestParams, OylApiGlobalAlkanesSearchResponse
works - /get-alkane-details -> OylApiGetAlkaneDetailsRequestParams, OylApiGetAlkaneDetailsResponse
works - /get-pools -> OylApiGetPoolsRequestParams, OylApiGetPoolsResponse
works - /get-pool-details -> OylApiGetPoolDetailsRequestParams, OylApiGetPoolDetailsResponse
works - /address-positions -> OylApiAddressPositionsRequestParams, OylApiAddressPositionsResponse
works - /get-all-pools-details -> OylApiGetAllPoolsDetailsRequestParams, OylApiGetAllPoolsDetailsResponse
works - /get-pool-swap-history -> OylApiGetPoolSwapHistoryRequestParams, OylApiGetPoolSwapHistoryResponse
works - /get-token-swap-history -> OylApiGetTokenSwapHistoryRequestParams, OylApiGetTokenSwapHistoryResponse
works - /get-pool-mint-history -> OylApiGetPoolMintHistoryRequestParams, OylApiGetPoolMintHistoryResponse
works - /get-pool-burn-history -> OylApiGetPoolBurnHistoryRequestParams, OylApiGetPoolBurnHistoryResponse
works - /get-pool-creation-history -> OylApiGetPoolCreationHistoryRequestParams, OylApiGetPoolCreationHistoryResponse
works - /get-address-swap-history-for-pool -> OylApiGetAddressSwapHistoryForPoolRequestParams, OylApiGetAddressSwapHistoryForPoolResponse
works - /get-address-swap-history-for-token -> OylApiGetAddressSwapHistoryForTokenRequestParams, OylApiGetAddressSwapHistoryForTokenResponse
works - /get-address-wrap-history -> OylApiGetAddressWrapHistoryRequestParams, OylApiGetAddressWrapHistoryResponse
works - /get-address-unwrap-history -> OylApiGetAddressUnwrapHistoryRequestParams, OylApiGetAddressUnwrapHistoryResponse
works - /get-all-wrap-history -> OylApiGetAllWrapHistoryRequestParams, OylApiGetAllWrapHistoryResponse
works - /get-all-unwrap-history -> OylApiGetAllUnwrapHistoryRequestParams, OylApiGetAllUnwrapHistoryResponse
works - /get-total-unwrap-amount -> OylApiGetTotalUnwrapAmountRequestParams, OylApiGetTotalUnwrapAmountResponse
works - /get-address-pool-creation-history -> OylApiGetAddressPoolCreationHistoryRequestParams, OylApiGetAddressPoolCreationHistoryResponse
works - /get-address-pool-mint-history -> OylApiGetAddressPoolMintHistoryRequestParams, OylApiGetAddressPoolMintHistoryResponse
works - /get-address-pool-burn-history -> OylApiGetAddressPoolBurnHistoryRequestParams, OylApiGetAddressPoolBurnHistoryResponse
works - /get-all-address-amm-tx-history -> OylApiGetAllAddressAmmTxHistoryRequestParams, OylApiGetAllAddressAmmTxHistoryResponse
works - /get-all-amm-tx-history -> OylApiGetAllAmmTxHistoryRequestParams, OylApiGetAllAmmTxHistoryResponse
works - /get-all-token-pairs -> OylApiGetAllTokenPairsRequestParams, OylApiGetAllTokenPairsResponse
works (possibly slow?)- /get-token-pairs -> OylApiGetTokenPairsRequestParams, OylApiGetTokenPairsResponse
?? (waiting for tip to populate reserves snapshot)- /get-alkane-swap-pair-details -> OylApiGetAlkaneSwapPairDetailsRequestParams, OylApiGetAlkaneSwapPairDetailsResponse
Notes:
- Error responses are typically `{ error: string, stack?: string }` without a `statusCode`.
*/

export type NumericString = string; // Base-10 numeric string (no separators); usually a raw integer in smallest units.
export type NumericLike = number | NumericString; // Number or numeric string; units depend on field.
export type IsoDateString = string; // ISO-8601 timestamp string (UTC).

export interface AlkaneId {
  block: string; // Block height as a base-10 string (not a hash).
  tx: string; // Alkane tx index within the block as a base-10 string (not a txid).
}

export interface AlkaneToken {
  id?: AlkaneId; // Canonical alkane id; may be omitted in some list endpoints.
  alkaneId?: AlkaneId; // Alternate location for the same alkane id (legacy shape).
  name: string;
  symbol: string;
  totalSupply: NumericLike; // Raw supply in smallest units ("alks", 1e8 per token).
  cap: NumericLike; // Mint cap in smallest units ("alks").
  minted: NumericLike; // Minted amount in smallest units ("alks").
  mintActive: boolean;
  percentageMinted: NumericLike; // Percent (0-100), usually an integer.
  mintAmount: NumericLike; // Per-mint amount in smallest units ("alks").
  image?: string; // Opaque metadata string; format varies (URL/data URI/asset id not enforced).
  frbtcPoolPriceInSats?: NumericLike; // Price per token in sats, derived from frBTC pool.
  busdPoolPriceInUsd?: NumericLike; // Price per token in USD, derived from bUSD pool.
  maxSupply?: NumericLike; // Max supply in smallest units ("alks").
  floorPrice?: NumericLike; // Marketplace floor price in USD; may be number or string.
  fdv?: NumericLike; // Fully diluted value in USD (best-effort).
  holders?: NumericLike; // Holder count as an integer.
  marketcap?: NumericLike; // Market cap in USD (best-effort).
  idClubMarketplace?: boolean;
  busdPoolFdvInUsd?: NumericLike; // FDV in USD using bUSD pricing.
  frbtcPoolFdvInSats?: NumericLike; // FDV in sats using frBTC pricing.
  priceUsd?: NumericLike; // Price per token in USD.
  fdvUsd?: NumericLike; // Alias of busdPoolFdvInUsd in current implementation.
  busdPoolMarketcapInUsd?: NumericLike; // Market cap in USD using bUSD pricing.
  frbtcPoolMarketcapInSats?: NumericLike; // Market cap in sats using frBTC pricing.
  tokenPoolsVolume1dInUsd?: NumericLike; // USD volume across pools, 1d window.
  tokenPoolsVolume30dInUsd?: NumericLike; // USD volume across pools, 30d window.
  tokenPoolsVolume7dInUsd?: NumericLike; // USD volume across pools, 7d window.
  tokenPoolsVolumeAllTimeInUsd?: NumericLike; // USD volume across pools, all-time.
  tokenVolume1d?: NumericLike; // Raw token volume (smallest units) over 1d.
  tokenVolume30d?: NumericLike; // Raw token volume (smallest units) over 30d.
  tokenVolume7d?: NumericLike; // Raw token volume (smallest units) over 7d.
  tokenVolumeAllTime?: NumericLike; // Raw token volume (smallest units) all-time.
  priceChange24h?: string; // Percent change as a string (e.g., "1.23"), not a fraction.
  priceChange7d?: string; // Percent change as a string.
  priceChange30d?: string; // Percent change as a string.
  priceChangeAllTime?: string; // Percent change as a string.
  derived_data?: Record<string, NumericLike>; // Derived liquidity metrics keyed by "field-derived_block:tx".
}

export interface AlkaneDetails extends AlkaneToken {
  decimals?: number; // Token decimals from metadata (default 8 when unknown).
  supply?: NumericLike; // Raw supply from metadata (smallest units).
  priceInSatoshi?: NumericLike; // Price per token in sats (alias of frbtcPoolPriceInSats in some paths).
  tokenImage?: string | null; // Opaque metadata string; format varies (URL/data URI/asset id not enforced).
}

export interface AlkanesUtxoEntry {
  value: NumericString; // Raw balance in smallest units ("alks") for this alkane.
  name: string;
  symbol: string;
}

export interface FormattedUtxo {
  txId: string; // Bitcoin txid (hex string).
  outputIndex: number; // Vout index.
  satoshis: number; // BTC value in sats.
  scriptPk: string; // ScriptPubKey as hex string (no 0x prefix).
  address: string; // Network-specific BTC address (bech32 or legacy).
  inscriptions: string[]; // Ordinal inscription identifiers; format not enforced.
  runes: Record<string, unknown>; // Ordinals runes map; shape varies by indexer.
  alkanes: Record<string, AlkanesUtxoEntry>; // Keyed by alkane id string "block:tx".
  confirmations: number; // Confirmation count.
  indexed: boolean; // True if indexed by backend.
}

export interface TokenPairToken {
  symbol: string;
  alkaneId: AlkaneId;
  name?: string; // Token name from metadata.
  decimals?: number; // Token decimals from metadata.
  image?: string; // Opaque metadata string; format varies (URL/data URI/asset id not enforced).
  token0Amount?: NumericLike; // Reserve amount for token0 in smallest units ("alks").
  token1Amount?: NumericLike; // Reserve amount for token1 in smallest units ("alks").
  [key: string]: unknown;
}

export interface TokenPair {
  poolId: AlkaneId;
  poolVolume1dInUsd?: NumericLike; // USD volume over 1d window.
  poolTvlInUsd?: NumericLike; // Pool TVL in USD.
  poolName?: string; // Display name like "TOKEN0 / TOKEN1".
  reserve0?: NumericLike; // Token0 reserve in smallest units ("alks").
  reserve1?: NumericLike; // Token1 reserve in smallest units ("alks").
  token0: TokenPairToken;
  token1: TokenPairToken;
}

export interface PoolDetailsResult {
  token0: AlkaneId;
  token1: AlkaneId;
  token0Amount: NumericString; // Token0 reserve in smallest units ("alks").
  token1Amount: NumericString; // Token1 reserve in smallest units ("alks").
  tokenSupply: NumericString; // LP token supply in smallest units.
  poolName: string; // Display name like "TOKEN0 / TOKEN1".
  poolId?: AlkaneId; // Pool alkane id (same format as tokens).
  token0TvlInSats?: NumericLike; // Token0 reserve value in sats.
  token0TvlInUsd?: NumericLike; // Token0 reserve value in USD.
  token1TvlInSats?: NumericLike; // Token1 reserve value in sats.
  token1TvlInUsd?: NumericLike; // Token1 reserve value in USD.
  poolVolume30dInSats?: NumericLike; // Total pool volume in sats over 30d.
  poolVolume1dInSats?: NumericLike; // Total pool volume in sats over 1d.
  poolVolume30dInUsd?: NumericLike; // Total pool volume in USD over 30d.
  poolVolume1dInUsd?: NumericLike; // Total pool volume in USD over 1d.
  token0Volume30d?: NumericLike; // Token0 raw volume (alks) over 30d.
  token1Volume30d?: NumericLike; // Token1 raw volume (alks) over 30d.
  token0Volume1d?: NumericLike; // Token0 raw volume (alks) over 1d.
  token1Volume1d?: NumericLike; // Token1 raw volume (alks) over 1d.
  lPTokenValueInSats?: NumericLike; // Value per LP token in sats.
  lPTokenValueInUsd?: NumericLike; // Value per LP token in USD.
  poolTvlInSats?: NumericLike; // Pool TVL in sats.
  poolTvlInUsd?: NumericLike; // Pool TVL in USD.
  tvlChange24h?: string; // Percent change over 24h as string (fixed decimals).
  tvlChange7d?: string; // Percent change over 7d as string (fixed decimals).
  totalSupply?: NumericString; // Alias for tokenSupply in some responses.
  poolApr?: NumericLike; // APR percent (0-100+), not a fraction.
  initialToken0Amount?: NumericString; // Initial token0 amount at pool creation (alks).
  initialToken1Amount?: NumericString; // Initial token1 amount at pool creation (alks).
  creatorAddress?: string | null; // Pool creator BTC address if known.
  creationBlockHeight?: number; // Block height when pool was created.
  tvl?: NumericLike; // Best-effort TVL (USD) in provider-backed responses.
  volume1d?: NumericLike; // Provider volume metric; units vary (not normalized).
  volume7d?: NumericLike; // Provider volume metric; units vary (not normalized).
  volume30d?: NumericLike; // Provider volume metric; units vary (not normalized).
  volumeAllTime?: NumericLike; // Provider volume metric; units vary (not normalized).
  apr?: NumericLike; // Provider APR percent; computed from provider volume/TVL.
  tvlChange?: NumericLike; // Provider TVL change metric; units not standardized.
}

export interface AddressPoolPosition extends PoolDetailsResult {
  balance: NumericLike; // LP token balance in smallest units.
  token0ValueInSats: NumericLike; // User's token0 share value in sats.
  token1ValueInSats: NumericLike; // User's token1 share value in sats.
  token0ValueInUsd: NumericLike; // User's token0 share value in USD.
  token1ValueInUsd: NumericLike; // User's token1 share value in USD.
  totalValueInSats: NumericLike; // Total position value in sats.
  totalValueInUsd: NumericLike; // Total position value in USD.
}

export interface SwapHistoryLeg {
  tokenId: AlkaneId;
  amount: NumericString; // Raw token amount in smallest units ("alks").
}

export interface SwapHistoryItem {
  transactionId: string; // Bitcoin txid (hex string).
  pay: SwapHistoryLeg;
  receive: SwapHistoryLeg;
  address: string; // Counterparty BTC address.
  timestamp: IsoDateString; // ISO-8601 timestamp.
}

export interface PoolSwapHistoryResult {
  pool: {
    poolId: AlkaneId;
    poolName: string; // Display name like "TOKEN0 / TOKEN1".
  };
  swaps: SwapHistoryItem[];
  count: number;
  offset: number;
  total: number;
}

export interface AmmSwapHistoryItem {
  transactionId: string; // Bitcoin txid (hex string).
  poolBlockId?: string; // Pool alkane block as string (may be omitted).
  poolTxId?: string; // Pool alkane tx index as string (may be omitted).
  soldTokenBlockId: string; // Sold token block as string.
  soldTokenTxId: string; // Sold token tx index as string.
  boughtTokenBlockId: string; // Bought token block as string.
  boughtTokenTxId: string; // Bought token tx index as string.
  soldAmount: NumericString; // Sold amount in smallest units ("alks").
  boughtAmount: NumericString; // Bought amount in smallest units ("alks").
  sellerAddress?: string; // Seller BTC address.
  address?: string; // Alias for sellerAddress in some responses.
  timestamp: IsoDateString; // ISO-8601 timestamp.
}

export interface AmmMintHistoryItem {
  transactionId: string; // Bitcoin txid (hex string).
  poolBlockId?: string; // Pool alkane block as string.
  poolTxId?: string; // Pool alkane tx index as string.
  token0BlockId: string; // Token0 block as string.
  token0TxId: string; // Token0 tx index as string.
  token1BlockId: string; // Token1 block as string.
  token1TxId: string; // Token1 tx index as string.
  token0Amount: NumericString; // Token0 amount in smallest units ("alks").
  token1Amount: NumericString; // Token1 amount in smallest units ("alks").
  lpTokenAmount: NumericString; // LP tokens minted in smallest units.
  minterAddress?: string; // Minter BTC address.
  address?: string; // Alias for minterAddress in some responses.
  timestamp: IsoDateString; // ISO-8601 timestamp.
}

export interface AmmBurnHistoryItem {
  transactionId: string; // Bitcoin txid (hex string).
  poolBlockId?: string; // Pool alkane block as string.
  poolTxId?: string; // Pool alkane tx index as string.
  token0BlockId: string; // Token0 block as string.
  token0TxId: string; // Token0 tx index as string.
  token1BlockId: string; // Token1 block as string.
  token1TxId: string; // Token1 tx index as string.
  token0Amount: NumericString; // Token0 amount in smallest units ("alks").
  token1Amount: NumericString; // Token1 amount in smallest units ("alks").
  lpTokenAmount: NumericString; // LP tokens burned in smallest units.
  burnerAddress?: string; // Burner BTC address.
  address?: string; // Alias for burnerAddress in some responses.
  timestamp: IsoDateString; // ISO-8601 timestamp.
}

export interface AmmCreationHistoryItem {
  transactionId: string; // Bitcoin txid (hex string).
  poolBlockId: string; // Pool alkane block as string.
  poolTxId: string; // Pool alkane tx index as string.
  token0BlockId: string; // Token0 block as string.
  token0TxId: string; // Token0 tx index as string.
  token1BlockId: string; // Token1 block as string.
  token1TxId: string; // Token1 tx index as string.
  token0Amount: NumericString; // Token0 amount in smallest units ("alks").
  token1Amount: NumericString; // Token1 amount in smallest units ("alks").
  tokenSupply: NumericString; // LP token supply at creation in smallest units.
  creatorAddress?: string; // Creator BTC address.
  address?: string; // Alias for creatorAddress in some responses.
  timestamp: IsoDateString; // ISO-8601 timestamp.
}

export interface WrapHistoryItem {
  transactionId: string; // Bitcoin txid (hex string).
  address: string; // BTC address that wrapped/unwrapped.
  amount: NumericString; // Wrap/unwrap amount from Subfrost (likely sats; not enforced).
  timestamp: IsoDateString; // ISO-8601 timestamp.
}

export type AmmTxHistoryItem =
  | (AmmSwapHistoryItem & { type: "swap" })
  | (AmmMintHistoryItem & { type: "mint" })
  | (AmmBurnHistoryItem & { type: "burn" })
  | (AmmCreationHistoryItem & { type: "creation" })
  | (WrapHistoryItem & { type: "wrap" })
  | (WrapHistoryItem & { type: "unwrap" });

export interface SwapPath {
  path: AlkaneId[]; // Token route for the swap path.
  pools: TokenPair[]; // Pools used for each hop.
}

export type AlkaneTokenSortBy =
  | "price"
  | "fdv"
  | "marketcap"
  | "volume1d"
  | "volume30d"
  | "volume7d"
  | "volumeAllTime"
  | "holders"
  | "change1d"
  | "change7d"
  | "change30d"
  | "changeAllTime";

export type PoolSortBy = "tvl" | "volume1d" | "volume30d" | "apr" | "tvlChange";
export type TokenPairsSortBy = "tvl";
export type AmmTransactionType =
  | "swap"
  | "mint"
  | "burn"
  | "creation"
  | "wrap"
  | "unwrap";

export interface AlkaneBalance {
  name: string;
  symbol: string;
  balance: NumericString; // Raw balance in smallest units ("alks").
  alkaneId: AlkaneId;
  floorPrice?: NumericLike; // Marketplace floor price in USD; may be string or number.
  frbtcPoolPriceInSats?: NumericLike; // Price per token in sats.
  busdPoolPriceInUsd?: NumericLike; // Price per token in USD.
  priceUsd?: NumericLike; // Price per token in USD.
  priceInSatoshi?: NumericLike; // Price per token in sats.
  tokenImage?: string | null; // Opaque metadata string; format varies (URL/data URI/asset id not enforced).
  idClubMarketplace?: boolean;
}

// /get-alkanes-by-address
export interface OylApiGetAlkanesByAddressRequestParams {
  query_params: {};
  body_params: {
    address: string; // BTC address (bech32 or legacy) for the current network.
  };
}

export interface OylApiGetAlkanesByAddressResponse {
  statusCode: number;
  data: AlkaneBalance[];
}

// /get-alkanes-utxo
export interface OylApiGetAlkanesUtxoRequestParams {
  query_params: {};
  body_params: {
    address: string; // BTC address (bech32 or legacy) for the current network.
  };
}

export interface OylApiGetAlkanesUtxoResponse {
  statusCode: number;
  data: FormattedUtxo[];
}

// /get-amm-utxos
export interface OylApiGetAmmUtxosRequestParams {
  query_params: {};
  body_params: {
    address: string; // BTC address (bech32 or legacy) for the current network.
    // NOTE: Currently ignored by handler.
    // JSON-serialized SpendStrategy object (see @oyl/sdk).
    spendStrategy?: string;
  };
}

export interface OylApiGetAmmUtxosResponse {
  statusCode: number;
  data: {
    utxos: FormattedUtxo[];
  };
}

// /get-alkanes
export interface OylApiGetAlkanesRequestParams {
  query_params: {};
  body_params: {
    limit: number;
    // Default: 0.
    offset?: number;
    // Default: "volumeAllTime".
    sort_by?: AlkaneTokenSortBy;
    // Default: "desc".
    order?: "asc" | "desc";
    // Case-insensitive match on name/symbol; also matches exact "block:tx".
    searchQuery?: string | null;
  };
}

export interface OylApiGetAlkanesResponse {
  statusCode: number;
  data: {
    tokens: AlkaneToken[];
    total?: number;
    count?: number;
    offset?: number;
    limit?: number | null;
  };
}

// /global-alkanes-search
export interface OylApiGlobalAlkanesSearchRequestParams {
  query_params: {};
  body_params: {
    // Case-insensitive match on token/pool name or exact "block:tx".
    searchQuery: string;
  };
}

export interface OylApiGlobalAlkanesSearchResponse {
  statusCode: number;
  data: {
    tokens: AlkaneToken[];
    pools: PoolDetailsResult[];
  };
}

// /get-alkane-details
export interface OylApiGetAlkaneDetailsRequestParams {
  query_params: {};
  body_params: {
    alkaneId: AlkaneId;
  };
}

export interface OylApiGetAlkaneDetailsResponse {
  statusCode: number;
  data: AlkaneDetails;
}

// /get-pools
export interface OylApiGetPoolsRequestParams {
  query_params: {};
  body_params: {
    factoryId: AlkaneId;
    // NOTE: Currently ignored by handler.
    limit?: number | null;
    // NOTE: Currently ignored by handler.
    offset?: number;
  };
}

export interface OylApiGetPoolsResponse {
  statusCode: number;
  data: AlkaneId[];
  total: number;
  offset: number;
  limit: number | null;
}

// /get-pool-details
export interface OylApiGetPoolDetailsRequestParams {
  query_params: {};
  body_params: {
    factoryId: AlkaneId;
    poolId: AlkaneId;
  };
}

export interface OylApiGetPoolDetailsResponse {
  statusCode: number;
  data: PoolDetailsResult | null;
}

// /address-positions
export interface OylApiAddressPositionsRequestParams {
  query_params: {};
  body_params: {
    address: string; // BTC address (bech32 or legacy) for the current network.
    // NOTE: Currently ignored by handler.
    factoryId: AlkaneId;
  };
}

export interface OylApiAddressPositionsResponse {
  statusCode: number;
  data: AddressPoolPosition[];
}

// /get-all-pools-details
export interface OylApiGetAllPoolsDetailsRequestParams {
  query_params: {};
  body_params: {
    factoryId: AlkaneId;
    // Default: null (returns all pools).
    limit?: number | null;
    // Default: 0.
    offset?: number;
    // Default: "tvl".
    sort_by?: PoolSortBy;
    // Default: "desc".
    order?: "asc" | "desc";
    address?: string; // BTC address (bech32 or legacy) used for filtering.
    // Case-insensitive match on pool name or exact token "block:tx".
    searchQuery?: string;
  };
}

export interface OylApiGetAllPoolsDetailsResponse {
  statusCode: number;
  data: {
    count: number;
    pools: PoolDetailsResult[];
    total: number;
    offset: number;
    limit: number | null;
    largestPool: (PoolDetailsResult & { tvl?: NumericLike }) | null; // "tvl" is USD best-effort.
    trendingPools: { "1d": PoolDetailsResult & { trend: NumericLike } } | null; // "trend" is percent change (number).
    totalTvl: NumericLike; // Aggregate TVL in USD.
    totalPoolVolume24hChange: string; // Percent change string; currently hardcoded.
    totalPoolVolume24h: NumericLike; // Aggregate 24h volume in USD.
  };
}

// /get-pool-swap-history
export interface OylApiGetPoolSwapHistoryRequestParams {
  query_params: {};
  body_params: {
    poolId: AlkaneId;
    // Default: 50 (min 1, max 200).
    count?: number;
    // Default: 0.
    offset?: number;
    // Default: false.
    successful?: boolean; // Filter to successful txs when supported.
    // Default: true.
    includeTotal?: boolean; // Whether to include total count in response.
  };
}

export interface OylApiGetPoolSwapHistoryResponse {
  statusCode: number;
  data: {
    items: PoolSwapHistoryResult;
    total: number;
    count: number;
    offset: number;
  };
}

// /get-token-swap-history
export interface OylApiGetTokenSwapHistoryRequestParams {
  query_params: {};
  body_params: {
    tokenId: AlkaneId;
    // Default: 50 (min 1, max 200).
    count?: number;
    // Default: 0.
    offset?: number;
    // Default: false.
    successful?: boolean; // Filter to successful txs when supported.
    // Default: true.
    includeTotal?: boolean; // Whether to include total count in response.
  };
}

export interface OylApiGetTokenSwapHistoryResponse {
  statusCode: number;
  data: {
    items: AmmSwapHistoryItem[];
    total: number;
    count: number;
    offset: number;
  };
}

// /get-pool-mint-history
export interface OylApiGetPoolMintHistoryRequestParams {
  query_params: {};
  body_params: {
    poolId: AlkaneId;
    // Default: 50 (min 1, max 200).
    count?: number;
    // Default: 0.
    offset?: number;
    // Default: false.
    successful?: boolean; // Filter to successful txs when supported.
    // Default: true.
    includeTotal?: boolean; // Whether to include total count in response.
  };
}

export interface OylApiGetPoolMintHistoryResponse {
  statusCode: number;
  data: {
    items: AmmMintHistoryItem[];
    total: number;
    count: number;
    offset: number;
  };
}

// /get-pool-burn-history
export interface OylApiGetPoolBurnHistoryRequestParams {
  query_params: {};
  body_params: {
    poolId: AlkaneId;
    // Default: 50 (min 1, max 200).
    count?: number;
    // Default: 0.
    offset?: number;
    // Default: false.
    successful?: boolean; // Filter to successful txs when supported.
    // Default: true.
    includeTotal?: boolean; // Whether to include total count in response.
  };
}

export interface OylApiGetPoolBurnHistoryResponse {
  statusCode: number;
  data: {
    items: AmmBurnHistoryItem[];
    total: number;
    count: number;
    offset: number;
  };
}

// /get-pool-creation-history
export interface OylApiGetPoolCreationHistoryRequestParams {
  query_params: {};
  body_params: {
    // NOTE: Ignored; endpoint returns all pool creations.
    poolId?: AlkaneId;
    // Default: 50 (min 1, max 200).
    count?: number;
    // Default: 0.
    offset?: number;
    // Default: false.
    successful?: boolean; // Filter to successful txs when supported.
    // Default: true.
    includeTotal?: boolean; // Whether to include total count in response.
  };
}

export interface OylApiGetPoolCreationHistoryResponse {
  statusCode: number;
  data: {
    items: AmmCreationHistoryItem[];
    total: number;
    count: number;
    offset: number;
  };
}

// /get-address-swap-history-for-pool
export interface OylApiGetAddressSwapHistoryForPoolRequestParams {
  query_params: {};
  body_params: {
    address: string; // BTC address (bech32 or legacy) for the current network.
    poolId: AlkaneId;
    // Default: 50 (min 1, max 200).
    count?: number;
    // Default: 0.
    offset?: number;
    // Default: false.
    successful?: boolean; // Filter to successful txs when supported.
    // Default: true.
    includeTotal?: boolean; // Whether to include total count in response.
  };
}

export interface OylApiGetAddressSwapHistoryForPoolResponse {
  statusCode: number;
  data: {
    items: AmmSwapHistoryItem[];
    total: number;
    count: number;
    offset: number;
  };
}

// /get-address-swap-history-for-token
export interface OylApiGetAddressSwapHistoryForTokenRequestParams {
  query_params: {};
  body_params: {
    address: string; // BTC address (bech32 or legacy) for the current network.
    tokenId: AlkaneId;
    // Default: 50 (min 1, max 200).
    count?: number;
    // Default: 0.
    offset?: number;
    // Default: false.
    successful?: boolean; // Filter to successful txs when supported.
    // Default: true.
    includeTotal?: boolean; // Whether to include total count in response.
  };
}

export interface OylApiGetAddressSwapHistoryForTokenResponse {
  statusCode: number;
  data: {
    items: AmmSwapHistoryItem[];
    total: number;
    count: number;
    offset: number;
  };
}

// /get-address-wrap-history
export interface OylApiGetAddressWrapHistoryRequestParams {
  query_params: {};
  body_params: {
    address: string; // BTC address (bech32 or legacy) for the current network.
    // Default: 50 (min 1, max 200).
    count?: number;
    // Default: 0.
    offset?: number;
    // Default: false.
    successful?: boolean; // Filter to successful txs when supported.
    // Default: true.
    includeTotal?: boolean; // Whether to include total count in response.
  };
}

export interface OylApiGetAddressWrapHistoryResponse {
  statusCode: number;
  data: {
    items: WrapHistoryItem[];
    total: number;
    count: number;
    offset: number;
  };
}

// /get-address-unwrap-history
export interface OylApiGetAddressUnwrapHistoryRequestParams {
  query_params: {};
  body_params: {
    address: string; // BTC address (bech32 or legacy) for the current network.
    // Default: 50 (min 1, max 200).
    count?: number;
    // Default: 0.
    offset?: number;
    // Default: false.
    successful?: boolean; // Filter to successful txs when supported.
    // Default: true.
    includeTotal?: boolean; // Whether to include total count in response.
  };
}

export interface OylApiGetAddressUnwrapHistoryResponse {
  statusCode: number;
  data: {
    items: WrapHistoryItem[];
    total: number;
    count: number;
    offset: number;
  };
}

// /get-all-wrap-history
export interface OylApiGetAllWrapHistoryRequestParams {
  query_params: {};
  body_params: {
    // Default: 50 (min 1, max 200).
    count?: number;
    // Default: 0.
    offset?: number;
    // Default: false.
    successful?: boolean; // Filter to successful txs when supported.
    // Default: true.
    includeTotal?: boolean; // Whether to include total count in response.
  };
}

export interface OylApiGetAllWrapHistoryResponse {
  statusCode: number;
  data: {
    items: WrapHistoryItem[];
    total: number;
    count: number;
    offset: number;
  };
}

// /get-all-unwrap-history
export interface OylApiGetAllUnwrapHistoryRequestParams {
  query_params: {};
  body_params: {
    // Default: 50 (min 1, max 200).
    count?: number;
    // Default: 0.
    offset?: number;
    // Default: false.
    successful?: boolean; // Filter to successful txs when supported.
    // Default: true.
    includeTotal?: boolean; // Whether to include total count in response.
  };
}

export interface OylApiGetAllUnwrapHistoryResponse {
  statusCode: number;
  data: {
    items: WrapHistoryItem[];
    total: number;
    count: number;
    offset: number;
  };
}

// /get-total-unwrap-amount
export interface OylApiGetTotalUnwrapAmountRequestParams {
  query_params: {};
  body_params?: {
    // Default: false.
    successful?: boolean; // Filter to successful txs when supported.
    // Optional cutoff block height (inclusive). NOTE: Not currently forwarded by handler.
    blockHeight?: number; // Block height cutoff, inclusive.
  };
}

export interface OylApiGetTotalUnwrapAmountResponse {
  statusCode: number;
  data: {
    totalAmount: NumericString; // Sum of unwrap amounts from Subfrost (likely sats).
  };
}

// /get-address-pool-creation-history
export interface OylApiGetAddressPoolCreationHistoryRequestParams {
  query_params: {};
  body_params: {
    address: string; // BTC address (bech32 or legacy) for the current network.
    // NOTE: Currently ignored by handler.
    poolId?: AlkaneId;
    // Default: 50 (min 1, max 200).
    count?: number;
    // Default: 0.
    offset?: number;
    // Default: false.
    successful?: boolean; // Filter to successful txs when supported.
    // Default: true.
    includeTotal?: boolean; // Whether to include total count in response.
  };
}

export interface OylApiGetAddressPoolCreationHistoryResponse {
  statusCode: number;
  data: {
    items: AmmCreationHistoryItem[];
    total: number;
    count: number;
    offset: number;
  };
}

// /get-address-pool-mint-history
export interface OylApiGetAddressPoolMintHistoryRequestParams {
  query_params: {};
  body_params: {
    address: string; // BTC address (bech32 or legacy) for the current network.
    // NOTE: Currently ignored by handler.
    poolId?: AlkaneId;
    // Default: 50 (min 1, max 200).
    count?: number;
    // Default: 0.
    offset?: number;
    // Default: false.
    successful?: boolean; // Filter to successful txs when supported.
    // Default: true.
    includeTotal?: boolean; // Whether to include total count in response.
  };
}

export interface OylApiGetAddressPoolMintHistoryResponse {
  statusCode: number;
  data: {
    items: AmmMintHistoryItem[];
    total: number;
    count: number;
    offset: number;
  };
}

// /get-address-pool-burn-history
export interface OylApiGetAddressPoolBurnHistoryRequestParams {
  query_params: {};
  body_params: {
    address: string; // BTC address (bech32 or legacy) for the current network.
    // NOTE: Currently ignored by handler.
    poolId?: AlkaneId;
    // Default: 50 (min 1, max 200).
    count?: number;
    // Default: 0.
    offset?: number;
    // Default: false.
    successful?: boolean; // Filter to successful txs when supported.
    // Default: true.
    includeTotal?: boolean; // Whether to include total count in response.
  };
}

export interface OylApiGetAddressPoolBurnHistoryResponse {
  statusCode: number;
  data: {
    items: AmmBurnHistoryItem[];
    total: number;
    count: number;
    offset: number;
  };
}

// /get-all-address-amm-tx-history
export interface OylApiGetAllAddressAmmTxHistoryRequestParams {
  query_params: {};
  body_params: {
    address: string; // BTC address (bech32 or legacy) for the current network.
    // NOTE: Currently ignored by handler.
    poolId?: AlkaneId;
    transactionType?: AmmTransactionType; // Filter to a specific AMM tx type.
    // Default: 50 (min 1, max 200).
    count?: number;
    // Default: 0.
    offset?: number;
    // Default: false.
    successful?: boolean; // Filter to successful txs when supported.
    // Default: true.
    includeTotal?: boolean; // Whether to include total count in response.
  };
}

export interface OylApiGetAllAddressAmmTxHistoryResponse {
  statusCode: number;
  data: {
    items: AmmTxHistoryItem[];
    total: number;
    count: number;
    offset: number;
  };
}

// /get-all-amm-tx-history
export interface OylApiGetAllAmmTxHistoryRequestParams {
  query_params: {};
  body_params: {
    // NOTE: Currently ignored by handler.
    poolId?: AlkaneId;
    transactionType?: AmmTransactionType; // Filter to a specific AMM tx type.
    // Default: 50 (min 1, max 200).
    count?: number;
    // Default: 0.
    offset?: number;
    // Default: false.
    successful?: boolean; // Filter to successful txs when supported.
    // Default: true.
    includeTotal?: boolean; // Whether to include total count in response.
  };
}

export interface OylApiGetAllAmmTxHistoryResponse {
  statusCode: number;
  data: {
    items: AmmTxHistoryItem[];
    total: number;
    count: number;
    offset: number;
  };
}

// /get-all-token-pairs
export interface OylApiGetAllTokenPairsRequestParams {
  query_params: {};
  body_params: {
    factoryId: AlkaneId;
  };
}

export interface OylApiGetAllTokenPairsResponse {
  statusCode: number;
  data: TokenPair[];
}

// /get-token-pairs
export interface OylApiGetTokenPairsRequestParams {
  query_params: {};
  body_params: {
    factoryId: AlkaneId;
    alkaneId: AlkaneId;
    sort_by?: TokenPairsSortBy; // Sort field for pairs; only "tvl" supported.
    limit?: number | null;
    offset?: number;
    // Case-insensitive match on token/pool name or exact "block:tx".
    searchQuery?: string;
  };
}

export interface OylApiGetTokenPairsResponse {
  statusCode: number;
  data: TokenPair[];
}

// /get-alkane-swap-pair-details
export interface OylApiGetAlkaneSwapPairDetailsRequestParams {
  query_params: {};
  body_params: {
    factoryId: AlkaneId;
    tokenAId: AlkaneId;
    tokenBId: AlkaneId;
  };
}

export interface OylApiGetAlkaneSwapPairDetailsResponse {
  statusCode: number;
  data: SwapPath[];
}
