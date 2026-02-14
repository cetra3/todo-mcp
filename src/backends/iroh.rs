use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use iroh::{Endpoint, EndpointId, SecretKey};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{
    Receiver as TokioReceiver, Sender as TokioSender, channel as tokio_channel,
};
use tokio::sync::{Mutex, RwLock};
use tracing::*;

use crate::backends::multicast::SyncMessage;
use crate::backends::proto::{ProtoMessage, run_connection_loop, write_message};

use super::multicast::Site;

const ALPN: &[u8] = b"todo-mcp/v1";

#[cfg(target_os = "android")]
const STORAGE_DIR: &str = "/data/data/dev.cetra.todomcp/files";

#[cfg(not(target_os = "android"))]
const STORAGE_DIR: &str = "~/.local/share/todo_mcp";

fn storage_dir() -> PathBuf {
    shellexpand::tilde(STORAGE_DIR).to_string().into()
}

// --- Secret key persistence ---

fn secret_key_path() -> PathBuf {
    storage_dir().join("iroh_secret_key")
}

fn load_or_generate_secret_key() -> Result<SecretKey> {
    let path = secret_key_path();

    if path.exists() {
        let bytes = std::fs::read(&path)
            .with_context(|| format!("Failed to read secret key from {}", path.display()))?;
        if bytes.len() == 32 {
            let arr: [u8; 32] = bytes.try_into().unwrap();
            Ok(SecretKey::from_bytes(&arr))
        } else {
            warn!("Secret key file has wrong length, generating new key");
            generate_and_save_secret_key(&path)
        }
    } else {
        generate_and_save_secret_key(&path)
    }
}

fn generate_and_save_secret_key(path: &PathBuf) -> Result<SecretKey> {
    let key = SecretKey::generate(&mut rand::rng());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, key.to_bytes())?;
    Ok(key)
}

// --- Known peers persistence ---

fn known_peers_path() -> PathBuf {
    storage_dir().join("known_peers.json")
}

#[derive(Serialize, Deserialize, Default)]
struct KnownPeersFile {
    peers: Vec<String>,
}

fn load_known_peers() -> HashSet<EndpointId> {
    let path = known_peers_path();
    if !path.exists() {
        return HashSet::new();
    }

    match std::fs::read_to_string(&path) {
        Ok(data) => match serde_json::from_str::<KnownPeersFile>(&data) {
            Ok(file) => file
                .peers
                .iter()
                .filter_map(|s| s.parse::<EndpointId>().ok())
                .collect(),
            Err(e) => {
                warn!("Failed to parse known peers: {e}");
                HashSet::new()
            }
        },
        Err(e) => {
            warn!("Failed to read known peers: {e}");
            HashSet::new()
        }
    }
}

fn save_known_peers(peers: &HashSet<EndpointId>) {
    let file = KnownPeersFile {
        peers: peers.iter().map(|p| p.to_string()).collect(),
    };

    let path = known_peers_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    match serde_json::to_string_pretty(&file) {
        Ok(data) => {
            if let Err(e) = std::fs::write(&path, data) {
                warn!("Failed to save known peers: {e}");
            }
        }
        Err(e) => warn!("Failed to serialize known peers: {e}"),
    }
}

// --- EndpointId to site_id mapping ---

pub fn endpoint_id_to_site_id(id: &EndpointId) -> u32 {
    u32::from_be_bytes(id.as_bytes()[0..4].try_into().unwrap())
}

// --- Connection state ---

type ActivePeers = Arc<RwLock<HashMap<EndpointId, tokio::sync::broadcast::Sender<SyncMessage>>>>;
type KnownPeers = Arc<Mutex<HashSet<EndpointId>>>;

// --- Per-connection handler ---

