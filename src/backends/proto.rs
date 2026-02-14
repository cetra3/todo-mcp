use std::{
    cmp,
    collections::HashMap,
    marker::PhantomData,
    net::{Ipv4Addr, SocketAddr},
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use futures::{ready, Stream};
use pin_project_lite::pin_project;
use serde::{de::DeserializeOwned, Serialize};
use socket2::{Domain, Socket, Type};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf},
    net::UdpSocket,
    sync::mpsc::Sender as TokioSender,
};
use tracing::*;

use super::multicast::SyncMessage;

const BUF_SIZE: usize = 1400;

pin_project! {
    pub struct McastReceiver<D: DeserializeOwned> {
        site_partials: HashMap<u32, SitePartials>,
        #[pin]
        socket: UdpSocket,
        buffer: [u8; BUF_SIZE + 16],
        _var: PhantomData<D>,
    }
}

pub struct ProtoMessage<D: DeserializeOwned> {
    pub site_id: u32,
    pub message: D,
}

impl<D: DeserializeOwned> McastReceiver<D> {
    pub fn new() -> anyhow::Result<Self> {
        // This is our multicast listener
        // This does actually include *all* messages including from the originating site
        let recv_socket = Socket::new(Domain::IPV4, Type::DGRAM, None)?;
        recv_socket.join_multicast_v4(&Ipv4Addr::new(239, 1, 1, 1), &Ipv4Addr::new(0, 0, 0, 0))?;
        recv_socket.set_reuse_address(true)?;
        recv_socket.set_reuse_port(true)?;
        recv_socket.set_nonblocking(true)?;
        recv_socket.bind(&"0.0.0.0:1111".parse::<SocketAddr>()?.into())?;

        let socket = UdpSocket::from_std(recv_socket.into())?;

        Ok(McastReceiver {
            site_partials: HashMap::new(),
            socket,
            buffer: [0; BUF_SIZE + 16],
            _var: PhantomData,
        })
    }
}

impl<D: DeserializeOwned> Stream for McastReceiver<D> {
    type Item = anyhow::Result<ProtoMessage<D>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();

        let mut read_buf = ReadBuf::new(&mut *this.buffer);

        ready!(this.socket.poll_recv(cx, &mut read_buf))?;

        let filled_buffer = read_buf.filled();

        let amt = filled_buffer.len();

        if amt < 16 {
            warn!("Amount on the wire is below the partial header size!");
            return Poll::Pending;
        }
        let incoming_site_id = u32::from_be_bytes(filled_buffer[0..4].try_into()?);

        let site_partials = this.site_partials.entry(incoming_site_id).or_default();

        site_partials.fill_from_buffer(filled_buffer)?;

