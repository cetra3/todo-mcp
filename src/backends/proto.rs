use std::{
    cmp,
    collections::HashMap,
    marker::PhantomData,
    net::{Ipv4Addr, SocketAddr},
    pin::Pin,
    task::{Context, Poll},
};

use futures::{ready, Stream};
use pin_project_lite::pin_project;
use serde::{de::DeserializeOwned, Serialize};
use socket2::{Domain, Socket, Type};
use tokio::{io::ReadBuf, net::UdpSocket};
use tracing::*;

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
