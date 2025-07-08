use crate::{
    tcp::tcp_server::{TcpMessage, TcpSender},
    tunnel_message::{TunnelMessage, UdpLocalAddr},
    udp::{udp_packet::UdpPacket, udp_server::UdpMessage},
    BUFFER_POOL, UDP_PACKET_SIZE, GAMING_BUFFER_POOL, GAMING_UDP_PACKET_SIZE,
};

use super::udp_server::UdpServer;
use anyhow::{bail, Context, Result};
use dashmap::DashMap;
use log::{debug, error, info, warn};
use quinn::{Connection, RecvStream, SendStream};
use rs_utilities::log_and_bail;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::net::UdpSocket;

type TSafe<T> = Arc<tokio::sync::Mutex<T>>;

pub struct UdpTunnel;

impl UdpTunnel {
    pub async fn start(
        conn: &quinn::Connection,
        mut udp_server: UdpServer,
        tcp_sender: Option<TcpSender>,
        udp_timeout_ms: u64,
        gaming_mode: bool,
    ) -> Result<()> {
        let stream_map = Arc::new(DashMap::new());
        udp_server.set_active(true);
        let mut udp_receiver = udp_server.take_receiver().unwrap();

        debug!("start transfering udp packets from: {}", udp_server.addr());
        while let Some(UdpMessage::Packet(packet)) = udp_receiver.recv().await {
            let quic_send = match UdpTunnel::open_stream(
                conn.clone(),
                udp_server.clone(),
                packet.addr,
                stream_map.clone(),
                udp_timeout_ms,
                gaming_mode,
            )
            .await
            {
                Ok(quic_send) => quic_send,
                Err(e) => {
                    error!("{e}");
                    if conn.close_reason().is_some() {
                        if let Some(tcp_sender) = tcp_sender {
                            tcp_sender.send(TcpMessage::Quit).await.ok();
                        }
                        debug!("connection is closed, will quit");
                        break;
                    }
                    continue;
                }
            };

            // send the packet using an async task
            tokio::spawn(async move {
                let mut quic_send = quic_send.lock().await;
                let payload_len = packet.payload.len();
                TunnelMessage::send_raw(&mut quic_send, &packet.payload)
                    .await
                    .inspect_err(|e| {
                        warn!(
                            "failed to send datagram({payload_len}) through the tunnel, err: {e}"
                        );
                    })
                    .ok();
            });
        }

        // put the receiver back
        udp_server.set_active(false);
        udp_server.put_receiver(udp_receiver);
        info!("local udp server paused");
        Ok(())
    }

    async fn open_stream(
        conn: Connection,
        udp_server: UdpServer,
        peer_addr: SocketAddr,
        stream_map: Arc<DashMap<SocketAddr, TSafe<SendStream>>>,
        udp_timeout_ms: u64,
        gaming_mode: bool,
    ) -> Result<TSafe<SendStream>> {
        if let Some(s) = stream_map.get(&peer_addr) {
            return Ok((*s).clone());
        }

        let (mut quic_send, mut quic_recv) =
            conn.open_bi().await.context("open_bi failed for udp out")?;

        TunnelMessage::send(
            &mut quic_send,
            &TunnelMessage::ReqUdpStart(UdpLocalAddr(peer_addr)),
        )
        .await?;

        debug!(
            "new udp session: {peer_addr}, streams: {}",
            stream_map.len()
        );

        let quic_send = Arc::new(tokio::sync::Mutex::new(quic_send));
        stream_map.insert(peer_addr, quic_send.clone());
        let udp_sender = udp_server.clone_udp_sender();

        let stream_map = stream_map.clone();
        let packet_size = if gaming_mode { GAMING_UDP_PACKET_SIZE } else { UDP_PACKET_SIZE };
        let buffer_pool = if gaming_mode { &GAMING_BUFFER_POOL } else { &BUFFER_POOL };
        
        tokio::spawn(async move {
            debug!(
                "start udp stream: {peer_addr}, streams: {}",
                stream_map.len()
            );
            loop {
                let mut buf = buffer_pool.alloc_and_fill(packet_size);
                match tokio::time::timeout(
                    Duration::from_millis(udp_timeout_ms),
                    TunnelMessage::recv_raw(&mut quic_recv, &mut buf),
                )
                .await
                {
                    Ok(Ok(len)) => {
                        unsafe {
                            buf.set_len(len as usize);
                        }
                        let packet = UdpPacket {
                            payload: buf,
                            addr: peer_addr,
                        };
                        udp_sender.send(UdpMessage::Packet(packet)).await.ok();
                    }
                    e => {
                        match e {
                            Ok(Err(e)) => {
                                warn!("failed to read for udp, err: {e}");
                            }
                            Err(_) => {
                                // timedout
                                // debug!("timeout on reading udp packet");
                            }
                            _ => unreachable!(""),
                        }
                        break;
                    }
                }
            }

            stream_map.remove(&peer_addr);
            debug!(
                "drop udp session: {peer_addr}, streams: {}",
                stream_map.len()
            );
        });

        Ok(quic_send)
    }

