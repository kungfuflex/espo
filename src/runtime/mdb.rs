#![allow(clippy::type_complexity)]

use bitcoin::BlockHash;
use rocksdb::{
    BlockBasedOptions, Cache, DB, Direction, Error as RocksError, IteratorMode, Options,
    ReadOptions, WriteBatch,
};
use std::{path::Path, sync::Arc};

/// ===== Cache / open-time tuning =====
/// How big you want the LRU block cache (data + index/filter when enabled).
pub const ROCKS_BLOCK_CACHE_BYTES: usize = 1 << 30; // 1 GiB

/// Warm the block cache for this namespace on open (iterate all keys once).
pub const WARM_CACHE_ON_OPEN: bool = true;

/// Bloom filter bits/key (helps point lookups).
pub const BLOOM_BITS_PER_KEY: f64 = 10.0;

#[derive(Clone)]
pub struct Mdb {
    db: Arc<DB>,
    prefix: Vec<u8>,
}

impl Mdb {
    fn from_parts(db: Arc<DB>, prefix: impl AsRef<[u8]>) -> Self {
        let prefix_vec = prefix.as_ref().to_vec();
        Self { db, prefix: prefix_vec }
    }

    pub fn from_db(db: Arc<DB>, prefix: impl AsRef<[u8]>) -> Self {
        Self::from_parts(db, prefix)
    }

    /// Clone this handle onto the same underlying RocksDB with a different namespace prefix.
    pub fn clone_with_prefix(&self, prefix: impl AsRef<[u8]>) -> Self {
        Self::from_parts(Arc::clone(&self.db), prefix)
    }

    pub fn open(path: impl AsRef<Path>, prefix: impl AsRef<[u8]>) -> Result<Self, RocksError> {
        // ---- Block cache + table options ----
        let cache = Cache::new_lru_cache(ROCKS_BLOCK_CACHE_BYTES);

        let mut table = BlockBasedOptions::default();
        table.set_block_cache(&cache);
        // Put index + filter in the cache (hot metadata)
        table.set_cache_index_and_filter_blocks(true);
        // Pin L0 index/filter in cache (fastest for recent data)
        table.set_pin_l0_filter_and_index_blocks_in_cache(true);
        // Bloom filter (not whole-key)
        table.set_bloom_filter(BLOOM_BITS_PER_KEY, false);

        let mut opts = Options::default();
        opts.create_if_missing(true);
        // Keep readers open (avoid fd thrash)
        opts.set_max_open_files(-1);
        opts.set_block_based_table_factory(&table);

        let db = DB::open(&opts, path)?;

        let mdb = Self::from_parts(Arc::new(db), prefix);
        if WARM_CACHE_ON_OPEN {
            let _ = mdb.warm_up_namespace(); // best-effort
        }
        Ok(mdb)
    }

    pub fn open_read_only(
        path: impl AsRef<Path>,
        prefix: impl AsRef<[u8]>,
        error_if_log_file_exist: bool,
    ) -> Result<Self, RocksError> {
        let cache = Cache::new_lru_cache(ROCKS_BLOCK_CACHE_BYTES);

        let mut table = BlockBasedOptions::default();
        table.set_block_cache(&cache);
        table.set_cache_index_and_filter_blocks(true);
        table.set_pin_l0_filter_and_index_blocks_in_cache(true);
        table.set_bloom_filter(BLOOM_BITS_PER_KEY, false);

        let mut opts = Options::default();
        opts.set_block_based_table_factory(&table);

        let db = DB::open_for_read_only(&opts, path, error_if_log_file_exist)?;
        let mdb = Self::from_parts(Arc::new(db), prefix);
        if WARM_CACHE_ON_OPEN {
            let _ = mdb.warm_up_namespace();
        }
        Ok(mdb)
    }

    /// Walk the namespace once to populate the block cache.
    /// Returns the number of KV pairs touched.
    pub fn warm_up_namespace(&self) -> Result<usize, RocksError> {
        let ns = self.prefix.clone();

        let mut ro = ReadOptions::default();
        ro.fill_cache(true); // populate block cache on read

        // Start at the namespace prefix and scan forward until it stops matching.
        let it = self.db.iterator_opt(IteratorMode::From(&ns, Direction::Forward), ro);

        let mut count = 0usize;
        for res in it {
            let (k, _v) = res?;
            if !k.starts_with(&ns) {
                break;
            }
            count += 1;
        }
        Ok(count)
    }

