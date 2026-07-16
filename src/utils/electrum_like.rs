use anyhow::{Context, Result, bail};
use bitcoin::{Address, Txid};
use electrum_client::{Client as ElectrumClient, ElectrumApi, Param};
use futures::{StreamExt, stream};
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::future::Future;
use std::str::FromStr;
use std::sync::Arc;
use tokio::runtime::{Handle, Runtime};

const ESPLORA_RAW_TX_CONCURRENCY: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElectrumLikeBackend {
    ElectrumRpc,
    EsploraHttp,
}

#[derive(Clone, Debug)]
pub struct AddressStats {
    pub backend: ElectrumLikeBackend,
    pub confirmed_balance: Option<u64>,
    pub total_received: Option<u64>,
    pub confirmed_utxos: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AddressSummary {
    pub address: String,
    pub chain_stats: AddressTxStats,
    pub mempool_stats: AddressTxStats,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AddressTxStats {
    pub funded_txo_count: u64,
    pub funded_txo_sum: u64,
    pub spent_txo_count: u64,
    pub spent_txo_sum: u64,
    pub tx_count: u64,
}

#[derive(Clone, Debug)]
pub struct AddressHistoryEntry {
    pub txid: Txid,
    pub height: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct AddressHistoryPage {
    pub entries: Vec<AddressHistoryEntry>,
    pub total: Option<usize>,
    pub has_more: bool,
}

#[derive(Clone, Debug)]
pub struct AddressUtxo {
    pub txid: Txid,
    pub vout: u32,
    pub value: u64,
    pub block_height: Option<u64>,
    pub confirmed: bool,
}

/// Minimal interface needed by ammdata for fetching raw transactions.
pub trait ElectrumLike: Send + Sync {
    fn backend(&self) -> ElectrumLikeBackend;
    fn batch_transaction_get_raw(&self, txids: &[Txid]) -> Result<Vec<Vec<u8>>>;
    fn transaction_get_raw(&self, txid: &Txid) -> Result<Vec<u8>>;
    fn transaction_broadcast_raw(&self, raw_tx: &[u8]) -> Result<Txid>;
    fn tip_height(&self) -> Result<u32>;
    fn transaction_get_outspends(&self, txid: &Txid) -> Result<Vec<Option<Txid>>>;
    fn batch_transaction_get_outspends(&self, txids: &[Txid]) -> Result<Vec<Vec<Option<Txid>>>>;
    fn transaction_get_height(&self, txid: &Txid) -> Result<Option<u64>>;
    fn address_summary(&self, address: &Address) -> Result<AddressSummary>;
    fn address_stats(&self, address: &Address) -> Result<AddressStats>;
    fn address_history_page(
        &self,
        address: &Address,
        offset: usize,
        limit: usize,
    ) -> Result<AddressHistoryPage>;
    fn address_history_page_cursor(
        &self,
        address: &Address,
        cursor: Option<&Txid>,
        limit: usize,
    ) -> Result<AddressHistoryPage>;
    fn address_utxos(&self, address: &Address) -> Result<Vec<AddressUtxo>>;
}

/// Thin wrapper over the native Electrum RPC client.
pub struct ElectrumRpcClient {
    client: Arc<ElectrumClient>,
}

impl ElectrumRpcClient {
    pub fn new(client: Arc<ElectrumClient>) -> Self {
        Self { client }
    }
}

impl ElectrumLike for ElectrumRpcClient {
    fn backend(&self) -> ElectrumLikeBackend {
        ElectrumLikeBackend::ElectrumRpc
    }

    fn batch_transaction_get_raw(&self, txids: &[Txid]) -> Result<Vec<Vec<u8>>> {
        self.client
            .batch_transaction_get_raw(txids)
            .context("electrum batch_transaction_get_raw")
    }

    fn transaction_get_raw(&self, txid: &Txid) -> Result<Vec<u8>> {
        self.client.transaction_get_raw(txid).context("electrum transaction_get_raw")
    }

    fn transaction_broadcast_raw(&self, raw_tx: &[u8]) -> Result<Txid> {
        self.client
            .transaction_broadcast_raw(raw_tx)
            .context("electrum transaction_broadcast_raw")
    }

    fn tip_height(&self) -> Result<u32> {
        let sub = self
            .client
            .block_headers_subscribe_raw()
            .context("electrum: blockchain.headers.subscribe failed")?;

        let tip: u32 = sub
            .height
            .try_into()
            .map_err(|_| anyhow::anyhow!("electrum: tip height doesn't fit into u32"))?;
        Ok(tip)
    }

    fn transaction_get_outspends(&self, txid: &Txid) -> Result<Vec<Option<Txid>>> {
        // Electrum protocol does not standardize outspends; try verbose get (electrs supports
        // `outspends` field) and fall back to empty.
        let params = vec![Param::String(txid.to_string()), Param::Bool(true)];
        let resp = self
            .client
            .raw_call("blockchain.transaction.get", params)
            .context("electrum transaction.get (verbose) for outspends")?;

        Ok(extract_outspends(&resp))
    }

    fn batch_transaction_get_outspends(&self, txids: &[Txid]) -> Result<Vec<Vec<Option<Txid>>>> {
        let mut out = Vec::with_capacity(txids.len());
        for txid in txids {
            let res = self.transaction_get_outspends(txid).unwrap_or_default();
            out.push(res);
        }
        Ok(out)
    }

    fn transaction_get_height(&self, txid: &Txid) -> Result<Option<u64>> {
        let params = vec![Param::String(txid.to_string()), Param::Bool(true)];
        let resp = self
            .client
            .raw_call("blockchain.transaction.get", params)
            .context("electrum transaction.get (verbose) for height")?;

        Ok(extract_height(&resp))
    }

    fn address_summary(&self, _address: &Address) -> Result<AddressSummary> {
        Err(anyhow::anyhow!(
            "exact address summaries require an electrs/esplora HTTP backend; electrum RPC is not supported"
        ))
    }

    fn address_stats(&self, address: &Address) -> Result<AddressStats> {
        let script = address.script_pubkey();
        let balance =
            self.client.script_get_balance(&script).context("electrum script_get_balance")?;
        let utxos = self
            .client
            .script_list_unspent(&script)
            .context("electrum script_list_unspent")?;
        let confirmed_utxos = utxos.into_iter().filter(|u| u.height > 0).count();

        Ok(AddressStats {
            backend: ElectrumLikeBackend::ElectrumRpc,
            confirmed_balance: Some(balance.confirmed),
            total_received: None,
            confirmed_utxos: Some(confirmed_utxos),
        })
    }

    fn address_history_page(
        &self,
        address: &Address,
        offset: usize,
        limit: usize,
    ) -> Result<AddressHistoryPage> {
        let script = address.script_pubkey();
        let history =
            self.client.script_get_history(&script).context("electrum script_get_history")?;
        let total = history.len();
        let slice = history.into_iter().skip(offset).take(limit);
        let mut out = Vec::new();
        for h in slice {
            let height = if h.height > 0 { Some(h.height as u64) } else { None };
            out.push(AddressHistoryEntry { txid: h.tx_hash, height });
        }
        Ok(AddressHistoryPage {
            entries: out,
            total: Some(total),
            has_more: offset + limit < total,
        })
    }

    fn address_history_page_cursor(
        &self,
        _address: &Address,
        _cursor: Option<&Txid>,
        _limit: usize,
    ) -> Result<AddressHistoryPage> {
        Err(anyhow::anyhow!("electrum cursor pagination not supported"))
    }

    fn address_utxos(&self, _address: &Address) -> Result<Vec<AddressUtxo>> {
        Err(anyhow::anyhow!(
            "address UTXO fetch requires an electrs/esplora HTTP backend; electrum RPC is not supported"
        ))
    }
}

/// Esplora-backed implementation that hits `/tx/:txid/raw`.
pub struct EsploraElectrumLike {
    base_url: String,
    http: HttpClient,
}

impl EsploraElectrumLike {
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let mut url = base_url.into();
        while url.ends_with('/') {
            url.pop();
        }
        Ok(Self { base_url: url, http: HttpClient::new() })
    }

    async fn fetch_one_indexed(&self, idx: usize, txid: &Txid) -> Result<(usize, Vec<u8>)> {
        if crate::runtime::shutdown::is_shutdown_requested() {
            bail!("shutdown requested");
        }
        let url = format!("{}/tx/{}/raw", self.base_url, txid);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("esplora GET {url} failed"))?
            .error_for_status()
            .with_context(|| format!("esplora GET {url} returned error status"))?;

        let bytes = resp.bytes().await.context("esplora response body read failed")?;
        Ok((idx, bytes.to_vec()))
    }

    async fn broadcast_raw(&self, raw_tx: &[u8]) -> Result<Txid> {
        let url = format!("{}/tx", self.base_url);
        let resp = self
            .http
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "text/plain")
            .body(hex::encode(raw_tx))
            .send()
            .await
            .with_context(|| format!("esplora POST {url} failed"))?;
        let status = resp.status();
        let body = resp.text().await.context("esplora broadcast response body read failed")?;
        if !status.is_success() {
            bail!("esplora transaction broadcast returned {status}: {}", body.trim());
        }
        Txid::from_str(body.trim()).context("invalid txid in esplora broadcast response")
    }