async fn handle_connection(
    conn: iroh::endpoint::Connection,
    our_id: EndpointId,
    _site_id: u32,
    inbound_tx: TokioSender<ProtoMessage<SyncMessage>>,
    active_peers: ActivePeers,
    known_peers: KnownPeers,
    site: Site,
    is_initiator: bool,
) {
    let remote_id = conn.remote_id();
    let remote_site_id = endpoint_id_to_site_id(&remote_id);

    info!(
        "Connection established with peer {} (site_id={}), initiator={}",
        remote_id.fmt_short(),
        remote_site_id,
        is_initiator
    );

    // Add to known peers
    {
        let mut kp = known_peers.lock().await;
        if kp.insert(remote_id) {
            save_known_peers(&kp);
        }
    }

    // Register in active peers with a broadcast channel for outbound messages
    let outbound_rx = {
        let mut peers = active_peers.write().await;
        if peers.contains_key(&remote_id) {
            // Deduplication: lower ID is the designated initiator
            if is_initiator && our_id < remote_id {
                // We're the designated initiator AND we initiated — keep this, close existing
                // Actually the existing one will get cleaned up when it errors
            } else if !is_initiator && our_id > remote_id {
                // We accepted but we're not the designated initiator — drop this
                debug!("Dropping duplicate connection from {}", remote_id.fmt_short());
                conn.close(0u32.into(), b"duplicate");
                return;
            }
        }

        let (tx, rx) = tokio::sync::broadcast::channel::<SyncMessage>(64);
        peers.insert(remote_id, tx);
        rx
    };

    // Open bidirectional stream
    let (mut send, recv) = if is_initiator {
        match conn.open_bi().await {
            Ok(streams) => streams,
            Err(e) => {
                error!("Failed to open bi stream: {e}");
                active_peers.write().await.remove(&remote_id);
                return;
            }
        }
    } else {
        match conn.accept_bi().await {
            Ok(streams) => streams,
            Err(e) => {
                error!("Failed to accept bi stream: {e}");
                active_peers.write().await.remove(&remote_id);
                return;
            }
        }
    };

    // If we're the initiator, send our state as an announce
    if is_initiator {
        let state_bytes = site.read().await.commit.clone().save();
        if let Err(e) = write_message(&mut send, &SyncMessage::Announce(state_bytes)).await {
            error!("Failed to send announce: {e}");
            active_peers.write().await.remove(&remote_id);
            return;
        }
    }

    // Run shared read/write/alive loop
    run_connection_loop(recv, send, remote_site_id, inbound_tx, outbound_rx).await;

    // Cleanup
    active_peers.write().await.remove(&remote_id);
    info!("Connection closed with peer {}", remote_id.fmt_short());
}

// --- Public API ---

pub struct IrohHandle {
    pub endpoint: Endpoint,
    pub our_endpoint_id: EndpointId,
    pub discovered_peer_tx: TokioSender<EndpointId>,
}

