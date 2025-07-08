use super::tcp_server::TcpMessage;
use crate::{TcpServer, BUFFER_POOL, GAMING_BUFFER_POOL, GAMING_TCP_BUFFER_SIZE};
use anyhow::Result;
use log::{debug, error, info};
use quinn::{RecvStream, SendStream};
use std::borrow::BorrowMut;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::TcpStream;
use tokio::time::error::Elapsed;

#[derive(Debug, PartialEq, Eq)]
pub enum TransferError {
    InternalError,
    TimeoutError,
}

pub struct TcpTunnel;

impl TcpTunnel {
    pub async fn start(
        tunnel_out: bool,
        conn: &quinn::Connection,
        tcp_server: &mut TcpServer,
        pending_stream: &mut Option<TcpStream>,
        tcp_timeout_ms: u64,
        gaming_mode: bool,
    ) {
        tcp_server.set_active(true);
        let mut tcp_receiver = tcp_server.take_tcp_receiver().unwrap();
        loop {
            let tcp_stream = match pending_stream.take() {
                Some(tcp_stream) => tcp_stream,
                None => match tcp_receiver.borrow_mut().recv().await {
                    Some(TcpMessage::Request(tcp_stream)) => tcp_stream,
                    _ => break,
                },
            };

            match conn.open_bi().await {
                Ok(quic_stream) => {
                    TcpTunnel::run(tunnel_out, tcp_stream, quic_stream, tcp_timeout_ms, gaming_mode)
                }
                Err(e) => {
                    error!("failed to open_bi, will retry: {e}");
                    *pending_stream = Some(tcp_stream);
                    break;
                }
            }
        }
        tcp_server.put_tcp_receiver(tcp_receiver);
        // the tcp server will be reused when tunnel reconnects
        tcp_server.set_active(false);
    }

    fn run(
        tunnel_out: bool,
        tcp_stream: TcpStream,
        quic_stream: (SendStream, RecvStream),
        tcp_timeout_ms: u64,
        gaming_mode: bool,
    ) {
        let (mut tcp_read, mut tcp_write) = tcp_stream.into_split();
        let (mut quic_send, mut quic_recv) = quic_stream;

        let tag = if tunnel_out { "OUT" } else { "IN" };
        let index = quic_send.id().index();
        let in_addr = match tcp_read.peer_addr() {
            Ok(in_addr) => in_addr,
            Err(e) => {
                log::error!("failed to get peer_addr: {e:?}");
                return;
            }
        };

        debug!("[{tag}] START {index:<3} →  {in_addr:<20}");

        let loop_count = Arc::new(AtomicI32::new(0));
        let loop_count_clone = loop_count.clone();
        
        // Use gaming-optimized buffer size if gaming mode is enabled
        let buffer_size = if gaming_mode { GAMING_TCP_BUFFER_SIZE } else { 8192 };
        let buffer_pool = if gaming_mode { &GAMING_BUFFER_POOL } else { &BUFFER_POOL };

        tokio::spawn(async move {
            let mut transfer_bytes = 0u64;
            let mut buffer = buffer_pool.alloc_and_fill(buffer_size);
            loop {
                let c_start = loop_count.load(Ordering::Relaxed);
                let result = Self::quic_to_tcp(
                    &mut quic_recv,
                    &mut tcp_write,
                    &mut buffer,
                    &mut transfer_bytes,
                    tcp_timeout_ms,
                )
                .await;
                let c_end = loop_count.fetch_add(1, Ordering::Relaxed);

                match result {
                    Err(TransferError::TimeoutError) => {
                        if c_start == c_end {
                            log::warn!("quic to tcp timeout");
                            break;
                        }
                    }
                    Ok(0) | Err(_) => {
                        break;
                    }
                    _ => {
                        // ok
                    }
                }
            }

            debug!("[{tag}] END  {index:<3} →  {in_addr}, {transfer_bytes} bytes");
        });

        tokio::spawn(async move {
            let mut transfer_bytes = 0u64;
            let mut buffer = buffer_pool.alloc_and_fill(buffer_size);
            loop {
                let c_start = loop_count_clone.load(Ordering::Relaxed);
                let result = Self::tcp_to_quic(
                    &mut tcp_read,
                    &mut quic_send,
                    &mut buffer,
                    &mut transfer_bytes,
                    tcp_timeout_ms,
                )
                .await;
                let c_end = loop_count_clone.fetch_add(1, Ordering::Relaxed);

                match result {
                    Err(TransferError::TimeoutError) => {
                        if c_start == c_end {
                            log::warn!("tcp to quic timeout");
                            break;
                        }
                    }
                    Ok(0) | Err(_) => {
                        break;
                    }
                    _ => {
                        // ok
                    }
                }
            }

            debug!("[{tag}] END  {index:<3} ←  {in_addr}, {transfer_bytes} bytes");
        });
    }

    async fn tcp_to_quic(
        tcp_read: &mut OwnedReadHalf,
        quic_send: &mut SendStream,
        buffer: &mut [u8],
        transfer_bytes: &mut u64,
        tcp_timeout_ms: u64,
    ) -> Result<usize, TransferError> {
        let len_read =
            tokio::time::timeout(Duration::from_millis(tcp_timeout_ms), tcp_read.read(buffer))
                .await
                .map_err(|_: Elapsed| TransferError::TimeoutError)?
                .map_err(|_| TransferError::InternalError)?;
        if len_read > 0 {
            *transfer_bytes += len_read as u64;
            quic_send
                .write_all(&buffer[..len_read])
                .await
                .map_err(|_| TransferError::InternalError)?;
            Ok(len_read)
        } else {
            quic_send
                .finish()
                .map_err(|_| TransferError::InternalError)?;
            Ok(0)
        }
    }

    async fn quic_to_tcp(
        quic_recv: &mut RecvStream,
        tcp_write: &mut OwnedWriteHalf,
        buffer: &mut [u8],
        transfer_bytes: &mut u64,
        tcp_timeout_ms: u64,
    ) -> Result<usize, TransferError> {
        let result = tokio::time::timeout(
            Duration::from_millis(tcp_timeout_ms),
            quic_recv.read(buffer),
        )
        .await
        .map_err(|_: Elapsed| TransferError::TimeoutError)?
        .map_err(|_| TransferError::InternalError)?;
        if let Some(len_read) = result {
            *transfer_bytes += len_read as u64;
            tcp_write
                .write_all(&buffer[..len_read])
                .await
                .map_err(|_| TransferError::InternalError)?;
            Ok(len_read)
        } else {
            tcp_write
                .shutdown()
                .await
                .map_err(|_| TransferError::InternalError)?;
            Ok(0)
        }
    }

    pub async fn process(conn: &quinn::Connection, upstream_addr: SocketAddr, tcp_timeout_ms: u64, gaming_mode: bool) {
        let remote_addr = &conn.remote_address();
        info!("start tcp streaming, {remote_addr} ↔  {upstream_addr}");

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
                Ok((quic_send, quic_recv)) => {
                    match tokio::time::timeout(
                        Duration::from_millis(tcp_timeout_ms),
                        TcpStream::connect(upstream_addr),
                    )
                    .await
                    {
                        Ok(Ok(tcp_stream)) => {
                            Self::run(true, tcp_stream, quic_stream, tcp_timeout_ms, gaming_mode)
                        }
                        Ok(Err(e)) => error!("failed to connect to {upstream_addr}, err: {e}"),
                        Err(_) => error!("timeout connecting to {upstream_addr}"),
                    }
                }
            }
        }

        info!("connection for tcp out is dropped");
    }
}