    fn block_on_result<F, T>(&self, fut: F) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        if crate::runtime::shutdown::is_shutdown_requested() {
            bail!("shutdown requested");
        }
        match Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
            Err(_) => {
                let rt = Runtime::new().context("failed to build ad-hoc Tokio runtime")?;
                rt.block_on(fut)
            }
        }
    }

    async fn fetch_address_summary(&self, address: &str) -> Result<AddressSummary> {
        let url = format!("{}/address/{}", self.base_url, address);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("esplora GET {url} failed"))?
            .error_for_status()
            .with_context(|| format!("esplora GET {url} returned error status"))?;

        let body_str = resp.text().await.context("esplora address body read failed")?;
        let summary: AddressSummary =
            serde_json::from_str(&body_str).context("esplora address json decode failed")?;
        Ok(summary)
    }

    async fn fetch_address_utxos(&self, address: &str) -> Result<Vec<EsploraUtxo>> {
        let url = format!("{}/address/{}/utxo", self.base_url, address);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("esplora GET {url} failed"))?
            .error_for_status()
            .with_context(|| format!("esplora GET {url} returned error status"))?;

        let body_str = resp.text().await.context("esplora utxo body read failed")?;
        let utxos: Vec<EsploraUtxo> =
            serde_json::from_str(&body_str).context("esplora utxo json decode failed")?;
        Ok(utxos)
    }

    async fn fetch_outspends_single(&self, txid: &Txid) -> Result<Vec<Option<Txid>>> {
        let url = format!("{}/tx/{}/outspends", self.base_url, txid);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("esplora GET {url} failed"))?
            .error_for_status()
            .with_context(|| format!("esplora GET {url} returned error status"))?;

        let body_str = resp.text().await.context("esplora outspends body read failed")?;
        let body: Vec<EsploraOutspend> =
            serde_json::from_str(&body_str).context("esplora outspends json decode failed")?;

        let mut out = Vec::with_capacity(body.len());
        for o in body {
            if o.spent {
                out.push(o.txid.and_then(|s| Txid::from_str(&s).ok()));
            } else {
                out.push(None);
            }
        }
        Ok(out)
    }

    async fn fetch_outspends_batch(&self, txids: &[Txid]) -> Vec<Vec<Option<Txid>>> {
        let futs = txids
            .iter()
            .enumerate()
            .map(|(idx, txid)| async move { (idx, txid, self.fetch_outspends_single(txid).await) });
        let mut out = vec![Vec::new(); txids.len()];
        for (idx, txid, res) in futures::future::join_all(futs).await {
            match res {
                Ok(v) => out[idx] = v,
                Err(e) => {
                    eprintln!("[esplora] failed to fetch outspends for {txid}: {e:?}");
                    out[idx] = Vec::new();
                }
            }
        }
        out
    }
    async fn fetch_address_txs(&self, url: &str) -> Result<Vec<EsploraAddressTx>> {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .with_context(|| format!("esplora GET {url} failed"))?
            .error_for_status()
            .with_context(|| format!("esplora GET {url} returned error status"))?;

        let body_str = resp.text().await.context("esplora address txs body read failed")?;
        let txs: Vec<EsploraAddressTx> =
            serde_json::from_str(&body_str).context("esplora address txs json decode failed")?;
        Ok(txs)
    }
}

