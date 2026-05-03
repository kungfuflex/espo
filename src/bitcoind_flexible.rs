// Flexible Bitcoin RPC client adapter
// Based on ord-rocksdb's approach - uses custom HTTP + JSON-RPC 2.0
// Compatible with both Bitcoin Core and alternative endpoints like Subfrost

use anyhow::Result;
use bitcoin::{Block, BlockHash};
use bitcoincore_rpc::bitcoin;
use bitcoincore_rpc::bitcoincore_rpc_json::{GetBlockHeaderResult, GetBlockchainInfoResult};
use bitcoincore_rpc::{Error as RpcError, RpcApi};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

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
    request_id: AtomicU32,
}

impl FlexibleBitcoindClient {
    pub fn new(url: &str, auth: Option<(String, String)>) -> Result<Self> {
        let auth_header = auth.map(|(user, pass)| {
            let credentials = format!("{}:{}", user, pass);
            format!("Basic {}", base64_encode(credentials.as_bytes()))
        });

        Ok(Self { url: url.to_string(), auth: auth_header, request_id: AtomicU32::new(1) })
    }

    fn parse_http_url(&self) -> Result<(String, u16, String), RpcError> {
        let raw = self.url.strip_prefix("http://").ok_or_else(|| {
            rpc_error(
                -10,
                format!("unsupported bitcoind RPC URL {}; only http:// is supported", self.url),
            )
        })?;
        let (authority, path) = raw.split_once('/').unwrap_or((raw, ""));
        let path = format!("/{}", path);
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => {
                let port = port
                    .parse::<u16>()
                    .map_err(|e| rpc_error(-11, format!("invalid bitcoind RPC port: {e}")))?;
                (host.to_string(), port)
            }
            None => (authority.to_string(), 80),
        };
        if host.is_empty() {
            return Err(rpc_error(-12, "empty bitcoind RPC host".to_string()));
        }
        Ok((host, port, path))
    }

    fn post_json(&self, body: &str) -> Result<String, RpcError> {
        let (host, port, path) = self.parse_http_url()?;
        let mut stream = TcpStream::connect((host.as_str(), port))
            .map_err(|e| rpc_error(-1, format!("HTTP connect failed: {e}")))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(120)))
            .map_err(|e| rpc_error(-1, format!("HTTP set_read_timeout failed: {e}")))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(120)))
            .map_err(|e| rpc_error(-1, format!("HTTP set_write_timeout failed: {e}")))?;

        let mut request = format!(
            "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        if let Some(auth) = self.auth.as_ref() {
            request.push_str(&format!("Authorization: {auth}\r\n"));
        }
        request.push_str("\r\n");
        request.push_str(body);

        stream
            .write_all(request.as_bytes())
            .map_err(|e| rpc_error(-1, format!("HTTP write failed: {e}")))?;
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .map_err(|e| rpc_error(-1, format!("HTTP read failed: {e}")))?;

        let response = String::from_utf8(response)
            .map_err(|e| rpc_error(-2, format!("HTTP response was not UTF-8: {e}")))?;
        let (head, body) = response
            .split_once("\r\n\r\n")
            .ok_or_else(|| rpc_error(-2, "malformed HTTP response".to_string()))?;
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse::<u16>().ok())
            .ok_or_else(|| rpc_error(-2, "missing HTTP status".to_string()))?;
        if !(200..300).contains(&status) {
            return Err(rpc_error(status as i32, format!("HTTP {status}: {body}")));
        }
        Ok(body.to_string())
    }

    fn rpc_call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Vec<Value>,
    ) -> Result<T, RpcError> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);

        let request =
            JsonRpcRequest { jsonrpc: "2.0".to_string(), id, method: method.to_string(), params };

        let body = serde_json::to_string(&request)
            .map_err(|e| rpc_error(-2, format!("Failed to encode JSON-RPC request: {e}")))?;
        let response_body = self.post_json(&body)?;
        let rpc_response: JsonRpcResponse<T> = serde_json::from_str(&response_body)
            .map_err(|e| rpc_error(-2, format!("Failed to parse JSON-RPC response: {e}")))?;

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

fn rpc_error(code: i32, message: String) -> RpcError {
    RpcError::JsonRpc(bitcoincore_rpc::jsonrpc::Error::Rpc(
        bitcoincore_rpc::jsonrpc::error::RpcError { code, message, data: None },
    ))
}
