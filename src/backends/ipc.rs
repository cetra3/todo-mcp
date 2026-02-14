use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc::{Receiver as TokioReceiver, Sender as TokioSender};
use tracing::*;

use crate::backends::multicast::{Site, SyncMessage};
use crate::backends::proto::{ProtoMessage, run_connection_loop, write_message};

/// Parse a site_id from a socket filename like "12345.sock".
fn parse_site_id_from_path(path: &std::path::Path) -> Option<u32> {
    path.file_stem()?
        .to_str()?
        .parse::<u32>()
        .ok()
}

/// Handle a single IPC connection (either accepted or initiated).
async fn handle_ipc_connection(
    stream: UnixStream,
    _our_site_id: u32,
    remote_site_id: u32,
    site: Site,
    inbound_tx: TokioSender<ProtoMessage<SyncMessage>>,
    outbound_rx: tokio::sync::broadcast::Receiver<SyncMessage>,
    is_initiator: bool,
) {
    info!(
        "IPC connection with site_id={}, initiator={}",
        remote_site_id, is_initiator
    );

    let (read_half, mut write_half) = stream.into_split();

    // If we're the initiator, send our state as an announce
    if is_initiator {
        let state_bytes = site.read().await.commit.clone().save();
        if let Err(e) = write_message(&mut write_half, &SyncMessage::Announce(state_bytes)).await {
            error!("IPC: Failed to send announce: {e}");
            return;
        }
    }

    // Run shared read/write/alive loop
    run_connection_loop(read_half, write_half, remote_site_id, inbound_tx, outbound_rx).await;

    info!("IPC connection closed with site_id={}", remote_site_id);
}

/// Main IPC setup: binds a Unix socket, scans for peers, accepts connections.
pub async fn setup_ipc(
    site_id: u32,
    storage_dir: PathBuf,
    site: Site,
    inbound_tx: TokioSender<ProtoMessage<SyncMessage>>,
    mut outbound_rx: TokioReceiver<SyncMessage>,
) -> Result<()> {
    let ipc_dir = storage_dir.join("ipc");
    tokio::fs::create_dir_all(&ipc_dir).await?;

    let our_socket_path = ipc_dir.join(format!("{}.sock", site_id));

    // Clean up our own stale socket if it exists
    if our_socket_path.exists() {
        tokio::fs::remove_file(&our_socket_path).await.ok();
    }

    let listener = UnixListener::bind(&our_socket_path)?;

    info!("IPC listening on {}", our_socket_path.display());

    // Broadcast channel for fan-out to all IPC connections
    let (broadcast_tx, _) = tokio::sync::broadcast::channel::<SyncMessage>(64);

    // Fan-out: outbound_rx -> broadcast to all IPC connections
    let fanout_broadcast_tx = broadcast_tx.clone();
    tokio::spawn(async move {
        while let Some(msg) = outbound_rx.recv().await {
            fanout_broadcast_tx.send(msg).ok();
        }
    });

    // Scan existing sockets and connect to peers
    let scan_broadcast_tx = broadcast_tx.clone();
    let scan_inbound_tx = inbound_tx.clone();
    let scan_site = site.clone();
    let scan_ipc_dir = ipc_dir.clone();

    tokio::spawn(async move {
        if let Err(e) = scan_and_connect(
            site_id,
            &scan_ipc_dir,
            scan_site,
            scan_inbound_tx,
            scan_broadcast_tx,
        )
        .await
        {
            warn!("IPC scan error: {e}");
        }
    });

    // Accept loop
    loop {
        let (stream, _) = listener.accept().await?;

        // For accepted connections, we don't know the remote site_id from the address.
        // We need to figure it out. We'll peek at the first message or use a handshake.
        // Since the initiator sends Announce first, we can read the first message
        // and derive site_id from the connection. But the plan says to use socket filename.
        // For accepted connections, the initiator knows our socket name. We don't know theirs.
        // Solution: have the initiator send their site_id as a 4-byte prefix before the protocol.

        let inbound_tx = inbound_tx.clone();
        let outbound_rx = broadcast_tx.subscribe();
        let site = site.clone();

        tokio::spawn(async move {
            // Read the remote site_id (4-byte handshake)
            let remote_site_id = match read_handshake_site_id(&stream).await {
                Ok(id) => id,
                Err(e) => {
                    warn!("IPC: Failed to read handshake site_id: {e}");
                    return;
                }
            };

            if remote_site_id == site_id {
                debug!("IPC: Ignoring connection from ourselves");
                return;
            }

            handle_ipc_connection(
                stream,
                site_id,
                remote_site_id,
                site,
                inbound_tx,
                outbound_rx,
                false,
            )
            .await;
        });
    }
}

/// Read a 4-byte site_id handshake from the stream (without consuming stream ownership).
async fn read_handshake_site_id(stream: &UnixStream) -> Result<u32> {
    stream.readable().await?;

    let mut buf = [0u8; 4];
    let mut total = 0;

    // We need to read exactly 4 bytes; use a loop with try_read
    while total < 4 {
        stream.readable().await?;
        match stream.try_read(&mut buf[total..]) {
            Ok(0) => anyhow::bail!("Connection closed during handshake"),
            Ok(n) => total += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => return Err(e.into()),
        }
    }

    Ok(u32::from_be_bytes(buf))
}

/// Write a 4-byte site_id handshake to the stream (without consuming stream ownership).
async fn write_handshake_site_id(stream: &UnixStream, site_id: u32) -> Result<()> {
    let bytes = site_id.to_be_bytes();
    let mut total = 0;

    while total < 4 {
        stream.writable().await?;
        match stream.try_write(&bytes[total..]) {
            Ok(n) => total += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => return Err(e.into()),
        }
    }

    Ok(())
}

/// Scan existing `.sock` files and connect to peer instances.
async fn scan_and_connect(
    site_id: u32,
    ipc_dir: &std::path::Path,
    site: Site,
    inbound_tx: TokioSender<ProtoMessage<SyncMessage>>,
    broadcast_tx: tokio::sync::broadcast::Sender<SyncMessage>,
) -> Result<()> {
    let mut entries = tokio::fs::read_dir(ipc_dir).await?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("sock") {
            continue;
        }

        let remote_site_id = match parse_site_id_from_path(&path) {
            Some(id) => id,
            None => continue,
        };

        // Skip our own socket
        if remote_site_id == site_id {
            continue;
        }

        // Try to connect with a short timeout
        match tokio::time::timeout(Duration::from_secs(2), UnixStream::connect(&path)).await {
            Ok(Ok(stream)) => {
                info!("IPC: Connected to peer site_id={}", remote_site_id);

                // Send our site_id as handshake
                if let Err(e) = write_handshake_site_id(&stream, site_id).await {
                    warn!("IPC: Failed handshake with site_id={}: {e}", remote_site_id);
                    continue;
                }

                let inbound_tx = inbound_tx.clone();
                let outbound_rx = broadcast_tx.subscribe();
                let site = site.clone();

                tokio::spawn(async move {
                    handle_ipc_connection(
                        stream,
                        site_id,
                        remote_site_id,
                        site,
                        inbound_tx,
                        outbound_rx,
                        true,
                    )
                    .await;
                });
            }
            Ok(Err(e)) => {
                debug!("IPC: Stale socket for site_id={}, removing: {e}", remote_site_id);
                tokio::fs::remove_file(&path).await.ok();
            }
            Err(_) => {
                debug!("IPC: Timeout connecting to site_id={}, removing stale socket", remote_site_id);
                tokio::fs::remove_file(&path).await.ok();
            }
        }
    }

    Ok(())
}
