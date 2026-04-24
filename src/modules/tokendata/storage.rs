use super::schemas::{SchemaTokenActivityV1, TokenActivityKind, TokenActivitySource};
use crate::runtime::mdb::{Mdb, MdbBatch};
use crate::runtime::state_at::StateAt;
use crate::runtime::tree_db::get_global_tree_db;
use crate::schemas::SchemaAlkaneId;
use anyhow::{Result, anyhow};
use bitcoin::BlockHash;
use borsh::BorshDeserialize;
use std::sync::Arc;

const INDEX_HEIGHT_KEY: &[u8] = b"/index_height";
const TOKEN_ACTIVITY_TS_ROOT: &[u8] = b"/token_activity/v1/";
const TOKEN_ACTIVITY_AMOUNT_ROOT: &[u8] = b"/token_activity_amount/v1/";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenActivityScope {
    All,
    Market,
    Mint,
}

impl TokenActivityScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Market => "market",
            Self::Mint => "mint",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenActivitySortField {
    Timestamp,
    Amount,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDir {
    Desc,
    Asc,
}

#[derive(Clone)]
pub struct TokenDataTable<'a> {
    mdb: &'a Mdb,
}

impl<'a> TokenDataTable<'a> {
    pub fn new(mdb: &'a Mdb) -> Self {
        Self { mdb }
    }

    pub fn index_height_key(&self) -> Vec<u8> {
        INDEX_HEIGHT_KEY.to_vec()
    }

    fn token_prefix(root: &[u8], scope: TokenActivityScope, token: &SchemaAlkaneId) -> Vec<u8> {
        let mut key = Vec::with_capacity(root.len() + 4 + 1 + 12 + 1);
        key.extend_from_slice(root);
        key.extend_from_slice(scope.as_str().as_bytes());
        key.push(b'/');
        key.extend_from_slice(&token.block.to_be_bytes());
        key.extend_from_slice(&token.tx.to_be_bytes());
        key.push(b'/');
        key
    }

    pub fn token_activity_prefix(
        &self,
        scope: TokenActivityScope,
        token: &SchemaAlkaneId,
    ) -> Vec<u8> {
        Self::token_prefix(TOKEN_ACTIVITY_TS_ROOT, scope, token)
    }

    pub fn token_activity_amount_prefix(
        &self,
        scope: TokenActivityScope,
        token: &SchemaAlkaneId,
    ) -> Vec<u8> {
        Self::token_prefix(TOKEN_ACTIVITY_AMOUNT_ROOT, scope, token)
    }

    pub fn token_activity_key(
        &self,
        scope: TokenActivityScope,
        token: &SchemaAlkaneId,
        timestamp: u64,
        txid: &[u8; 32],
        ordinal: u32,
        kind: TokenActivityKind,
    ) -> Vec<u8> {
        let mut key = self.token_activity_prefix(scope, token);
        key.extend_from_slice(&timestamp.to_be_bytes());
        key.extend_from_slice(txid);
        key.extend_from_slice(&ordinal.to_be_bytes());
        key.push(activity_kind_code(kind));
        key
    }

    pub fn token_activity_amount_key(
        &self,
        scope: TokenActivityScope,
        token: &SchemaAlkaneId,
        amount: u128,
        timestamp: u64,
        txid: &[u8; 32],
        ordinal: u32,
        kind: TokenActivityKind,
    ) -> Vec<u8> {
        let mut key = self.token_activity_amount_prefix(scope, token);
        key.extend_from_slice(&amount.to_be_bytes());
        key.extend_from_slice(&timestamp.to_be_bytes());
        key.extend_from_slice(txid);
        key.extend_from_slice(&ordinal.to_be_bytes());
        key.push(activity_kind_code(kind));
        key
    }

    pub fn mdb(&self) -> &Mdb {
        self.mdb
    }
}

#[derive(Clone)]
pub struct TokenDataProvider {
    mdb: Arc<Mdb>,
    view_blockhash: Option<BlockHash>,
}

impl TokenDataProvider {
    pub fn new(mdb: Arc<Mdb>) -> Self {
        Self { mdb, view_blockhash: None }
    }

    pub fn with_view_blockhash(&self, blockhash: Option<BlockHash>) -> Self {
        Self { mdb: Arc::clone(&self.mdb), view_blockhash: blockhash }
    }