impl ElectrumLike for EsploraElectrumLike {
    fn backend(&self) -> ElectrumLikeBackend {
        ElectrumLikeBackend::EsploraHttp
    }

    fn batch_transaction_get_raw(&self, txids: &[Txid]) -> Result<Vec<Vec<u8>>> {
        if txids.is_empty() {
            return Ok(Vec::new());
        }

        self.block_on_result(async {
            let mut futs = stream::iter(txids.iter().enumerate())
                .map(|(idx, txid)| async move {
                    let txid = *txid;
                    (txid, self.fetch_one_indexed(idx, &txid).await)
                })
                .buffer_unordered(ESPLORA_RAW_TX_CONCURRENCY);
            let mut out = vec![Vec::new(); txids.len()];
            while let Some((_, res)) = futs.next().await {
                if crate::runtime::shutdown::is_shutdown_requested() {
                    break;
                }
                match res {
                    Ok((idx, raw)) => out[idx] = raw,
                    Err(e) => {
                        if crate::runtime::shutdown::is_shutdown_requested() {
                            break;
                        }
                        eprintln!("[esplora] failed to fetch raw tx: {e:?}");
                    }
                }
            }
            Ok(out)
        })
    }

    fn transaction_get_raw(&self, txid: &Txid) -> Result<Vec<u8>> {
        self.block_on_result(async {
            let (_, raw) = self.fetch_one_indexed(0, txid).await?;
            Ok(raw)
        })
    }

