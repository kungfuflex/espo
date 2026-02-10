// Flexible Bitcoin RPC client adapter
// Based on ord-rocksdb's approach - uses custom HTTP + JSON-RPC 2.0
// Compatible with both Bitcoin Core and alternative endpoints like Subfrost

use anyhow::{Result, anyhow};
use bitcoin::{Block, BlockHash};
use bitcoincore_rpc::bitcoin;
use bitcoincore_rpc::bitcoincore_rpc_json::{GetBlockHeaderResult, GetBlockchainInfoResult};
use bitcoincore_rpc::{Error as RpcError, RpcApi};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: u32,
    method: String,
    params: Vec<Value>,
}

#[derive(Deserialize, Debug)]
struct JsonRpcResponse<T> {
    result: Option<T>,
    error: Option<JsonRpcErrorDetail>,
}

#[derive(Deserialize, Debug)]
struct JsonRpcErrorDetail {
    code: i32,
    message: String,
}

pub struct FlexibleBitcoindClient {
    url: String,
    auth: Option<String>,
    client: reqwest::blocking::Client,
    request_id: AtomicU32,
}

impl FlexibleBitcoindClient {
    pub fn new(url: &str, auth: Option<(String, String)>) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| anyhow!("Failed to create HTTP client: {}", e))?;

        let auth_header = auth.map(|(user, pass)| {
            let credentials = format!("{}:{}", user, pass);
            format!("Basic {}", base64_encode(credentials.as_bytes()))
        });

        Ok(Self { url: url.to_string(), auth: auth_header, client, request_id: AtomicU32::new(1) })
    }

    fn rpc_call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Vec<Value>,
    ) -> Result<T, RpcError> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);

        let request =
            JsonRpcRequest { jsonrpc: "2.0".to_string(), id, method: method.to_string(), params };

        let mut req = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .json(&request);

        if let Some(ref auth) = self.auth {
            req = req.header("Authorization", auth);
        }

        let response = req.send().map_err(|e| {
            RpcError::JsonRpc(bitcoincore_rpc::jsonrpc::Error::Rpc(
                bitcoincore_rpc::jsonrpc::error::RpcError {
                    code: -1,
                    message: format!("HTTP request failed: {}", e),
                    data: None,
                },
            ))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(RpcError::JsonRpc(bitcoincore_rpc::jsonrpc::Error::Rpc(
                bitcoincore_rpc::jsonrpc::error::RpcError {
                    code: status.as_u16() as i32,
                    message: format!("HTTP {}: {}", status, body),
                    data: None,
                },
            )));
        }

        let rpc_response: JsonRpcResponse<T> = response.json().map_err(|e| {
            RpcError::JsonRpc(bitcoincore_rpc::jsonrpc::Error::Rpc(
                bitcoincore_rpc::jsonrpc::error::RpcError {
                    code: -2,
                    message: format!("Failed to parse JSON-RPC response: {}", e),
                    data: None,
                },
            ))
        })?;

        if let Some(error) = rpc_response.error {
            return Err(RpcError::JsonRpc(bitcoincore_rpc::jsonrpc::Error::Rpc(
                bitcoincore_rpc::jsonrpc::error::RpcError {
                    code: error.code,
                    message: error.message,
                    data: None,
                },
            )));
        }

        rpc_response.result.ok_or_else(|| {
            RpcError::JsonRpc(bitcoincore_rpc::jsonrpc::Error::Rpc(
                bitcoincore_rpc::jsonrpc::error::RpcError {
                    code: -3,
                    message: "Missing result field in response".to_string(),
                    data: None,
                },
            ))
        })
    }
}

impl RpcApi for FlexibleBitcoindClient {
    fn call<T: for<'a> serde::de::Deserialize<'a>>(
        &self,
        cmd: &str,
        args: &[serde_json::Value],
    ) -> Result<T, RpcError> {
        self.rpc_call(cmd, args.to_vec())
    }

    fn get_block_hash(&self, height: u64) -> Result<BlockHash, RpcError> {
        let hash_str: String = self.rpc_call("getblockhash", vec![json!(height)])?;
        hash_str.parse().map_err(|e| {
            RpcError::JsonRpc(bitcoincore_rpc::jsonrpc::Error::Rpc(
                bitcoincore_rpc::jsonrpc::error::RpcError {
                    code: -4,
                    message: format!("Invalid block hash: {}", e),
                    data: None,
                },
            ))
        })
    }

    fn get_block(&self, hash: &BlockHash) -> Result<Block, RpcError> {
        let block_hex: String =
            self.rpc_call("getblock", vec![json!(hash.to_string()), json!(0)])?;
        let block_bytes = hex::decode(&block_hex).map_err(|e| {
            RpcError::JsonRpc(bitcoincore_rpc::jsonrpc::Error::Rpc(
                bitcoincore_rpc::jsonrpc::error::RpcError {
                    code: -5,
                    message: format!("Invalid hex in block response: {}", e),
                    data: None,
                },
            ))
        })?;
        bitcoin::consensus::deserialize(&block_bytes).map_err(|e| {
            RpcError::JsonRpc(bitcoincore_rpc::jsonrpc::Error::Rpc(
                bitcoincore_rpc::jsonrpc::error::RpcError {
                    code: -6,
                    message: format!("Failed to deserialize block: {}", e),
                    data: None,
                },
            ))
        })
    }

    fn get_block_header_info(&self, hash: &BlockHash) -> Result<GetBlockHeaderResult, RpcError> {
        self.rpc_call("getblockheader", vec![json!(hash.to_string()), json!(true)])
    }

    fn get_blockchain_info(&self) -> Result<GetBlockchainInfoResult, RpcError> {
        self.rpc_call("getblockchaininfo", vec![])
    }

    fn get_block_count(&self) -> Result<u64, RpcError> {
        self.rpc_call("getblockcount", vec![])
    }
}

fn base64_encode(input: &[u8]) -> String {
    use std::io::Write;
    let mut buf = Vec::new();
    {
        let mut encoder =
            base64::write::EncoderWriter::new(&mut buf, &base64::engine::general_purpose::STANDARD);
        encoder.write_all(input).unwrap();
    }
    String::from_utf8(buf).unwrap()
}
