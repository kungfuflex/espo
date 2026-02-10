// Payload shapes for Oyl wallet endpoints (legacy API / api-provider usage).
// Sources:
// - agent-context/legacy-api-main/legacy-api-main/services/oyl-api/src.ts/index.ts
// - agent-context/legacy-api-main/legacy-api-main/services/oyl-api/src.ts/services/migration/types.ts
// - agent-context/legacy-api-main/legacy-api-main/services/oyl-api/src.ts/external/subfrost/index.ts
// - agent-context/oyl-sdk-main/src/utxo/types.ts
// - agent-context/oyl-sdk-main/src/account/account.ts

export interface ApiResponse<T> {
  statusCode: number
  data: T
}

// ---------------------------------------------
// /get-inscriptions
// ---------------------------------------------

export interface GetInscriptionsRequest {
  address: string
  sort_by?: string
  order?: string
  offset?: number
  count?: number
  exclude_brc20?: boolean
}

export type GetInscriptionsResponse = ApiResponse<InscriptionItem[]>

export interface InscriptionDelegate {
  delegate_id: string
  render_url: string
  mime_type: string
  content_url: string
  bis_url: string
}

export interface InscriptionMetadata {
  name: string
  [key: string]: unknown
}

export interface InscriptionItem {
  inscription_name: string
  inscription_id: string
  inscription_number: number
  parent_ids: string[]
  output_value: number | null
  metadata: InscriptionMetadata | null
  owner_wallet_addr: string
  last_sale_price: number
  slug: string | null
  collection_name: string
  satpoint: string
  last_transfer_block_height: number
  genesis_height: number
  content_url: string
  delegate?: InscriptionDelegate
}

// Note: server handler currently uses only address, offset, and count.
// sort_by, order, and exclude_brc20 are accepted but ignored in the current implementation.

// ---------------------------------------------
// /get-rune-balance
// ---------------------------------------------

export interface GetRuneBalanceRequest {
  address: string
}

export type GetRuneBalanceResponse = ApiResponse<RuneBalance[]>

export interface RuneBalance {
  wallet_addr: string
  rune_id: string | null
  total_balance: string
  rune_name: string
  spaced_rune_name: string
  decimals: number
  pkscript: string
  avg_unit_price_in_sats: number
}

// ---------------------------------------------
// /get-account-utxos
// ---------------------------------------------

export interface GetAccountUtxosRequest {
  // Legacy API expects a string. The wallet client sends JSON.stringify(account).
  account: string
}

// The legacy API routes /get-account-utxos through Subfrost.getAccountUtxos,
// which currently returns an AddressUtxoPortfolio-like shape (not AccountUtxoPortfolio).
export type GetAccountUtxosResponse = ApiResponse<AddressUtxoPortfolio>

export type RuneName = string
export type AlkaneReadableId = string

export type OrdOutputRune = Record<string, unknown>

export interface AlkanesUtxoEntry {
  value: string
  name: string
  symbol: string
}

export interface FormattedUtxo {
  txId: string
  outputIndex: number
  satoshis: number
  scriptPk: string
  address: string
  inscriptions: string[]
  runes: Record<RuneName, OrdOutputRune>
  alkanes: Record<AlkaneReadableId, AlkanesUtxoEntry>
  confirmations: number
  indexed: boolean
}

export interface AddressUtxoPortfolio {
  utxos: FormattedUtxo[]
  alkaneUtxos: FormattedUtxo[]
  spendableTotalBalance: number
  spendableUtxos: FormattedUtxo[]
  runeUtxos: FormattedUtxo[]
  ordUtxos: FormattedUtxo[]
  pendingUtxos: FormattedUtxo[]
  pendingTotalBalance: number
  totalBalance: number
}

// Wallet-side expectation (from @oyl/sdk) for comparison:
// export interface AccountUtxoPortfolio {
//   accountUtxos: FormattedUtxo[]
//   accountTotalBalance: number
//   accountSpendableTotalUtxos: FormattedUtxo[]
//   accountSpendableTotalBalance: number
//   accountPendingTotalBalance: number
//   accounts: Record<"nativeSegwit" | "taproot" | "nestedSegwit" | "legacy", AddressUtxoPortfolio>
// }
