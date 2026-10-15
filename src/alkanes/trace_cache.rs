//! The TiKV block-trace cache as a trace source (`traces_tikv_pd`).
//!
//! alspo-traces-tlsd writes one RawKV entry per settled block (6+ deep) into the
//! dedicated `tikv-traces` cluster:
//!
//! * key   — `asp1:blk:` ‖ the block hash in display hex;
//! * value — hex of a "synthetic block": the real block, byte for byte, with one
//!   extra coinbase output `OP_RETURN <"ASP0" ‖ traceblock protobuf>` appended
//!   (`tlsd-apps/alspo-traces/src/lib.rs::embed_traces`). The header is not
//!   touched, so the value's header hashes to the key's block hash.
//!
//! Pinning: the key is the block hash bitcoind reports for the height being
//! indexed, and the value is accepted only if its header hashes to that same
//! hash, so an entry can never be served for the wrong block. A miss is a miss:
//! it fails the block (the indexer waits and retries), it is never guessed.

use crate::alkanes::metashrew_rpc::decode_traceblock;
use crate::alkanes::trace::PartialEspoTrace;
use anyhow::{Context, Result, anyhow, bail};
use bitcoin::BlockHash;
use bitcoin::blockdata::script::Instruction;
use bitcoin::opcodes::all::OP_RETURN;

pub const TRACE_MAGIC: &[u8; 4] = b"ASP0";
pub const DEFAULT_KEY_PREFIX: &str = "asp1:blk:";

/// Alias records (template of a created alkane), written beside the block
/// entries under their own prefix so the existing `asp1:blk:` readers (exact
/// key gets) never see them: `asp1:alias:<block>:<tx>` → ascii `<block>:<tx>`
/// of the alias chain's end, i.e. what the local reader reports as
/// `factory_alkane`.
pub const ALIAS_PREFIX: &str = "asp1:alias:";

pub fn alias_key(id: &crate::schemas::SchemaAlkaneId) -> Vec<u8> {
    format!("{ALIAS_PREFIX}{}:{}", id.block, id.tx).into_bytes()
}

pub fn decode_alias(value: &[u8]) -> Result<crate::schemas::SchemaAlkaneId> {
    let s = std::str::from_utf8(value).context("alias record is not utf-8")?;
    let (b, t) = s.trim().split_once(':').ok_or_else(|| anyhow!("alias record {s:?} is not block:tx"))?;
    Ok(crate::schemas::SchemaAlkaneId {
        block: b.parse().with_context(|| format!("alias record {s:?}"))?,
        tx: t.parse().with_context(|| format!("alias record {s:?}"))?,
    })
}

/// Message used for a cache miss; the indexer loop treats it as a wait.
pub const MISS_MESSAGE: &str = "trace cache miss at height ";

pub fn cache_key(prefix: &str, hash: &BlockHash) -> Vec<u8> {
    let mut k = prefix.as_bytes().to_vec();
    k.extend_from_slice(hash.to_string().as_bytes());
    k
}

/// Decode a cached synthetic block, check it is `expected`, and return its traces.
pub fn decode_cached_block(value: &[u8], expected: &BlockHash) -> Result<Vec<PartialEspoTrace>> {
    let text = std::str::from_utf8(value).context("trace cache value is not utf-8 hex")?;
    let raw = hex::decode(text.trim()).context("trace cache value is not hex")?;
    let block: bitcoin::Block =
        bitcoin::consensus::deserialize(&raw).context("trace cache value is not a block")?;
    let got = block.block_hash();
    if got != *expected {
        bail!("trace cache entry for {expected} holds block {got}");
    }
    let coinbase = block.txdata.first().ok_or_else(|| anyhow!("cached block has no coinbase"))?;
    let out = coinbase
        .output
        .last()
        .ok_or_else(|| anyhow!("cached block {expected}: coinbase has no outputs"))?;
    let payload = trace_payload(out.script_pubkey.as_script())
        .ok_or_else(|| anyhow!("cached block {expected}: no ASP0 trace output on the coinbase"))?;
    decode_traceblock(&payload)
}

/// `OP_RETURN <push "ASP0" ‖ bytes>` → bytes.
fn trace_payload(script: &bitcoin::Script) -> Option<Vec<u8>> {
    let mut it = script.instructions();
    match it.next()? {
        Ok(Instruction::Op(op)) if op == OP_RETURN => {}
        _ => return None,
    }
    let data = match it.next()? {
        Ok(Instruction::PushBytes(pb)) => pb.as_bytes().to_vec(),
        _ => return None,
    };
    if it.next().is_some() || !data.starts_with(TRACE_MAGIC) {
        return None;
    }
    Some(data[TRACE_MAGIC.len()..].to_vec())
}

#[cfg(feature = "tikv-traces")]
pub use client::TikvTraceCache;

#[cfg(feature = "tikv-traces")]
mod client {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    /// RawKV reader on its own small tokio runtime, so it can be called from
    /// sync code that is itself running inside the indexer's runtime.
    pub struct TikvTraceCache {
        rt: tokio::runtime::Runtime,
        client: Arc<tikv_client::RawClient>,
        prefix: String,
        timeout: Duration,
    }

    impl TikvTraceCache {
        pub fn connect(pd: Vec<String>, prefix: String, timeout: Duration) -> Result<Self> {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("tikv-traces")
                .enable_all()
                .build()
                .context("build tikv-traces runtime")?;
            let cfg = tikv_client::Config::default()
                .with_timeout(timeout)
                // a synthetic block runs to ~7 MB
                .with_grpc_max_decoding_message_size(64 << 20);
            let client = rt
                .block_on(tikv_client::RawClient::new_with_config(pd.clone(), cfg))
                .with_context(|| format!("connect tikv-traces PD {pd:?}"))?;
            Ok(Self { rt, client: Arc::new(client), prefix, timeout })
        }