    pub fn with_height(&self, height: Option<u64>, height_present: bool) -> Result<Self> {
        if !height_present {
            return Ok(self.with_view_blockhash(None));
        }
        let Some(height) = height else {
            return Err(anyhow!("missing_or_invalid_height"));
        };
        let height_u32 = u32::try_from(height).map_err(|_| anyhow!("height_out_of_range"))?;
        let Some(tree) = get_global_tree_db() else {
            return Err(anyhow!("versioned_tree_unavailable"));
        };
        let Some(blockhash) = tree
            .blockhash_for_height(height_u32)
            .map_err(|e| anyhow!("tree lookup failed: {e}"))?
        else {
            return Err(anyhow!("height_not_indexed"));
        };
        Ok(self.with_view_blockhash(Some(blockhash)))
    }

    pub fn table(&self) -> TokenDataTable<'_> {
        TokenDataTable::new(self.mdb.as_ref())
    }

    fn raw_get_at(&self, key: &[u8], blockhash: Option<BlockHash>) -> Result<Option<Vec<u8>>> {
        match blockhash {
            Some(blockhash) => self
                .mdb
                .get_at_blockhash(&blockhash, key)
                .map_err(|e| anyhow!("mdb.get_at_blockhash failed: {e}")),
            None => self.mdb.get(key).map_err(|e| anyhow!("mdb.get failed: {e}")),
        }
    }

    fn raw_scan_prefix_entries(
        &self,
        prefix: &[u8],
        blockhash: Option<BlockHash>,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut entries = match blockhash {
            Some(blockhash) => self
                .mdb
                .scan_prefix_entries_at_blockhash(&blockhash, prefix)
                .map_err(|e| anyhow!("mdb.scan_prefix_entries_at_blockhash failed: {e}"))?,
            None => self
                .mdb
                .scan_prefix_entries(prefix)
                .map_err(|e| anyhow!("mdb.scan_prefix_entries failed: {e}"))?,
        };
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(entries)
    }

    fn parse_timestamp_sort_key(
        &self,
        prefix: &[u8],
        key: &[u8],
    ) -> Option<(u64, u32, [u8; 32])> {
        let suffix = key.strip_prefix(prefix)?;
        if suffix.len() < 8 + 32 + 4 {
            return None;
        }
        let mut ts = [0u8; 8];
        ts.copy_from_slice(&suffix[..8]);
        let mut txid = [0u8; 32];
        txid.copy_from_slice(&suffix[8..40]);
        let mut ordinal = [0u8; 4];
        ordinal.copy_from_slice(&suffix[40..44]);
        Some((u64::from_be_bytes(ts), u32::from_be_bytes(ordinal), txid))
    }

    fn parse_amount_sort_key(
        &self,
        prefix: &[u8],
        key: &[u8],
    ) -> Option<(u128, u64, u32, [u8; 32])> {
        let suffix = key.strip_prefix(prefix)?;
        if suffix.len() < 16 + 8 + 32 + 4 {
            return None;
        }
        let mut amount = [0u8; 16];
        amount.copy_from_slice(&suffix[..16]);
        let mut ts = [0u8; 8];
        ts.copy_from_slice(&suffix[16..24]);
        let mut txid = [0u8; 32];
        txid.copy_from_slice(&suffix[24..56]);
        let mut ordinal = [0u8; 4];
        ordinal.copy_from_slice(&suffix[56..60]);
        Some((
            u128::from_be_bytes(amount),
            u64::from_be_bytes(ts),
            u32::from_be_bytes(ordinal),
            txid,
        ))
    }

    pub fn get_index_height(&self, params: GetIndexHeightParams) -> Result<GetIndexHeightResult> {
        let table = self.table();
        let Some(bytes) = self
            .raw_get_at(&table.index_height_key(), params.blockhash.resolve(self.view_blockhash))?
        else {
            return Ok(GetIndexHeightResult { height: None });
        };
        if bytes.len() != 4 {
            return Err(anyhow!("invalid /index_height length {}", bytes.len()));
        }
        let mut arr = [0u8; 4];
        arr.copy_from_slice(&bytes);
        Ok(GetIndexHeightResult { height: Some(u32::from_le_bytes(arr)) })
    }

    pub fn set_index_height(&self, params: SetIndexHeightParams) -> Result<()> {
        if params.blockhash.resolve(self.view_blockhash).is_some() {
            return Err(anyhow!("cannot_write_historical_view"));
        }
        self.mdb
            .put(&self.table().index_height_key(), &params.height.to_le_bytes())
            .map_err(|e| anyhow!("mdb.put failed: {e}"))
    }

    pub fn set_batch(&self, params: SetBatchParams) -> Result<()> {
        if params.blockhash.resolve(self.view_blockhash).is_some() {
            return Err(anyhow!("cannot_write_historical_view"));
        }
        self.mdb
            .bulk_write(|wb: &mut MdbBatch<'_>| {
                for key in &params.deletes {
                    wb.delete(key);
                }
                for (key, value) in &params.puts {
                    wb.put(key, value);
                }
            })
            .map_err(|e| anyhow!("mdb.bulk_write failed: {e}"))
    }

    pub fn get_token_activity_page(
        &self,
        params: GetTokenActivityPageParams,
    ) -> Result<GetTokenActivityPageResult> {
        let table = self.table();
        let prefix = match params.sort_by {
            TokenActivitySortField::Timestamp => {
                table.token_activity_prefix(params.scope, &params.token)
            }
            TokenActivitySortField::Amount => {
                table.token_activity_amount_prefix(params.scope, &params.token)
            }
        };
        let blockhash = params.blockhash.resolve(self.view_blockhash);
        let mut entries: Vec<(SchemaTokenActivityV1, (u128, u64, u32, [u8; 32]))> = self
            .raw_scan_prefix_entries(&prefix, blockhash)?
            .into_iter()
            .filter_map(|(k, v)| {
                let row = SchemaTokenActivityV1::try_from_slice(&v).ok()?;
                let sort_meta = match params.sort_by {
                    TokenActivitySortField::Timestamp => self
                        .parse_timestamp_sort_key(&prefix, &k)
                        .map(|(ts, ordinal, txid)| (0, ts, ordinal, txid))
                        .unwrap_or((0, row.timestamp, 0, row.txid)),
                    TokenActivitySortField::Amount => self
                        .parse_amount_sort_key(&prefix, &k)
                        .unwrap_or((amount_from_row(&row), row.timestamp, 0, row.txid)),
                };
                Some((row, sort_meta))
            })
            .filter(|(entry, _)| params.kind.map(|k| entry.kind == k).unwrap_or(true))
            .collect();
        entries.sort_by(|a, b| a.1.cmp(&b.1));
        if matches!(params.dir, SortDir::Desc) {
            entries.reverse();
        }
        let total = entries.len();
        let page = entries
            .into_iter()
            .skip(params.offset)
            .take(params.limit)
            .map(|(entry, _)| entry)
            .collect();
        Ok(GetTokenActivityPageResult { entries: page, total })
    }
}

