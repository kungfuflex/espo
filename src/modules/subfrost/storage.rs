use super::consts::KEY_INDEX_HEIGHT;
use super::schemas::SchemaWrapEventV1;
use crate::runtime::mdb::{Mdb, MdbBatch};
use anyhow::{Result, anyhow};
use borsh::BorshDeserialize;
use std::sync::Arc;

#[derive(Clone)]
pub struct MdbPointer<'a> {
    mdb: &'a Mdb,
    key: Vec<u8>,
}

impl<'a> MdbPointer<'a> {
    pub fn root(mdb: &'a Mdb) -> Self {
        Self { mdb, key: Vec::new() }
    }

    pub fn key(&self) -> &[u8] {
        &self.key
    }

    pub fn keyword(&self, suffix: &str) -> Self {
        self.select(suffix.as_bytes())
    }

    pub fn select(&self, suffix: &[u8]) -> Self {
        let mut key = self.key.clone();
        key.extend_from_slice(suffix);
        Self { mdb: self.mdb, key }
    }

    pub fn get(&self) -> Result<Option<Vec<u8>>> {
        self.mdb.get(&self.key).map_err(|e| anyhow!("mdb.get failed: {e}"))
    }

    pub fn put(&self, value: &[u8]) -> Result<()> {
        self.mdb.put(&self.key, value).map_err(|e| anyhow!("mdb.put failed: {e}"))
    }

    pub fn scan_prefix(&self) -> Result<Vec<Vec<u8>>> {
        self.mdb
            .scan_prefix(&self.key)
            .map_err(|e| anyhow!("mdb.scan_prefix failed: {e}"))
    }

    pub fn bulk_write<F>(&self, build: F) -> Result<()>
    where
        F: FnOnce(&mut MdbBatch<'_>),
    {
        self.mdb.bulk_write(build).map_err(|e| anyhow!("mdb.bulk_write failed: {e}"))
    }
}

#[allow(non_snake_case)]
#[derive(Clone)]
pub struct SubfrostTable<'a> {
    pub ROOT: MdbPointer<'a>,
    pub INDEX_HEIGHT: MdbPointer<'a>,
    pub WRAP_EVENTS_ALL: MdbPointer<'a>,
    pub WRAP_EVENTS_BY_ADDRESS: MdbPointer<'a>,
    pub UNWRAP_EVENTS_ALL: MdbPointer<'a>,
    pub UNWRAP_EVENTS_BY_ADDRESS: MdbPointer<'a>,
    pub UNWRAP_TOTAL_LATEST: MdbPointer<'a>,
    pub UNWRAP_TOTAL_BY_HEIGHT: MdbPointer<'a>,
    pub UNWRAP_TOTAL_LATEST_SUCCESS: MdbPointer<'a>,
    pub UNWRAP_TOTAL_BY_HEIGHT_SUCCESS: MdbPointer<'a>,
}

impl<'a> SubfrostTable<'a> {
    pub fn new(mdb: &'a Mdb) -> Self {
        let root = MdbPointer::root(mdb);
        SubfrostTable {
            ROOT: root.clone(),
            INDEX_HEIGHT: root.select(KEY_INDEX_HEIGHT),
            WRAP_EVENTS_ALL: root.keyword("/wrap_events_all/v1/"),
            WRAP_EVENTS_BY_ADDRESS: root.keyword("/wrap_events_by_address/v1/"),
            UNWRAP_EVENTS_ALL: root.keyword("/unwrap_events_all/v1/"),
            UNWRAP_EVENTS_BY_ADDRESS: root.keyword("/unwrap_events_by_address/v1/"),
            UNWRAP_TOTAL_LATEST: root.keyword("/unwrap_total_latest/v1"),
            UNWRAP_TOTAL_BY_HEIGHT: root.keyword("/unwrap_total_by_height/v1/"),
            UNWRAP_TOTAL_LATEST_SUCCESS: root.keyword("/unwrap_total_latest_success/v1"),
            UNWRAP_TOTAL_BY_HEIGHT_SUCCESS: root.keyword("/unwrap_total_by_height_success/v1/"),
        }
    }
}