    pub async fn process(conn: &quinn::Connection, upstream_addr: SocketAddr, udp_timeout_ms: u64, gaming_mode: bool) {
        let remote_addr = &conn.remote_address();
        info!("start udp streaming, {remote_addr} ↔  {upstream_addr}");

        loop {
            match conn.accept_bi().await {
                Err(quinn::ConnectionError::TimedOut { .. }) => {
                    info!("connection timeout: {remote_addr}");
                    break;
                }
                Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
                    debug!("connection closed: {remote_addr}");
                    break;
                }
                Err(e) => {
                    error!("failed to accpet_bi: {remote_addr}, err: {e}");
                    break;
                }
                Ok((quic_send, quic_recv)) => tokio::spawn(async move {
                    Self::process_internal(quic_send, quic_recv, upstream_addr, udp_timeout_ms, gaming_mode)
                        .await
                }),
            };
        }

        info!("connection for udp out is dropped");
    }

    async fn process_internal(
        mut quic_send: SendStream,
        mut quic_recv: RecvStream,
        upstream_addr: SocketAddr,
        udp_timeout_ms: u64,
        gaming_mode: bool,
    ) -> Result<()> {
        let peer_addr = match TunnelMessage::recv(&mut quic_recv).await {
            Ok(TunnelMessage::ReqUdpStart(peer_addr)) => peer_addr.0,
            _ => {
                log_and_bail!("unexpected first udp message");
            }
        };

        let udp_socket = UdpSocket::bind("0.0.0.0:0").await?;
        udp_socket.connect(upstream_addr).await?;

        let packet_size = if gaming_mode { GAMING_UDP_PACKET_SIZE } else { UDP_PACKET_SIZE };
        let buffer_pool = if gaming_mode { &GAMING_BUFFER_POOL } else { &BUFFER_POOL };

        // spawn a task to forward packets from upstream to the tunnel
        let mut quic_send_clone = quic_send.try_clone().await?;
        let udp_socket_clone = udp_socket.try_clone().await?;
        tokio::spawn(async move {
            let mut buf = buffer_pool.alloc_and_fill(packet_size);
            loop {
                match tokio::time::timeout(
                    Duration::from_millis(udp_timeout_ms),
                    udp_socket_clone.recv(&mut buf),
                )
                .await
                {
                    Ok(Ok(len)) => {
                        unsafe {
                            buf.set_len(len);
                        }
                        TunnelMessage::send_raw(&mut quic_send_clone, &buf)
                            .await
                            .inspect_err(|e| {
                                warn!(
                                    "timeout on receiving datagrams from upstream: {upstream_addr}"
                                );
                            })
                            .ok();
                    }
                    Ok(Err(e)) => {
                        warn!("failed to receive from upstream: {upstream_addr}, err: {e}");
                        break;
                    }
                    Err(_) => {
                        // timeout
                        break;
                    }
                }
            }
        });

        // forward packets from the tunnel to upstream
        let mut buf = buffer_pool.alloc_and_fill(packet_size);
        loop {
            match tokio::time::timeout(
                Duration::from_millis(udp_timeout_ms),
                TunnelMessage::recv_raw(&mut quic_recv, &mut buf),
            )
            .await
            {
                Ok(Ok(len)) => {
                    unsafe {
                        buf.set_len(len as usize);
                    }
                    udp_socket.send(&buf).await.inspect_err(|e| {
                        warn!("failed to send to upstream: {upstream_addr}, err: {e}");
                    })?;
                }
                Ok(Err(e)) => {
                    warn!("failed to read udp packet from tunnel, err: {e}");
                    break;
                }
                Err(_) => {
                    debug!("timeout on reading udp packet from tunnel");
                    break;
                }
            }
        }

        Ok(())
    }
}