    fn transaction_broadcast_raw(&self, raw_tx: &[u8]) -> Result<Txid> {
        self.block_on_result(self.broadcast_raw(raw_tx))
    }

    fn tip_height(&self) -> Result<u32> {
        self.block_on_result(async {
            let url = format!("{}/blocks/tip/height", self.base_url);
            let resp = self
                .http
                .get(&url)
                .send()
                .await
                .with_context(|| format!("esplora GET {url} failed"))?
                .error_for_status()
                .with_context(|| format!("esplora GET {url} returned error status"))?;

            let body = resp.text().await.context("esplora tip height body read failed")?;
            let tip: u32 = body
                .trim()
                .parse()
                .with_context(|| format!("failed to parse esplora tip height from '{body}'"))?;
            Ok(tip)
        })
    }

    fn transaction_get_outspends(&self, txid: &Txid) -> Result<Vec<Option<Txid>>> {
        self.block_on_result(async { self.fetch_outspends_single(txid).await })
    }

    fn batch_transaction_get_outspends(&self, txids: &[Txid]) -> Result<Vec<Vec<Option<Txid>>>> {
        if txids.is_empty() {
            return Ok(Vec::new());
        }
        self.block_on_result(async { Ok(self.fetch_outspends_batch(txids).await) })
    }

    fn transaction_get_height(&self, txid: &Txid) -> Result<Option<u64>> {
        self.block_on_result(async {
            let url = format!("{}/tx/{}", self.base_url, txid);
            let resp = self
                .http
                .get(&url)
                .send()
                .await
                .with_context(|| format!("esplora GET {url} failed"))?
                .error_for_status()
                .with_context(|| format!("esplora GET {url} returned error status"))?;

            let body_str = resp.text().await.context("esplora tx body read failed")?;
            let body: serde_json::Value =
                serde_json::from_str(&body_str).context("esplora tx json decode failed")?;

            let height =
                body.get("status").and_then(|s| s.get("block_height")).and_then(|h| h.as_u64());
            Ok(height)
        })
    }

