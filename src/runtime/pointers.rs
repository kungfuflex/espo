use crate::runtime::mdb::{Mdb, MdbBatch};
use anyhow::{Result, anyhow};
use bitcoin::BlockHash;

pub trait KeyValuePointer {
    fn key(&self) -> &[u8];
    fn get(&self) -> Result<Option<Vec<u8>>>;
    fn get_at_blockhash(&self, blockhash: &BlockHash) -> Result<Option<Vec<u8>>>;
    fn put(&self, value: &[u8]) -> Result<()>;
    fn multi_get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>>;
    fn multi_get_at_blockhash(
        &self,
        blockhash: &BlockHash,
        keys: &[Vec<u8>],
    ) -> Result<Vec<Option<Vec<u8>>>>;
    fn bulk_write<F>(&self, build: F) -> Result<()>
    where
        F: FnOnce(&mut MdbBatch<'_>);
}

pub trait ListBasedPointer {
    fn key(&self) -> &[u8];
    fn get(&self) -> Result<Option<Vec<u8>>>;
    fn get_at_blockhash(&self, blockhash: &BlockHash) -> Result<Option<Vec<u8>>>;
    fn put(&self, value: &[u8]) -> Result<()>;
    fn multi_get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>>;
    fn multi_get_at_blockhash(
        &self,
        blockhash: &BlockHash,
        keys: &[Vec<u8>],
    ) -> Result<Vec<Option<Vec<u8>>>>;
    fn bulk_write<F>(&self, build: F) -> Result<()>
    where
        F: FnOnce(&mut MdbBatch<'_>);
}

#[derive(Clone, Debug, Default)]
pub struct CursorScanPage {
    pub entries: Vec<(Vec<u8>, Vec<u8>)>,
    pub next_cursor: Option<Vec<u8>>,
    pub has_more: bool,
}

pub trait CursorPagedListPointer {
    fn scan_desc_cursor_page(
        &self,
        at_blockhash: Option<&BlockHash>,
        cursor: Option<&[u8]>,
        limit: usize,
    ) -> Result<CursorScanPage>;
}

#[derive(Clone)]
pub struct KvPointer<'a> {
    mdb: &'a Mdb,
    key: Vec<u8>,
}

impl<'a> KvPointer<'a> {
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

    pub fn list_keyword(&self, suffix: &str) -> ListPointer<'a> {
        self.list_select(suffix.as_bytes())
    }

    pub fn list_select(&self, suffix: &[u8]) -> ListPointer<'a> {
        let mut key = self.key.clone();
        key.extend_from_slice(suffix);
        ListPointer { mdb: self.mdb, key }
    }

    pub fn get(&self) -> Result<Option<Vec<u8>>> {
        self.mdb.get(&self.key).map_err(|e| anyhow!("mdb.get failed: {e}"))
    }

    pub fn get_at_blockhash(&self, blockhash: &BlockHash) -> Result<Option<Vec<u8>>> {
        self.mdb
            .get_at_blockhash(blockhash, &self.key)
            .map_err(|e| anyhow!("mdb.get_at_blockhash failed: {e}"))
    }

    pub fn put(&self, value: &[u8]) -> Result<()> {
        self.mdb.put(&self.key, value).map_err(|e| anyhow!("mdb.put failed: {e}"))
    }

    pub fn multi_get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>> {
        let full_keys: Vec<Vec<u8>> = keys
            .iter()
            .map(|k| {
                let mut key = self.key.clone();
                key.extend_from_slice(k);
                key
            })
            .collect();
        self.mdb.multi_get(&full_keys).map_err(|e| anyhow!("mdb.multi_get failed: {e}"))
    }

    pub fn multi_get_at_blockhash(
        &self,
        blockhash: &BlockHash,
        keys: &[Vec<u8>],
    ) -> Result<Vec<Option<Vec<u8>>>> {
        let full_keys: Vec<Vec<u8>> = keys
            .iter()
            .map(|k| {
                let mut key = self.key.clone();
                key.extend_from_slice(k);
                key
            })
            .collect();
        let mut out = Vec::with_capacity(full_keys.len());
        for key in &full_keys {
            out.push(
                self.mdb
                    .get_at_blockhash(blockhash, key)
                    .map_err(|e| anyhow!("mdb.get_at_blockhash failed: {e}"))?,
            );
        }
        Ok(out)
    }

    pub fn bulk_write<F>(&self, build: F) -> Result<()>
    where
        F: FnOnce(&mut MdbBatch<'_>),
    {
        self.mdb.bulk_write(build).map_err(|e| anyhow!("mdb.bulk_write failed: {e}"))
    }
}

