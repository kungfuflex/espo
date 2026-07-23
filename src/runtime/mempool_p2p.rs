//! P2P mempool driver — always compiled (native); opt in at runtime with
//! `--mempool-p2p` (or `mempool.source = "p2p"`).
//!
//! Speaks bitcoind's P2P protocol via the `qubitcoin-net` crate and maintains
//! espo's in-memory mempool incrementally from `inv`/`tx` messages, instead of
//! polling `getrawmempool` + bulk `getrawtransaction`. It plugs into the SAME
//! storage path as the RPC ingester (`build_memory_entry` + `upsert_memory_entry`)
//! so lean storage, trace enqueue, RBF handling and events all work unchanged.
//!
//! Outbound-only: it never starts a listener. The `getrawmempool` reconcile in
//! `run_mempool_service` remains active (slowed to a safety-net cadence) to
//! catch anything the incremental stream misses.

use anyhow::{Result, anyhow};
use bitcoin::consensus::encode::deserialize;
use bitcoin::{Network, Transaction};
use std::net::SocketAddr;
use std::time::Duration;

use qubitcoin_net::connection::{ConnConfig, ConnManager, ConnectionEvent, serialize_message};
use qubitcoin_net::protocol::{InvType, InvVect, NetMessage, NetworkMagic, ServiceFlags};

use crate::runtime::mempool::{build_memory_entry, memory_contains_txid, upsert_memory_entry};
use crate::runtime::shutdown::is_shutdown_requested;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Map a rust-bitcoin `Network` to the P2P network magic bytes.
fn magic_for_network(network: Network) -> NetworkMagic {
    match network {
        Network::Bitcoin => NetworkMagic::MAINNET,
        Network::Testnet => NetworkMagic::TESTNET,
        Network::Signet => NetworkMagic::SIGNET,
        Network::Regtest => NetworkMagic::REGTEST,
        // rust-bitcoin's Network is #[non_exhaustive]; default unknown nets to
        // mainnet magic (caller controls the network, so this is only a guard).
        _ => NetworkMagic::MAINNET,
    }
}

/// Run the incremental P2P mempool driver against a single bitcoind peer.
///
/// Never returns on the happy path (it loops over connection events and
/// reconnects with capped backoff on disconnect). Returns `Err` only when the
/// event stream ends or a reconnect fails, so the supervising task in
/// `run_mempool_service` can restart it.
pub async fn run_p2p_mempool_driver(network: Network, peer_addr: SocketAddr) -> Result<()> {
    let config = ConnConfig {
        listen_addr: "127.0.0.1:0".parse().expect("valid loopback addr"),
        magic: magic_for_network(network),
        max_inbound: 0,
        max_outbound: 1,
        our_services: ServiceFlags::NODE_NETWORK | ServiceFlags::NODE_WITNESS,
        user_agent: "/espo-mempool:0.1.0/".to_string(),
        // TODO: wire the real chain height (espo tip) so peers see an accurate
        // start_height. 0 is fine for tx relay — bitcoind still relays mempool
        // txs to a height-0 peer — but is cosmetically wrong.
        best_height: 0,
    };

    let mut cm = ConnManager::new(config);
    let mut events = cm
        .take_events()
        .ok_or_else(|| anyhow!("connection event receiver already taken"))?;

    // Outbound-only — deliberately no start_listening().
    eprintln!("[mempool][p2p] connecting to {peer_addr}");
    cm.connect_to(peer_addr)
        .await
        .map_err(|e| anyhow!("initial connect to {peer_addr} failed: {e}"))?;

    let mut backoff = INITIAL_BACKOFF;

    while let Some(event) = events.recv().await {
        match event {
            ConnectionEvent::HandshakeComplete { peer_id } => {
                backoff = INITIAL_BACKOFF;
                // Deliberately DO NOT send a "mempool" (BIP35) message: bitcoin Core
                // disconnects any peer that requests it unless it was started with
                // -peerbloomfilters=1 (off by default) — "mempool request with bloom
                // filters disabled, disconnecting". New txs arrive automatically via
                // `inv` relay (txrelay negotiated in the version handshake); the
                // initial mempool snapshot comes from the RPC getrawmempool hydration
                // in run_mempool_service. Verified against Core v31 in isolation.
                eprintln!(
                    "[mempool][p2p] handshake complete with peer {peer_id}; ingesting inv/tx relay"
                );
            }
            ConnectionEvent::MessageReceived { peer_id, message } => {
                handle_message(&cm, peer_id, message, network);
            }
            ConnectionEvent::Disconnected { peer_id, reason } => {
                if is_shutdown_requested() {
                    break;
                }
                eprintln!(
                    "[mempool][p2p] peer {peer_id} disconnected ({reason}); reconnecting in {backoff:?}"
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                cm.connect_to(peer_addr)
                    .await
                    .map_err(|e| anyhow!("reconnect to {peer_addr} failed: {e}"))?;
            }
            ConnectionEvent::NewInbound { .. } | ConnectionEvent::NewOutbound { .. } => {}
        }

        if is_shutdown_requested() {
            break;
        }
    }

    cm.shutdown();
    Ok(())
}

/// Handle a single decoded P2P message.
fn handle_message(cm: &ConnManager, peer_id: u64, message: NetMessage, network: Network) {
    match message {
        NetMessage::Inv(invs) => {
            // Request only transaction inventory, upgrading plain tx invs to the
            // witness-serialized form so we receive segwit witnesses. WTx (BIP
            // 339 wtxid) and already-witness invs are requested as advertised so
            // the peer resolves them in the correct hash space.
            let wanted: Vec<InvVect> = invs
                .into_iter()
                .filter_map(|iv| match iv.inv_type {
                    InvType::Tx => Some(InvVect::new(InvType::WitnessTx, iv.hash)),
                    InvType::WTx | InvType::WitnessTx => Some(iv),
                    _ => None,
                })
                .collect();
            if wanted.is_empty() {
                return;
            }
            // send_to_peer wants a header-LESS payload; serialize_message on a
            // GetData yields exactly the inv-vector payload (count varint + each
            // 36-byte InvVect) with no 24-byte wire header.
            let payload = serialize_message(&NetMessage::GetData(wanted.clone()));
            if !cm.send_to_peer(peer_id, "getdata", payload) {
                eprintln!(
                    "[mempool][p2p] failed to queue getdata ({} tx invs) to peer {peer_id}",
                    wanted.len()
                );
            }
        }
        NetMessage::Tx(raw) => match deserialize::<Transaction>(&raw) {
            Ok(tx) => {
                let txid = tx.compute_txid();
                if memory_contains_txid(&txid) {
                    return;
                }
                // Same storage path as the RPC ingester (verbose = None).
                let entry = build_memory_entry(txid, tx, None, network);
                upsert_memory_entry(entry);
            }
            Err(e) => {
                eprintln!(
                    "[mempool][p2p] failed to deserialize tx ({} bytes) from peer {peer_id}: {e}",
                    raw.len()
                );
            }
        },
        NetMessage::Ping(nonce) => {
            // The connection layer already auto-replies pong internally; we mirror
            // it explicitly per the driver contract. Harmless if duplicated.
            let payload = serialize_message(&NetMessage::Pong(nonce));
            let _ = cm.send_to_peer(peer_id, "pong", payload);
        }
        _ => {}
    }
}