impl<'a> SubfrostTable<'a> {
    pub fn wrap_events_all_key(&self, ts: u64, seq: u32) -> Vec<u8> {
        let mut k = self.WRAP_EVENTS_ALL.key().to_vec();
        k.extend_from_slice(&ts.to_be_bytes());
        k.extend_from_slice(&seq.to_be_bytes());
        k
    }

    pub fn unwrap_events_all_key(&self, ts: u64, seq: u32) -> Vec<u8> {
        let mut k = self.UNWRAP_EVENTS_ALL.key().to_vec();
        k.extend_from_slice(&ts.to_be_bytes());
        k.extend_from_slice(&seq.to_be_bytes());
        k
    }

    pub fn wrap_events_by_address_prefix(&self, spk: &[u8]) -> Vec<u8> {
        let mut k = self.WRAP_EVENTS_BY_ADDRESS.key().to_vec();
        push_spk(&mut k, spk);
        k
    }

    pub fn unwrap_events_by_address_prefix(&self, spk: &[u8]) -> Vec<u8> {
        let mut k = self.UNWRAP_EVENTS_BY_ADDRESS.key().to_vec();
        push_spk(&mut k, spk);
        k
    }

    pub fn wrap_events_by_address_key(&self, spk: &[u8], ts: u64, seq: u32) -> Vec<u8> {
        let mut k = self.wrap_events_by_address_prefix(spk);
        k.extend_from_slice(&ts.to_be_bytes());
        k.extend_from_slice(&seq.to_be_bytes());
        k
    }

    pub fn unwrap_events_by_address_key(&self, spk: &[u8], ts: u64, seq: u32) -> Vec<u8> {
        let mut k = self.unwrap_events_by_address_prefix(spk);
        k.extend_from_slice(&ts.to_be_bytes());
        k.extend_from_slice(&seq.to_be_bytes());
        k
    }

    pub fn unwrap_total_latest_key(&self, successful: bool) -> Vec<u8> {
        if successful {
            self.UNWRAP_TOTAL_LATEST_SUCCESS.key().to_vec()
        } else {
            self.UNWRAP_TOTAL_LATEST.key().to_vec()
        }
    }

    pub fn unwrap_total_by_height_prefix(&self, successful: bool) -> Vec<u8> {
        if successful {
            self.UNWRAP_TOTAL_BY_HEIGHT_SUCCESS.key().to_vec()
        } else {
            self.UNWRAP_TOTAL_BY_HEIGHT.key().to_vec()
        }
    }

    pub fn unwrap_total_by_height_key(&self, height: u32, successful: bool) -> Vec<u8> {
        let mut k = self.unwrap_total_by_height_prefix(successful);
        k.extend_from_slice(&height.to_be_bytes());
        k
    }
}

fn push_spk(dst: &mut Vec<u8>, spk: &[u8]) {
    let len = spk.len().min(u16::MAX as usize) as u16;
    dst.extend_from_slice(&len.to_be_bytes());
    dst.extend_from_slice(&spk[..len as usize]);
}

fn decode_wrap_event(bytes: &[u8]) -> Result<SchemaWrapEventV1> {
    Ok(SchemaWrapEventV1::try_from_slice(bytes)?)
}

fn encode_wrap_event(event: &SchemaWrapEventV1) -> Result<Vec<u8>> {
    Ok(borsh::to_vec(event)?)
}

fn decode_u128_value(bytes: &[u8]) -> Option<u128> {
    if bytes.len() != 16 {
        return None;
    }
    let mut arr = [0u8; 16];
    arr.copy_from_slice(bytes);
    Some(u128::from_be_bytes(arr))
}

#[derive(Clone)]
pub struct SubfrostProvider {
    mdb: Arc<Mdb>,
}

impl SubfrostProvider {
    pub fn new(mdb: Arc<Mdb>) -> Self {
        Self { mdb }
    }