pub struct GetIndexHeightParams {
    pub blockhash: StateAt,
}

pub struct GetIndexHeightResult {
    pub height: Option<u32>,
}

pub struct SetIndexHeightParams {
    pub blockhash: StateAt,
    pub height: u32,
}

pub struct SetBatchParams {
    pub blockhash: StateAt,
    pub deletes: Vec<Vec<u8>>,
    pub puts: Vec<(Vec<u8>, Vec<u8>)>,
}

pub struct GetTokenActivityPageParams {
    pub blockhash: StateAt,
    pub token: SchemaAlkaneId,
    pub offset: usize,
    pub limit: usize,
    pub kind: Option<TokenActivityKind>,
    pub scope: TokenActivityScope,
    pub sort_by: TokenActivitySortField,
    pub dir: SortDir,
}

pub struct GetTokenActivityPageResult {
    pub entries: Vec<SchemaTokenActivityV1>,
    pub total: usize,
}

fn activity_kind_code(kind: TokenActivityKind) -> u8 {
    match kind {
        TokenActivityKind::Buy => 0,
        TokenActivityKind::Sell => 1,
        TokenActivityKind::LiquidityAdd => 2,
        TokenActivityKind::LiquidityRemove => 3,
        TokenActivityKind::PoolCreate => 4,
        TokenActivityKind::Mint => 5,
    }
}

pub fn amount_from_row(row: &SchemaTokenActivityV1) -> u128 {
    row.token_delta.unsigned_abs()
}

pub fn scopes_for_source(source: TokenActivitySource) -> [TokenActivityScope; 2] {
    match source {
        TokenActivitySource::Market => [TokenActivityScope::All, TokenActivityScope::Market],
        TokenActivitySource::Mint => [TokenActivityScope::All, TokenActivityScope::Mint],
    }
}