        /// `Ok(None)` on a miss.
        pub fn get_raw(&self, hash: &BlockHash) -> Result<Option<Vec<u8>>> {
            let key = cache_key(&self.prefix, hash);
            let client = Arc::clone(&self.client);
            let timeout = self.timeout;
            let (tx, rx) = std::sync::mpsc::channel();
            self.rt.spawn(async move {
                let r = tokio::time::timeout(timeout, client.get(key)).await;
                let _ = tx.send(r);
            });
            match rx.recv().map_err(|_| anyhow!("tikv-traces worker dropped the request"))? {
                Ok(Ok(v)) => Ok(v),
                Ok(Err(e)) => Err(anyhow!("tikv-traces get {hash}: {e}")),
                Err(_) => Err(anyhow!("tikv-traces get {hash}: timed out after {timeout:?}")),
            }
        }

        fn get_key(&self, key: Vec<u8>) -> Result<Option<Vec<u8>>> {
            let client = Arc::clone(&self.client);
            let timeout = self.timeout;
            let (tx, rx) = std::sync::mpsc::channel();
            self.rt.spawn(async move {
                let r = tokio::time::timeout(timeout, client.get(key)).await;
                let _ = tx.send(r);
            });
            match rx.recv().map_err(|_| anyhow!("tikv-traces worker dropped the request"))? {
                Ok(Ok(v)) => Ok(v),
                Ok(Err(e)) => Err(anyhow!("tikv-traces get: {e}")),
                Err(_) => Err(anyhow!("tikv-traces get: timed out after {timeout:?}")),
            }
        }

        /// The alias record for `id`, if the cache has been decorated with it.
        pub fn alias(
            &self,
            id: &crate::schemas::SchemaAlkaneId,
        ) -> Result<Option<crate::schemas::SchemaAlkaneId>> {
            self.get_key(alias_key(id))?.map(|v| decode_alias(&v)).transpose()
        }

        pub fn traces(&self, height: u32, hash: &BlockHash) -> Result<Vec<PartialEspoTrace>> {
            match self.get_raw(hash)? {
                Some(v) => decode_cached_block(&v, hash)
                    .with_context(|| format!("trace cache entry for height {height}")),
                None => bail!("{MISS_MESSAGE}{height} ({hash})"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alkanes_support::proto::alkanes::{AlkanesBlockEvent, AlkanesBlockTraceEvent, AlkanesTrace, Outpoint};
    use bitcoin::script::{Builder, PushBytesBuf};
    use prost::Message;

    fn synthetic(tb: &[u8]) -> (String, BlockHash) {
        let mut block: bitcoin::Block = bitcoin::constants::genesis_block(bitcoin::Network::Regtest);
        let mut payload = TRACE_MAGIC.to_vec();
        payload.extend_from_slice(tb);
        let script = Builder::new()
            .push_opcode(OP_RETURN)
            .push_slice(PushBytesBuf::try_from(payload).unwrap())
            .into_script();
        block.txdata[0].output.push(bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(0),
            script_pubkey: script,
        });
        let hash = block.block_hash();
        (hex::encode(bitcoin::consensus::serialize(&block)), hash)
    }

    #[test]
    fn decodes_the_alspo_synthetic_block_and_checks_its_hash() {
        let tb = AlkanesBlockTraceEvent {
            events: vec![AlkanesBlockEvent {
                traces: Some(AlkanesTrace::default()),
                outpoint: Some(Outpoint { txid: vec![9; 32], vout: 2 }),
                txindex: 1,
            }],
        }
        .encode_to_vec();
        let (value, hash) = synthetic(&tb);
        let out = decode_cached_block(value.as_bytes(), &hash).expect("decode");
        assert_eq!(out.len(), 1);
        assert_eq!(&out[0].outpoint[32..], &2u32.to_le_bytes());
        let other = BlockHash::from_byte_array([1; 32]);
        assert!(decode_cached_block(value.as_bytes(), &other).is_err(), "wrong block refused");
    }

    #[test]
    fn an_empty_traceblock_is_an_empty_block_not_a_miss() {
        let (value, hash) = synthetic(&[]);
        assert!(decode_cached_block(value.as_bytes(), &hash).expect("decode").is_empty());
    }

    #[test]
    fn a_block_without_the_trace_output_is_refused() {
        let block: bitcoin::Block = bitcoin::constants::genesis_block(bitcoin::Network::Regtest);
        let hash = block.block_hash();
        let value = hex::encode(bitcoin::consensus::serialize(&block));
        assert!(decode_cached_block(value.as_bytes(), &hash).is_err());
    }

    #[test]
    fn alias_keys_are_disjoint_from_block_keys() {
        let id = crate::schemas::SchemaAlkaneId { block: 2, tx: 103199 };
        let k = alias_key(&id);
        assert_eq!(k, b"asp1:alias:2:103199".to_vec());
        assert!(!k.starts_with(DEFAULT_KEY_PREFIX.as_bytes()));
        assert_eq!(decode_alias(b"4:520").unwrap(), crate::schemas::SchemaAlkaneId { block: 4, tx: 520 });
        assert!(decode_alias(b"garbage").is_err());
    }

    #[test]
    fn key_layout_matches_alspo() {
        let h: BlockHash =
            "0000000000000000000205bc90ca493709067269ea94536c5124ebdfa80ea909".parse().unwrap();
        assert_eq!(
            cache_key(DEFAULT_KEY_PREFIX, &h),
            b"asp1:blk:0000000000000000000205bc90ca493709067269ea94536c5124ebdfa80ea909".to_vec()
        );
    }

    use bitcoin::hashes::Hash as _;
}
