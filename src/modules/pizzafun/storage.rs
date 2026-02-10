use crate::runtime::mdb::{Mdb, MdbBatch};
use crate::schemas::SchemaAlkaneId;
use anyhow::{Result, anyhow};
use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SeriesEntry {
    pub series_id: String,
    pub alkane_id: SchemaAlkaneId,
    pub creation_height: u32,
}

pub fn normalize_series_id(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

pub fn series_id_base_from_name(name_norm: &str) -> Option<String> {
    let trimmed = name_norm.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lowered = trimmed.to_ascii_lowercase();
    Some(lowered.split_whitespace().collect::<Vec<_>>().join("-"))
}

fn series_id_matches_name(series_id: &str, name_norm: &str) -> bool {
    if series_id == name_norm {
        return true;
    }
    if let Some(rest) = series_id.strip_prefix(name_norm) {
        if let Some(num) = rest.strip_prefix('-') {
            return !num.is_empty() && num.chars().all(|c| c.is_ascii_digit());
        }
    }
    false
}

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

    pub fn scan_prefix(&self) -> Result<Vec<Vec<u8>>> {
        self.mdb.scan_prefix(&self.key)
    }

    pub fn bulk_write<F>(&self, build: F) -> Result<()>
    where
        F: FnOnce(&mut MdbBatch<'_>),
    {
        self.mdb.bulk_write(build).map_err(|e| anyhow!("mdb.bulk_write failed: {e}"))
    }

    pub fn mdb(&self) -> &Mdb {
        self.mdb
    }
}

#[allow(non_snake_case)]
#[derive(Clone)]
pub struct PizzafunTable<'a> {
    pub ROOT: MdbPointer<'a>,
    pub INDEX_HEIGHT: MdbPointer<'a>,
    pub SERIES_BY_ID: MdbPointer<'a>,
    pub SERIES_BY_ALKANE: MdbPointer<'a>,
}

impl<'a> PizzafunTable<'a> {
    pub fn new(mdb: &'a Mdb) -> Self {
        let root = MdbPointer::root(mdb);
        Self {
            INDEX_HEIGHT: root.keyword("/index_height"),
            SERIES_BY_ID: root.keyword("/series/by_id/"),
            SERIES_BY_ALKANE: root.keyword("/series/by_alkane/"),
            ROOT: root,
        }
    }

    pub fn series_by_id_key(&self, series_id: &str) -> Vec<u8> {
        self.SERIES_BY_ID.select(series_id.as_bytes()).key().to_vec()
    }

    pub fn series_by_id_prefix(&self) -> Vec<u8> {
        self.SERIES_BY_ID.key().to_vec()
    }

    pub fn series_by_alkane_key(&self, alkane: &SchemaAlkaneId) -> Vec<u8> {
        let mut suffix = Vec::with_capacity(12);
        suffix.extend_from_slice(&alkane.block.to_be_bytes());
        suffix.extend_from_slice(&alkane.tx.to_be_bytes());
        self.SERIES_BY_ALKANE.select(&suffix).key().to_vec()
    }

    pub fn series_by_alkane_prefix(&self) -> Vec<u8> {
        self.SERIES_BY_ALKANE.key().to_vec()
    }
}

pub struct GetIndexHeightParams;

pub struct GetIndexHeightResult {
    pub height: Option<u32>,
}

pub struct SetIndexHeightParams {
    pub height: u32,
}

#[derive(Clone)]
pub struct PizzafunProvider {
    mdb: Arc<Mdb>,
}

impl PizzafunProvider {
    pub fn new(mdb: Arc<Mdb>) -> Self {
        Self { mdb }
    }