pub async fn setup_iroh(
    site: Site,
    inbound_tx: TokioSender<ProtoMessage<SyncMessage>>,
    mut outbound_rx: TokioReceiver<SyncMessage>,
) -> Result<IrohHandle> {
    let secret_key = load_or_generate_secret_key()?;
    let our_endpoint_id = secret_key.public();

    info!(
        "Our iroh EndpointId: {} (site_id={})",
        our_endpoint_id.fmt_short(),
        endpoint_id_to_site_id(&our_endpoint_id)
    );

    let endpoint = Endpoint::builder()
        .secret_key(secret_key)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await?;

    let active_peers: ActivePeers = Arc::new(RwLock::new(HashMap::new()));
    let known_peers: KnownPeers = Arc::new(Mutex::new(load_known_peers()));

    let (discovered_peer_tx, mut discovered_peer_rx) = tokio_channel::<EndpointId>(64);

    // Accept loop
    let accept_endpoint = endpoint.clone();
    let accept_inbound_tx = inbound_tx.clone();
    let accept_active_peers = active_peers.clone();
    let accept_known_peers = known_peers.clone();
    let accept_site = site.clone();
    let accept_our_id = our_endpoint_id;

    tokio::spawn(async move {
        loop {
            let incoming = match accept_endpoint.accept().await {
                Some(incoming) => incoming,
                None => {
                    info!("Endpoint closed, stopping accept loop");
                    break;
                }
            };

            let accepting = match incoming.accept() {
                Ok(accepting) => accepting,
                Err(e) => {
                    warn!("Failed to accept incoming connection: {e}");
                    continue;
                }
            };

            let conn = match accepting.await {
                Ok(conn) => conn,
                Err(e) => {
                    warn!("Connection handshake failed: {e}");
                    continue;
                }
            };

            let inbound_tx = accept_inbound_tx.clone();
            let active_peers = accept_active_peers.clone();
            let known_peers = accept_known_peers.clone();
            let site = accept_site.clone();

            tokio::spawn(handle_connection(
                conn,
                accept_our_id,
                endpoint_id_to_site_id(&accept_our_id),
                inbound_tx,
                active_peers,
                known_peers,
                site,
                false,
            ));
        }
    });

    // Outbound fan-out: forward outbound messages to all active peer broadcast channels
    let fanout_active_peers = active_peers.clone();
    tokio::spawn(async move {
        while let Some(msg) = outbound_rx.recv().await {
            let peers = fanout_active_peers.read().await;
            for (_id, tx) in peers.iter() {
                // Don't care if no receivers
                tx.send(msg.clone()).ok();
            }
        }
    });

    // Discovery loop: connect to newly discovered peers
    let discover_endpoint = endpoint.clone();
    let discover_inbound_tx = inbound_tx.clone();
    let discover_active_peers = active_peers.clone();
    let discover_known_peers = known_peers.clone();
    let discover_site = site.clone();
    let discover_our_id = our_endpoint_id;

    tokio::spawn(async move {
        while let Some(peer_id) = discovered_peer_rx.recv().await {
            if peer_id == discover_our_id {
                continue;
            }

            // Check if already connected
            {
                let peers = discover_active_peers.read().await;
                if peers.contains_key(&peer_id) {
                    continue;
                }
            }

            // Connection deduplication: only initiate if we're the lower ID
            if discover_our_id > peer_id {
                // We're higher ID, so we wait for them to connect to us
                // But add them to known peers so we can try later if needed
                let mut kp = discover_known_peers.lock().await;
                if kp.insert(peer_id) {
                    save_known_peers(&kp);
                }
                continue;
            }

            connect_to_peer(
                &discover_endpoint,
                peer_id,
                discover_our_id,
                discover_inbound_tx.clone(),
                discover_active_peers.clone(),
                discover_known_peers.clone(),
                discover_site.clone(),
            )
            .await;
        }
    });

    // Reconnect loop: every 30s, try to connect to known peers that aren't active
    let reconnect_endpoint = endpoint.clone();
    let reconnect_inbound_tx = inbound_tx.clone();
    let reconnect_active_peers = active_peers.clone();
    let reconnect_known_peers = known_peers.clone();
    let reconnect_site = site.clone();
    let reconnect_our_id = our_endpoint_id;

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;

            let peers_to_connect: Vec<EndpointId> = {
                let known = reconnect_known_peers.lock().await;
                let active = reconnect_active_peers.read().await;
                known
                    .iter()
                    .filter(|p| **p != reconnect_our_id && !active.contains_key(*p))
                    // Only initiate if we're the lower ID
                    .filter(|p| reconnect_our_id < **p)
                    .copied()
                    .collect()
            };

            for peer_id in peers_to_connect {
                debug!("Reconnect attempt to {}", peer_id.fmt_short());
                connect_to_peer(
                    &reconnect_endpoint,
                    peer_id,
                    reconnect_our_id,
                    reconnect_inbound_tx.clone(),
                    reconnect_active_peers.clone(),
                    reconnect_known_peers.clone(),
                    reconnect_site.clone(),
                )
                .await;
            }
        }
    });

    Ok(IrohHandle {
        endpoint,
        our_endpoint_id,
        discovered_peer_tx,
    })
}

async fn connect_to_peer(
    endpoint: &Endpoint,
    peer_id: EndpointId,
    our_id: EndpointId,
    inbound_tx: TokioSender<ProtoMessage<SyncMessage>>,
    active_peers: ActivePeers,
    known_peers: KnownPeers,
    site: Site,
) {
    info!("Connecting to peer {}", peer_id.fmt_short());

    match endpoint.connect(peer_id, ALPN).await {
        Ok(conn) => {
            tokio::spawn(handle_connection(
                conn,
                our_id,
                endpoint_id_to_site_id(&our_id),
                inbound_tx,
                active_peers,
                known_peers,
                site,
                true,
            ));
        }
        Err(e) => {
            warn!("Failed to connect to {}: {e}", peer_id.fmt_short());
        }
    }
}