    fn address_summary(&self, address: &Address) -> Result<AddressSummary> {
        self.block_on_result(self.fetch_address_summary(&address.to_string()))
    }

    fn address_stats(&self, address: &Address) -> Result<AddressStats> {
        let addr = address.to_string();
        self.block_on_result(async {
            let summary = self.fetch_address_summary(&addr).await?;
            let utxos = self.fetch_address_utxos(&addr).await?;

            let confirmed_balance = summary
                .chain_stats
                .funded_txo_sum
                .saturating_sub(summary.chain_stats.spent_txo_sum);
            let confirmed_utxos = utxos.into_iter().filter(|u| u.status.confirmed).count();

            Ok(AddressStats {
                backend: ElectrumLikeBackend::EsploraHttp,
                confirmed_balance: Some(confirmed_balance),
                total_received: Some(summary.chain_stats.funded_txo_sum),
                confirmed_utxos: Some(confirmed_utxos),
            })
        })
    }

    fn address_history_page(
        &self,
        address: &Address,
        offset: usize,
        limit: usize,
    ) -> Result<AddressHistoryPage> {
        let addr = address.to_string();
        self.block_on_result(async {
            let mut out: Vec<AddressHistoryEntry> = Vec::new();
            let mut seen: HashSet<Txid> = HashSet::new();
            let mut last_seen: Option<String> = None;
            let mut consumed: usize = 0;
            let page_size: usize = 25;

            // Best-effort total via summary.
            let summary = self.fetch_address_summary(&addr).await.ok();
            let total = summary.as_ref().map(|s| s.chain_stats.tx_count as usize);

            while out.len() < limit {
                let url = match last_seen.as_ref() {
                    Some(txid) => format!("{}/address/{}/txs/chain/{}", self.base_url, addr, txid),
                    None => format!("{}/address/{}/txs", self.base_url, addr),
                };
                let page = self.fetch_address_txs(&url).await?;
                if page.is_empty() {
                    break;
                }

                let page_len = page.len();
                // Skip until we reach the requested offset.
                if consumed + page_len <= offset {
                    consumed += page_len;
                    last_seen = page.last().map(|t| t.txid.clone());
                    continue;
                }

                let start_idx = offset.saturating_sub(consumed);
                for tx in page.iter().skip(start_idx) {
                    if let Ok(txid) = Txid::from_str(&tx.txid) {
                        if seen.insert(txid) {
                            out.push(AddressHistoryEntry { txid, height: tx.status.block_height });
                            if out.len() >= limit {
                                break;
                            }
                        }
                    }
                }

                consumed += page_len;
                let next = page.last().map(|t| t.txid.clone());
                if next.is_none() || next == last_seen {
                    break;
                }
                last_seen = next;

                if page_len < page_size {
                    break;
                }
            }

            let has_more = if let Some(total) = total {
                offset + out.len() < total
            } else {
                out.len() == limit
            };

            Ok(AddressHistoryPage { entries: out, total, has_more })
        })
    }