    pub fn table(&self) -> PizzafunTable<'_> {
        PizzafunTable::new(self.mdb.as_ref())
    }

    pub fn mdb(&self) -> &Mdb {
        self.mdb.as_ref()
    }

    pub fn get_index_height(&self, _params: GetIndexHeightParams) -> Result<GetIndexHeightResult> {
        crate::debug_timer_log!("pizzafun.get_index_height");
        let table = self.table();
        let Some(bytes) = table.INDEX_HEIGHT.get()? else {
            return Ok(GetIndexHeightResult { height: None });
        };
        if bytes.len() != 4 {
            return Err(anyhow!("[PIZZAFUN] invalid /index_height length {}", bytes.len()));
        }
        let mut arr = [0u8; 4];
        arr.copy_from_slice(&bytes);
        Ok(GetIndexHeightResult { height: Some(u32::from_le_bytes(arr)) })
    }

    pub fn set_index_height(&self, params: SetIndexHeightParams) -> Result<()> {
        crate::debug_timer_log!("pizzafun.set_index_height");
        let table = self.table();
        table.INDEX_HEIGHT.put(&params.height.to_le_bytes())
    }

    pub fn get_series_by_id(&self, series_id: &str) -> Result<Option<SeriesEntry>> {
        let table = self.table();
        let key = table.series_by_id_key(series_id);
        let Some(bytes) = self.mdb.get(&key).map_err(|e| anyhow!("mdb.get failed: {e}"))? else {
            return Ok(None);
        };
        Ok(Some(SeriesEntry::try_from_slice(&bytes)?))
    }

    pub fn get_series_by_ids(&self, series_ids: &[String]) -> Result<Vec<Option<SeriesEntry>>> {
        let table = self.table();
        let keys: Vec<Vec<u8>> = series_ids.iter().map(|s| table.series_by_id_key(s)).collect();
        let raw = self.mdb.multi_get(&keys).map_err(|e| anyhow!("mdb.multi_get failed: {e}"))?;
        let mut out = Vec::with_capacity(raw.len());
        for item in raw {
            match item {
                Some(bytes) => out.push(Some(SeriesEntry::try_from_slice(&bytes)?)),
                None => out.push(None),
            }
        }
        Ok(out)
    }

    pub fn get_series_by_alkane(&self, alkane: &SchemaAlkaneId) -> Result<Option<SeriesEntry>> {
        let table = self.table();
        let key = table.series_by_alkane_key(alkane);
        let Some(bytes) = self.mdb.get(&key).map_err(|e| anyhow!("mdb.get failed: {e}"))? else {
            return Ok(None);
        };
        Ok(Some(SeriesEntry::try_from_slice(&bytes)?))
    }

    pub fn get_series_by_alkanes(
        &self,
        alkanes: &[SchemaAlkaneId],
    ) -> Result<Vec<Option<SeriesEntry>>> {
        let table = self.table();
        let keys: Vec<Vec<u8>> = alkanes.iter().map(|a| table.series_by_alkane_key(a)).collect();
        let raw = self.mdb.multi_get(&keys).map_err(|e| anyhow!("mdb.multi_get failed: {e}"))?;
        let mut out = Vec::with_capacity(raw.len());
        for item in raw {
            match item {
                Some(bytes) => out.push(Some(SeriesEntry::try_from_slice(&bytes)?)),
                None => out.push(None),
            }
        }
        Ok(out)
    }

    pub fn get_series_entries_by_name(&self, name_norm: &str) -> Result<Vec<SeriesEntry>> {
        let table = self.table();
        let base_prefix = table.series_by_id_prefix();
        let mut lookup_names: Vec<String> = vec![name_norm.to_string()];
        if let Some(series_base) = series_id_base_from_name(name_norm) {
            if series_base != name_norm {
                lookup_names.push(series_base);
            }
        }

        let mut filtered_keys: Vec<Vec<u8>> = Vec::new();
        let mut seen_keys: HashSet<Vec<u8>> = HashSet::new();
        for lookup_name in lookup_names {
            let mut prefix = base_prefix.clone();
            prefix.extend_from_slice(lookup_name.as_bytes());
            let keys = self.mdb.scan_prefix(&prefix)?;
            for key in keys {
                if !key.starts_with(base_prefix.as_slice()) {
                    continue;
                }
                let raw_id = &key[base_prefix.len()..];
                let Ok(series_id) = std::str::from_utf8(raw_id) else { continue };
                if series_id_matches_name(series_id, &lookup_name) && seen_keys.insert(key.clone())
                {
                    filtered_keys.push(key);
                }
            }
        }

        if filtered_keys.is_empty() {
            return Ok(Vec::new());
        }

        let raw = self
            .mdb
            .multi_get(&filtered_keys)
            .map_err(|e| anyhow!("mdb.multi_get failed: {e}"))?;
        let mut out = Vec::with_capacity(raw.len());
        for item in raw {
            if let Some(bytes) = item {
                out.push(SeriesEntry::try_from_slice(&bytes)?);
            }
        }
        Ok(out)
    }

    pub fn update_series_for_name(
        &self,
        existing: &[SeriesEntry],
        updated: &[SeriesEntry],
    ) -> Result<()> {
        let table = self.table();
        let mut deletes: Vec<Vec<u8>> = Vec::with_capacity(existing.len() * 2);
        for entry in existing {
            deletes.push(table.series_by_id_key(&entry.series_id));
            deletes.push(table.series_by_alkane_key(&entry.alkane_id));
        }

        let mut puts: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(updated.len() * 2);
        for entry in updated {
            let encoded = borsh::to_vec(entry)?;
            puts.push((table.series_by_id_key(&entry.series_id), encoded.clone()));
            puts.push((table.series_by_alkane_key(&entry.alkane_id), encoded));
        }

        self.mdb
            .bulk_write(|wb: &mut MdbBatch<'_>| {
                for key in &deletes {
                    wb.delete(key);
                }
                for (key, value) in &puts {
                    wb.put(key, value);
                }
            })
            .map_err(|e| anyhow!("mdb.bulk_write failed: {e}"))
    }

    pub fn replace_series_entries(&self, entries: &[SeriesEntry], height: u32) -> Result<()> {
        let table = self.table();
        let mut deletes = table.SERIES_BY_ID.scan_prefix()?;
        deletes.extend(table.SERIES_BY_ALKANE.scan_prefix()?);

        let mut puts: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(entries.len() * 2);
        for entry in entries {
            let encoded = borsh::to_vec(entry)?;
            puts.push((table.series_by_id_key(&entry.series_id), encoded.clone()));
            puts.push((table.series_by_alkane_key(&entry.alkane_id), encoded));
        }

        self.mdb
            .bulk_write(|wb: &mut MdbBatch<'_>| {
                for key in &deletes {
                    wb.delete(key);
                }
                for (key, value) in &puts {
                    wb.put(key, value);
                }
                wb.put(table.INDEX_HEIGHT.key(), &height.to_le_bytes());
            })
            .map_err(|e| anyhow!("mdb.bulk_write failed: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_series_id, series_id_base_from_name, series_id_matches_name};

    #[test]
    fn normalize_series_id_preserves_spacing_for_backcompat() {
        assert_eq!(normalize_series_id("  Love Bomb  ").as_deref(), Some("love bomb"));
        assert_eq!(normalize_series_id("   "), None);
    }

    #[test]
    fn series_id_base_from_name_matches_expected_slug() {
        assert_eq!(series_id_base_from_name("love bomb").as_deref(), Some("love-bomb"));
    }

    #[test]
    fn series_id_matching_supports_base_and_numbered_suffix() {
        assert!(series_id_matches_name("love-bomb", "love-bomb"));
        assert!(series_id_matches_name("love-bomb-2", "love-bomb"));
        assert!(!series_id_matches_name("love-bomb-two", "love-bomb"));
        assert!(!series_id_matches_name("love-bomb-2a", "love-bomb"));
    }
}