impl<'a> KeyValuePointer for KvPointer<'a> {
    fn key(&self) -> &[u8] {
        self.key()
    }

    fn get(&self) -> Result<Option<Vec<u8>>> {
        self.get()
    }

    fn get_at_blockhash(&self, blockhash: &BlockHash) -> Result<Option<Vec<u8>>> {
        self.get_at_blockhash(blockhash)
    }

    fn put(&self, value: &[u8]) -> Result<()> {
        self.put(value)
    }

    fn multi_get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>> {
        self.multi_get(keys)
    }

    fn multi_get_at_blockhash(
        &self,
        blockhash: &BlockHash,
        keys: &[Vec<u8>],
    ) -> Result<Vec<Option<Vec<u8>>>> {
        self.multi_get_at_blockhash(blockhash, keys)
    }

    fn bulk_write<F>(&self, build: F) -> Result<()>
    where
        F: FnOnce(&mut MdbBatch<'_>),
    {
        self.bulk_write(build)
    }
}

#[derive(Clone)]
pub struct ListPointer<'a> {
    mdb: &'a Mdb,
    key: Vec<u8>,
}

impl<'a> ListPointer<'a> {
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

    pub fn kv_keyword(&self, suffix: &str) -> KvPointer<'a> {
        self.kv_select(suffix.as_bytes())
    }

    pub fn kv_select(&self, suffix: &[u8]) -> KvPointer<'a> {
        let mut key = self.key.clone();
        key.extend_from_slice(suffix);
        KvPointer { mdb: self.mdb, key }
    }

    pub fn get(&self) -> Result<Option<Vec<u8>>> {
        self.mdb.get(&self.key).map_err(|e| anyhow!("mdb.get failed: {e}"))
    }

    pub fn get_at_blockhash(&self, blockhash: &BlockHash) -> Result<Option<Vec<u8>>> {
        self.mdb
            .get_at_blockhash(blockhash, &self.key)
            .map_err(|e| anyhow!("mdb.get_at_blockhash failed: {e}"))
    }

    pub fn put(&self, value: &[u8]) -> Result<()> {
        self.mdb.put(&self.key, value).map_err(|e| anyhow!("mdb.put failed: {e}"))
    }

    pub fn multi_get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>> {
        let full_keys: Vec<Vec<u8>> = keys
            .iter()
            .map(|k| {
                let mut key = self.key.clone();
                key.extend_from_slice(k);
                key
            })
            .collect();
        self.mdb.multi_get(&full_keys).map_err(|e| anyhow!("mdb.multi_get failed: {e}"))
    }

    pub fn multi_get_at_blockhash(
        &self,
        blockhash: &BlockHash,
        keys: &[Vec<u8>],
    ) -> Result<Vec<Option<Vec<u8>>>> {
        let full_keys: Vec<Vec<u8>> = keys
            .iter()
            .map(|k| {
                let mut key = self.key.clone();
                key.extend_from_slice(k);
                key
            })
            .collect();
        let mut out = Vec::with_capacity(full_keys.len());
        for key in &full_keys {
            out.push(
                self.mdb
                    .get_at_blockhash(blockhash, key)
                    .map_err(|e| anyhow!("mdb.get_at_blockhash failed: {e}"))?,
            );
        }
        Ok(out)
    }

    pub fn bulk_write<F>(&self, build: F) -> Result<()>
    where
        F: FnOnce(&mut MdbBatch<'_>),
    {
        self.mdb.bulk_write(build).map_err(|e| anyhow!("mdb.bulk_write failed: {e}"))
    }

    pub fn scan_desc_cursor_page(
        &self,
        at_blockhash: Option<&BlockHash>,
        cursor: Option<&[u8]>,
        limit: usize,
    ) -> Result<CursorScanPage> {
        if limit == 0 {
            return Ok(CursorScanPage::default());
        }

        let mut entries = match at_blockhash {
            Some(blockhash) => self
                .mdb
                .scan_prefix_entries_at_blockhash(blockhash, &self.key)
                .map_err(|e| anyhow!("mdb.scan_prefix_entries_at_blockhash failed: {e}"))?,
            None => self
                .mdb
                .scan_prefix_entries(&self.key)
                .map_err(|e| anyhow!("mdb.scan_prefix_entries failed: {e}"))?,
        };
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        if let Some(c) = cursor {
            let keep_until = entries.partition_point(|(k, _)| k.as_slice() < c);
            entries.truncate(keep_until);
        }

        entries.reverse();
        let has_more = entries.len() > limit;
        if has_more {
            entries.truncate(limit);
        }
        let next_cursor = if has_more && !entries.is_empty() {
            entries.last().map(|(k, _)| k.clone())
        } else {
            None
        };

        Ok(CursorScanPage { entries, next_cursor, has_more })
    }
}