    fn address_history_page_cursor(
        &self,
        address: &Address,
        cursor: Option<&Txid>,
        limit: usize,
    ) -> Result<AddressHistoryPage> {
        let addr = address.to_string();
        self.block_on_result(async {
            let mut out: Vec<AddressHistoryEntry> = Vec::new();
            let mut seen: HashSet<Txid> = HashSet::new();
            let mut last_seen: Option<String> = cursor.map(|t| t.to_string());
            let mut last_page_len: usize = 0;
            let page_size: usize = 25;

            let summary = self.fetch_address_summary(&addr).await.ok();
            let total = summary.as_ref().map(|s| s.chain_stats.tx_count as usize);

            while out.len() < limit {
                let url = match last_seen.as_ref() {
                    Some(txid) => format!("{}/address/{}/txs/chain/{}", self.base_url, addr, txid),
                    None => format!("{}/address/{}/txs", self.base_url, addr),
                };
                let page = self.fetch_address_txs(&url).await?;
                if page.is_empty() {
                    last_page_len = 0;
                    break;
                }

                last_page_len = page.len();
                for tx in page.iter() {
                    if let Ok(txid) = Txid::from_str(&tx.txid) {
                        if seen.insert(txid) {
                            out.push(AddressHistoryEntry { txid, height: tx.status.block_height });
                            if out.len() >= limit {
                                break;
                            }
                        }
                    }
                }

                let next = page.last().map(|t| t.txid.clone());
                if next.is_none() || next == last_seen {
                    break;
                }
                last_seen = next;

                if last_page_len < page_size {
                    break;
                }
            }

            let has_more = if let Some(total) = total {
                out.len() < total
            } else {
                out.len() >= limit && last_page_len == page_size
            };

            Ok(AddressHistoryPage { entries: out, total, has_more })
        })
    }

    fn address_utxos(&self, address: &Address) -> Result<Vec<AddressUtxo>> {
        let addr = address.to_string();
        self.block_on_result(async {
            let utxos = self.fetch_address_utxos(&addr).await?;
            let mut out = Vec::with_capacity(utxos.len());
            for utxo in utxos {
                let txid = Txid::from_str(&utxo.txid)
                    .with_context(|| format!("invalid txid in esplora utxo: {}", utxo.txid))?;
                out.push(AddressUtxo {
                    txid,
                    vout: utxo.vout,
                    value: utxo.value,
                    block_height: utxo.status.block_height,
                    confirmed: utxo.status.confirmed,
                });
            }
            Ok(out)
        })
    }
}

fn extract_height(resp: &serde_json::Value) -> Option<u64> {
    resp.get("height")
        .or_else(|| resp.get("blockheight"))
        .or_else(|| resp.get("block_height"))
        .and_then(|h| h.as_i64())
        .and_then(|h| if h >= 0 { Some(h as u64) } else { None })
}

#[derive(Deserialize)]
struct EsploraUtxo {
    txid: String,
    vout: u32,
    value: u64,
    status: EsploraUtxoStatus,
}

#[derive(Deserialize)]
struct EsploraUtxoStatus {
    confirmed: bool,
    block_height: Option<u64>,
}

#[derive(Deserialize)]
struct EsploraOutspend {
    spent: bool,
    txid: Option<String>,
}

#[derive(Deserialize)]
struct EsploraAddressTx {
    txid: String,
    status: EsploraAddressTxStatus,
}

#[derive(Deserialize)]
struct EsploraAddressTxStatus {
    #[allow(dead_code)]
    confirmed: bool,
    block_height: Option<u64>,
}

fn extract_outspends(v: &serde_json::Value) -> Vec<Option<Txid>> {
    let Some(arr) = v.get("outspends").and_then(|o| o.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let spent_flag = item.get("spent").and_then(|b| b.as_bool());
        let spender =
            item.get("txid").and_then(|t| t.as_str()).and_then(|s| Txid::from_str(s).ok());
        let spent = spent_flag.unwrap_or_else(|| spender.is_some());
        out.push(if spent { spender } else { None });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn address_summary_preserves_esplora_response_shape() {
        let expected = json!({
            "address": "1wiz18xYmhRX6xStj2b9t1rwWX4GKUgpv",
            "chain_stats": {
                "funded_txo_count": 11,
                "funded_txo_sum": 15_007_688_098u64,
                "spent_txo_count": 5,
                "spent_txo_sum": 15_007_599_040u64,
                "tx_count": 13
            },
            "mempool_stats": {
                "funded_txo_count": 0,
                "funded_txo_sum": 0,
                "spent_txo_count": 0,
                "spent_txo_sum": 0,
                "tx_count": 0
            }
        });

        let summary: AddressSummary = serde_json::from_value(expected.clone()).unwrap();

        assert_eq!(serde_json::to_value(summary).unwrap(), expected);
    }
}