    pub fn table(&self) -> SubfrostTable<'_> {
        SubfrostTable::new(self.mdb.as_ref())
    }

    pub fn get_raw_value(&self, params: GetRawValueParams) -> Result<GetRawValueResult> {
        let value = self.mdb.get(&params.key).map_err(|e| anyhow!("mdb.get failed: {e}"))?;
        Ok(GetRawValueResult { value })
    }

    pub fn get_iter_prefix_rev(
        &self,
        params: GetIterPrefixRevParams,
    ) -> Result<GetIterPrefixRevResult> {
        let full_prefix = self.mdb.prefixed(&params.prefix);
        let mut entries = Vec::new();
        for res in self.mdb.iter_prefix_rev(&full_prefix) {
            let (k_full, v) = res.map_err(|e| anyhow!("mdb.iter_prefix_rev failed: {e}"))?;
            let rel = &k_full[self.mdb.prefix().len()..];
            entries.push((rel.to_vec(), v));
        }
        Ok(GetIterPrefixRevResult { entries })
    }

    pub fn set_batch(&self, params: SetBatchParams) -> Result<()> {
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

    pub fn get_index_height(&self, _params: GetIndexHeightParams) -> Result<GetIndexHeightResult> {
        crate::debug_timer_log!("get_index_height");
        let table = self.table();
        let Some(bytes) = table.INDEX_HEIGHT.get()? else {
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
        crate::debug_timer_log!("set_index_height");
        let table = self.table();
        table.INDEX_HEIGHT.put(&params.height.to_le_bytes())
    }

    pub fn get_wrap_events_by_address(
        &self,
        params: GetWrapEventsByAddressParams,
    ) -> Result<GetWrapEventsResult> {
        crate::debug_timer_log!("get_wrap_events_by_address");
        let table = self.table();
        let prefix = table.wrap_events_by_address_prefix(&params.address_spk);
        read_events(self, prefix, params.offset, params.limit, params.successful)
    }

    pub fn get_unwrap_events_by_address(
        &self,
        params: GetUnwrapEventsByAddressParams,
    ) -> Result<GetWrapEventsResult> {
        crate::debug_timer_log!("get_unwrap_events_by_address");
        let table = self.table();
        let prefix = table.unwrap_events_by_address_prefix(&params.address_spk);
        read_events(self, prefix, params.offset, params.limit, params.successful)
    }

    pub fn get_wrap_events_all(
        &self,
        params: GetWrapEventsAllParams,
    ) -> Result<GetWrapEventsResult> {
        crate::debug_timer_log!("get_wrap_events_all");
        let table = self.table();
        let prefix = table.WRAP_EVENTS_ALL.key().to_vec();
        read_events(self, prefix, params.offset, params.limit, params.successful)
    }

    pub fn get_unwrap_events_all(
        &self,
        params: GetUnwrapEventsAllParams,
    ) -> Result<GetWrapEventsResult> {
        crate::debug_timer_log!("get_unwrap_events_all");
        let table = self.table();
        let prefix = table.UNWRAP_EVENTS_ALL.key().to_vec();
        read_events(self, prefix, params.offset, params.limit, params.successful)
    }

    pub fn get_unwrap_total_latest(
        &self,
        params: GetUnwrapTotalLatestParams,
    ) -> Result<GetUnwrapTotalLatestResult> {
        crate::debug_timer_log!("get_unwrap_total_latest");
        let table = self.table();
        let key = table.unwrap_total_latest_key(params.successful);
        let total = self
            .get_raw_value(GetRawValueParams { key })?
            .value
            .and_then(|v| decode_u128_value(&v))
            .unwrap_or(0);
        Ok(GetUnwrapTotalLatestResult { total })
    }

    pub fn get_unwrap_total_at_or_before(
        &self,
        params: GetUnwrapTotalAtOrBeforeParams,
    ) -> Result<GetUnwrapTotalAtOrBeforeResult> {
        crate::debug_timer_log!("get_unwrap_total_at_or_before");
        let table = self.table();
        let prefix = table.unwrap_total_by_height_prefix(params.successful);
        let entries = match self.get_iter_prefix_rev(GetIterPrefixRevParams { prefix }) {
            Ok(v) => v.entries,
            Err(_) => Vec::new(),
        };
        let mut total = None;
        for (k, v) in entries {
            if k.len() < 4 {
                continue;
            }
            let height_bytes = &k[k.len() - 4..];
            let mut arr = [0u8; 4];
            arr.copy_from_slice(height_bytes);
            let height = u32::from_be_bytes(arr);
            if height <= params.height {
                total = decode_u128_value(&v);
                break;
            }
        }
        Ok(GetUnwrapTotalAtOrBeforeResult { total })
    }
}

fn read_events(
    provider: &SubfrostProvider,
    prefix: Vec<u8>,
    offset: usize,
    limit: usize,
    successful: Option<bool>,
) -> Result<GetWrapEventsResult> {
    let entries = match provider.get_iter_prefix_rev(GetIterPrefixRevParams { prefix }) {
        Ok(v) => v.entries,
        Err(_) => Vec::new(),
    };

    let mut total = 0usize;
    let mut out = Vec::new();
    let mut seen = 0usize;
    for (_k, v) in entries {
        let entry = match decode_wrap_event(&v) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if let Some(want) = successful {
            if want && !entry.success {
                continue;
            }
        }
        total += 1;
        if seen < offset {
            seen += 1;
            continue;
        }
        if out.len() < limit {
            out.push(entry);
        }
    }

    Ok(GetWrapEventsResult { entries: out, total })
}

pub struct GetRawValueParams {
    pub key: Vec<u8>,
}

pub struct GetRawValueResult {
    pub value: Option<Vec<u8>>,
}

pub struct GetIterPrefixRevParams {
    pub prefix: Vec<u8>,
}

pub struct GetIterPrefixRevResult {
    pub entries: Vec<(Vec<u8>, Vec<u8>)>,
}

pub struct SetBatchParams {
    pub deletes: Vec<Vec<u8>>,
    pub puts: Vec<(Vec<u8>, Vec<u8>)>,
}

pub struct GetIndexHeightParams;

pub struct GetIndexHeightResult {
    pub height: Option<u32>,
}

pub struct SetIndexHeightParams {
    pub height: u32,
}

pub struct GetWrapEventsByAddressParams {
    pub address_spk: Vec<u8>,
    pub offset: usize,
    pub limit: usize,
    pub successful: Option<bool>,
}

pub struct GetUnwrapEventsByAddressParams {
    pub address_spk: Vec<u8>,
    pub offset: usize,
    pub limit: usize,
    pub successful: Option<bool>,
}

pub struct GetWrapEventsAllParams {
    pub offset: usize,
    pub limit: usize,
    pub successful: Option<bool>,
}

pub struct GetUnwrapEventsAllParams {
    pub offset: usize,
    pub limit: usize,
    pub successful: Option<bool>,
}

pub struct GetUnwrapTotalLatestParams {
    pub successful: bool,
}

pub struct GetUnwrapTotalLatestResult {
    pub total: u128,
}

pub struct GetUnwrapTotalAtOrBeforeParams {
    pub height: u32,
    pub successful: bool,
}

pub struct GetUnwrapTotalAtOrBeforeResult {
    pub total: Option<u128>,
}

pub struct GetWrapEventsResult {
    pub entries: Vec<SchemaWrapEventV1>,
    pub total: usize,
}

pub struct SetWrapEventParams {
    pub key: Vec<u8>,
    pub event: SchemaWrapEventV1,
}

impl SubfrostProvider {
    pub fn set_wrap_event(&self, params: SetWrapEventParams) -> Result<()> {
        crate::debug_timer_log!("set_wrap_event");
        let bytes = encode_wrap_event(&params.event)?;
        self.mdb.put(&params.key, &bytes).map_err(|e| anyhow!("mdb.put failed: {e}"))
    }
}