impl<'a> ListBasedPointer for ListPointer<'a> {
    fn key(&self) -> &[u8] {
        self.key()
    }

    fn get(&self) -> Result<Option<Vec<u8>>> {
        self.get()
    }

    fn get_at_blockhash(&self, blockhash: &BlockHash) -> Result<Option<Vec<u8>>> {
        self.get_at_blockhash(blockhash)
    }

    fn put(&self, value: &[u8]) -> Result<()> {
        self.put(value)
    }

    fn multi_get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>> {
        self.multi_get(keys)
    }

    fn multi_get_at_blockhash(
        &self,
        blockhash: &BlockHash,
        keys: &[Vec<u8>],
    ) -> Result<Vec<Option<Vec<u8>>>> {
        self.multi_get_at_blockhash(blockhash, keys)
    }

    fn bulk_write<F>(&self, build: F) -> Result<()>
    where
        F: FnOnce(&mut MdbBatch<'_>),
    {
        self.bulk_write(build)
    }
}

impl<'a> CursorPagedListPointer for ListPointer<'a> {
    fn scan_desc_cursor_page(
        &self,
        at_blockhash: Option<&BlockHash>,
        cursor: Option<&[u8]>,
        limit: usize,
    ) -> Result<CursorScanPage> {
        ListPointer::scan_desc_cursor_page(self, at_blockhash, cursor, limit)
    }
}

/// Two-surface pointer for immutable payload layouts:
/// - `locator`: versioned COW keys (historical-safe)
/// - `blob`: non-versioned immutable payload keys
#[derive(Clone)]
pub struct ListNonMutatePointer<'a> {
    locator: ListPointer<'a>,
    blob: ListPointer<'a>,
}

impl<'a> ListNonMutatePointer<'a> {
    pub fn root(locator_mdb: &'a Mdb, blob_mdb: &'a Mdb) -> Self {
        Self { locator: ListPointer::root(locator_mdb), blob: ListPointer::root(blob_mdb) }
    }

    pub fn key(&self) -> &[u8] {
        self.locator.key()
    }

    pub fn keyword(&self, suffix: &str) -> Self {
        self.select(suffix.as_bytes())
    }

    pub fn select(&self, suffix: &[u8]) -> Self {
        Self { locator: self.locator.select(suffix), blob: self.blob.select(suffix) }
    }

    pub fn locator_key(&self) -> &[u8] {
        self.locator.key()
    }

    pub fn blob_key(&self) -> &[u8] {
        self.blob.key()
    }

    pub fn locator_get(&self) -> Result<Option<Vec<u8>>> {
        self.locator.get()
    }

    pub fn locator_get_at_blockhash(&self, blockhash: &BlockHash) -> Result<Option<Vec<u8>>> {
        self.locator.get_at_blockhash(blockhash)
    }

    pub fn blob_get(&self) -> Result<Option<Vec<u8>>> {
        self.blob.get()
    }

    pub fn locator_multi_get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>> {
        self.locator.multi_get(keys)
    }

    pub fn locator_multi_get_at_blockhash(
        &self,
        blockhash: &BlockHash,
        keys: &[Vec<u8>],
    ) -> Result<Vec<Option<Vec<u8>>>> {
        self.locator.multi_get_at_blockhash(blockhash, keys)
    }

    pub fn blob_multi_get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>> {
        self.blob.multi_get(keys)
    }

    pub fn locator_put(&self, value: &[u8]) -> Result<()> {
        self.locator.put(value)
    }

    pub fn blob_put(&self, value: &[u8]) -> Result<()> {
        self.blob.put(value)
    }

    pub fn locator_bulk_write<F>(&self, build: F) -> Result<()>
    where
        F: FnOnce(&mut MdbBatch<'_>),
    {
        self.locator.bulk_write(build)
    }

    pub fn blob_bulk_write<F>(&self, build: F) -> Result<()>
    where
        F: FnOnce(&mut MdbBatch<'_>),
    {
        self.blob.bulk_write(build)
    }

    pub fn locator_scan_desc_cursor_page(
        &self,
        at_blockhash: Option<&BlockHash>,
        cursor: Option<&[u8]>,
        limit: usize,
    ) -> Result<CursorScanPage> {
        self.locator.scan_desc_cursor_page(at_blockhash, cursor, limit)
    }
}