        if let Some(bytes) = site_partials.get_buffer() {
            let (message, _) =
                bincode::serde::decode_from_slice::<D, _>(&bytes, bincode::config::standard())?;
            let message = ProtoMessage {
                site_id: incoming_site_id,
                message,
            };

            Poll::Ready(Some(Ok(message)))
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

struct SitePartials {
    seq: u32,
    num: u32,
    partials: Vec<(u32, Vec<u8>)>,
}

impl SitePartials {
    fn fill_from_buffer(&mut self, buf: &[u8]) -> anyhow::Result<()> {
        // The seq represents the current message number from the other end
        // I.e, if this message is fragmented over multiple packets (which is usually the case, 1500 bytes lol),
        let seq = u32::from_be_bytes(buf[4..8].try_into()?);
        // This is the total number of partials for this given seq
        let num = u32::from_be_bytes(buf[8..12].try_into()?);
        // This is the current partial index
        let idx = u32::from_be_bytes(buf[12..16].try_into()?);

        trace!("Seq:{}, Num:{}, Idx:{}", seq, num, idx);

        if seq != self.seq {
            trace!("Resetting partials for site");
            self.partials = vec![];
            self.seq = seq;
            self.num = num;
        }

        // This is the actual partial data
        let partial = buf[16..].to_vec();

        trace!("Partial len:{}", partial.len());

        self.partials.push((idx, partial));

        Ok(())
    }

    fn get_buffer(&mut self) -> Option<Vec<u8>> {
        // We can't get the buffer yet if there is not enough data
        if self.partials.len() != self.num as usize {
            return None;
        }

        let mut partials = std::mem::take(&mut self.partials);

        partials.sort_by(|(left, _), (right, _)| left.cmp(right));

        Some(
            partials
                .into_iter()
                .map(|(_idx, buf)| buf)
                .flatten()
                .collect(),
        )
    }
}

impl Default for SitePartials {
    fn default() -> Self {
        Self {
            seq: 0,
            num: 0,
            partials: vec![],
        }
    }
}

pub struct McastSender {
    site_id: u32,
    seq: u32,
    socket: UdpSocket,
    send_addr: SocketAddr,
}

impl McastSender {
    pub fn new(site_id: u32) -> anyhow::Result<Self> {
        let send_socket = Socket::new(Domain::IPV4, Type::DGRAM, None)?;
        send_socket.set_nonblocking(true)?;

        let send_addr = "239.1.1.1:1111".parse::<SocketAddr>()?;
        let socket = UdpSocket::from_std(send_socket.into())?;

        Ok(Self {
            site_id,
            seq: 1,
            socket,
            send_addr,
        })
    }

    pub async fn send<D: Serialize>(&mut self, message: D) -> anyhow::Result<()> {
        let mut val: Vec<u8> =
            bincode::serde::encode_to_vec(&message, bincode::config::standard())?;

        let mut to_send = val.len();

        trace!("Total len to send:{}", to_send);

        let num = (to_send / BUF_SIZE) as u32 + 1;

        trace!("Total number to send:{}", num);

        let mut idx: u32 = 0;

        while to_send > 0 {
            let end = cmp::min(BUF_SIZE, val.len());
            let new_val = val.split_off(end);

            let mut body = val;

            trace!("Body len:{}", body.len());

            let mut payload = Vec::new();
            trace!(
                "Sending Site:{}, Seq:{}, Num:{} Idx:{}",
                self.site_id,
                self.seq,
                num,
                idx
            );

            payload.append(&mut self.site_id.to_be_bytes().to_vec());
            payload.append(&mut self.seq.to_be_bytes().to_vec());
            payload.append(&mut num.to_be_bytes().to_vec());
            payload.append(&mut idx.to_be_bytes().to_vec());
            payload.append(&mut body);

            val = new_val;

            self.socket.send_to(&payload, &self.send_addr).await?;

            to_send = to_send - end;

            idx += 1;
        }

        self.seq += 1;

        Ok(())
    }
}

// --- Shared framing and connection loop for stream-based backends (iroh, IPC) ---

/// Write a length-prefixed bincode message to any AsyncWrite stream.
pub async fn write_message<W: AsyncWrite + Unpin>(
    writer: &mut W,
    msg: &SyncMessage,
) -> anyhow::Result<()> {
    let encoded = bincode::serde::encode_to_vec(msg, bincode::config::standard())?;
    let len = (encoded.len() as u32).to_be_bytes();
    writer.write_all(&len).await?;
    writer.write_all(&encoded).await?;
    Ok(())
}

/// Read a length-prefixed bincode message from any AsyncRead stream.
/// Returns `Ok(None)` on graceful stream close.
pub async fn read_message<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> anyhow::Result<Option<SyncMessage>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Ok(None);
        }
        Err(e) => {
            return Err(e.into());
        }
    }

    let len = u32::from_be_bytes(len_buf) as usize;

    if len > 64 * 1024 * 1024 {
        anyhow::bail!("Message too large: {len} bytes");
    }

    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;

    let (msg, _) =
        bincode::serde::decode_from_slice::<SyncMessage, _>(&buf, bincode::config::standard())?;

    Ok(Some(msg))
}

/// Run a read/write/alive connection loop over any AsyncRead + AsyncWrite pair.
///
/// Spawns three tasks: read loop, write loop, and alive heartbeat.
/// Blocks until any task completes, then sends a Shutdown message.
pub async fn run_connection_loop<R, W>(
    mut reader: R,
    mut writer: W,
    remote_site_id: u32,
    inbound_tx: TokioSender<ProtoMessage<SyncMessage>>,
    mut outbound_rx: tokio::sync::broadcast::Receiver<SyncMessage>,
) where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    // Read loop
    let read_inbound_tx = inbound_tx.clone();
    let read_handle = tokio::spawn(async move {
        loop {
            match read_message(&mut reader).await {
                Ok(Some(msg)) => {
                    let proto_msg = ProtoMessage {
                        site_id: remote_site_id,
                        message: msg,
                    };
                    if read_inbound_tx.send(proto_msg).await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    debug!("Read error from site_id={}: {e}", remote_site_id);
                    break;
                }
            }
        }
    });

    // Write loop (from broadcast)
    let write_handle = tokio::spawn(async move {
        loop {
            match outbound_rx.recv().await {
                Ok(msg) => {
                    if let Err(e) = write_message(&mut writer, &msg).await {
                        debug!("Write error: {e}");
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("Broadcast lagged by {n} messages");
                }
            }
        }
    });

    // Alive heartbeat
    let alive_inbound_tx = inbound_tx.clone();
    let alive_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            interval.tick().await;
            if alive_inbound_tx
                .send(ProtoMessage {
                    site_id: remote_site_id,
                    message: SyncMessage::Alive,
                })
                .await
                .is_err()
            {
                break;
            }
        }
    });

    tokio::select! {
        _ = read_handle => {}
        _ = write_handle => {}
        _ = alive_handle => {}
    }

    // Send Shutdown on disconnect
    inbound_tx
        .send(ProtoMessage {
            site_id: remote_site_id,
            message: SyncMessage::Shutdown,
        })
        .await
        .ok();
}