    #[inline]
    pub fn prefixed(&self, k: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.prefix.len() + k.len());
        out.extend_from_slice(&self.prefix);
        out.extend_from_slice(k);
        out
    }

    pub fn get(&self, k: &[u8]) -> Result<Option<Vec<u8>>, RocksError> {
        self.db.get(self.prefixed(k))
    }

    pub fn get_at_blockhash(
        &self,
        block_hash: &BlockHash,
        k: &[u8],
    ) -> Result<Option<Vec<u8>>, RocksError> {
        let _ = block_hash;
        self.get(k)
    }

    pub fn scan_prefix_entries(
        &self,
        prefix: &[u8],
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, RocksError> {
        let ns_prefix = self.prefixed(prefix);

        let mut out = Vec::new();
        for res in self.db.iterator(IteratorMode::From(&ns_prefix, Direction::Forward)) {
            let (key, value) = res?;
            if !key.starts_with(&ns_prefix) {
                break;
            }
            if key.starts_with(&self.prefix) {
                out.push((key[self.prefix.len()..].to_vec(), value.to_vec()));
            }
        }
        Ok(out)
    }

    pub fn scan_prefix_entries_at_blockhash(
        &self,
        block_hash: &BlockHash,
        prefix: &[u8],
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, RocksError> {
        let _ = block_hash;
        self.scan_prefix_entries(prefix)
    }

    pub fn scan_prefix_keys(&self, prefix: &[u8]) -> Result<Vec<Vec<u8>>, RocksError> {
        let ns_prefix = self.prefixed(prefix);

        let mut out = Vec::new();
        for res in self.db.iterator(IteratorMode::From(&ns_prefix, Direction::Forward)) {
            let (key, _value) = res?;
            if !key.starts_with(&ns_prefix) {
                break;
            }
            if key.starts_with(&self.prefix) {
                out.push(key[self.prefix.len()..].to_vec());
            }
        }
        Ok(out)
    }

    pub fn scan_prefix_keys_at_blockhash(
        &self,
        block_hash: &BlockHash,
        prefix: &[u8],
    ) -> Result<Vec<Vec<u8>>, RocksError> {
        let _ = block_hash;
        self.scan_prefix_keys(prefix)
    }

    pub fn multi_get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>, RocksError> {
        // Apply DB prefix to each RELATIVE key
        let prefixed: Vec<Vec<u8>> = keys.iter().map(|k| self.prefixed(k)).collect();

        // rocksdb::DB::multi_get returns Vec<Result<Option<DBPinnableSlice>, Error>>
        let results = self.db.multi_get(prefixed);

        // Map to Result<Vec<Option<Vec<u8>>>, Error>, preserving order
        let mut out = Vec::with_capacity(results.len());
        for r in results {
            match r {
                Ok(Some(slice)) => out.push(Some(slice.to_vec())),
                Ok(None) => out.push(None),
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    pub fn multi_get_at_blockhash(
        &self,
        block_hash: &BlockHash,
        keys: &[Vec<u8>],
    ) -> Result<Vec<Option<Vec<u8>>>, RocksError> {
        let _ = block_hash;
        self.multi_get(keys)
    }

    pub fn put(&self, k: &[u8], v: &[u8]) -> Result<(), RocksError> {
        self.db.put(self.prefixed(k), v)
    }

    pub fn delete(&self, k: &[u8]) -> Result<(), RocksError> {
        self.db.delete(self.prefixed(k))
    }

    pub fn bulk_write<F>(&self, build: F) -> Result<(), RocksError>
    where
        F: FnOnce(&mut MdbBatch<'_>),
    {
        let mut wb = WriteBatch::default();
        let mut mb = MdbBatch { mdb: self, wb: &mut wb };
        build(&mut mb);
        self.db.write(wb)
    }

    /// Iterate forward over raw DB starting from namespaced key `start` (inclusive).
    pub fn iter_from(
        &self,
        start: &[u8],
    ) -> Box<dyn Iterator<Item = Result<(Vec<u8>, Vec<u8>), RocksError>> + '_> {
        let ns_start = self.prefixed(start);
        Box::new(
            self.db
                .iterator(IteratorMode::From(&ns_start, Direction::Forward))
                .map(|res| res.map(|(k, v)| (k.to_vec(), v.to_vec()))),
        )
    }

    #[inline]
    pub fn inner_db(&self) -> &DB {
        &self.db
    }

    #[inline]
    pub fn prefix(&self) -> &[u8] {
        &self.prefix
    }

    #[inline]
    pub fn is_versioned(&self) -> bool {
        false
    }

    pub fn begin_block(
        &self,
        height: u32,
        block_hash: &BlockHash,
        parent_hash: &BlockHash,
    ) -> Result<(), RocksError> {
        let _ = (height, block_hash, parent_hash);
        Ok(())
    }

    pub fn finish_block(&self) -> Result<(), RocksError> {
        Ok(())
    }

    pub fn abort_block(&self) {}

    pub fn has_blockhash(&self, block_hash: &BlockHash) -> Result<bool, RocksError> {
        let _ = block_hash;
        Ok(false)
    }

    pub fn blockhash_for_height(&self, height: u32) -> Result<Option<BlockHash>, RocksError> {
        let _ = height;
        Ok(None)
    }

    pub fn active_blockhash(&self) -> Option<BlockHash> {
        None
    }

    pub fn height_for_blockhash(&self, block_hash: &BlockHash) -> Result<Option<u32>, RocksError> {
        let _ = block_hash;
        Ok(None)
    }

    pub fn is_ancestor(
        &self,
        ancestor: &BlockHash,
        descendant: &BlockHash,
    ) -> Result<bool, RocksError> {
        let _ = (ancestor, descendant);
        Ok(false)
    }

    pub fn indexed_height_bounds(&self) -> Result<Option<(u32, u32)>, RocksError> {
        Ok(None)
    }
}

pub struct MdbBatch<'a> {
    mdb: &'a Mdb,
    wb: &'a mut WriteBatch,
}

impl<'a> MdbBatch<'a> {
    #[inline]
    pub fn put(&mut self, k: &[u8], v: &[u8]) {
        let key = self.mdb.prefixed(k);
        self.wb.put(key, v);
    }
    #[inline]
    pub fn delete(&mut self, k: &[u8]) {
        let key = self.mdb.prefixed(k);
        self.wb.delete(key);
    }
}
