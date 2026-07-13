use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpListener;
use tokio::net::TcpStream;

/// The recv and send halves of a split host-guest channel.
pub type ChannelHalves = (Box<dyn HostGuestRecvHalf>, Box<dyn HostGuestSendHalf>);

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("connection refused: {0}")]
    ConnectionRefused(String),
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(String),
}

/// Abstract trait for host-guest communication channels.
///
/// Implementations:
/// - `TcpChannel` — TCP loopback (for testing without hypervisor)
/// - `VsockChannel` — AF_VSOCK on Linux (Phase 3)
/// - `HyperVChannel` — AF_HYPERV on Windows (Phase 3)
pub trait HostGuestChannel: Send {
    fn split(self: Box<Self>) -> Result<ChannelHalves, ChannelError>;
}

pub trait HostGuestRecvHalf: Send {
    fn poll_recv(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, ChannelError>>;
}

pub trait HostGuestSendHalf: Send {
    fn poll_send(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize, ChannelError>>;
}

/// Async wrapper that turns a `HostGuestRecvHalf` into a `Future`.
pub struct RecvFuture<'a> {
    half: &'a mut dyn HostGuestRecvHalf,
    buf: &'a mut [u8],
}

impl<'a> RecvFuture<'a> {
    pub fn new(half: &'a mut dyn HostGuestRecvHalf, buf: &'a mut [u8]) -> Self {
        Self { half, buf }
    }
}

impl Future for RecvFuture<'_> {
    type Output = Result<usize, ChannelError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.half.poll_recv(cx, this.buf)
    }
}

/// Async wrapper that turns a `HostGuestSendHalf` into a `Future`.
pub struct SendFuture<'a> {
    half: &'a mut dyn HostGuestSendHalf,
    buf: &'a [u8],
}

impl<'a> SendFuture<'a> {
    pub fn new(half: &'a mut dyn HostGuestSendHalf, buf: &'a [u8]) -> Self {
        Self { half, buf }
    }
}

impl Future for SendFuture<'_> {
    type Output = Result<usize, ChannelError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.half.poll_send(cx, this.buf)
    }
}

/// A TCP-based channel for testing without a hypervisor.
pub struct TcpChannel {
    stream: TcpStream,
}

impl TcpChannel {
    pub async fn connect(addr: &str) -> Result<Self, ChannelError> {
        let stream = TcpStream::connect(addr).await?;
        Ok(Self { stream })
    }

    pub async fn listen(addr: &str) -> Result<TcpChannelListener, ChannelError> {
        let listener = TcpListener::bind(addr).await?;
        Ok(TcpChannelListener { listener })
    }
}

impl HostGuestChannel for TcpChannel {
    fn split(
        self: Box<Self>,
    ) -> Result<(Box<dyn HostGuestRecvHalf>, Box<dyn HostGuestSendHalf>), ChannelError> {
        let (r, w) = self.stream.into_split();
        Ok((Box::new(TcpRecvHalf(r)), Box::new(TcpSendHalf(w))))
    }
}

pub struct TcpRecvHalf(OwnedReadHalf);

impl HostGuestRecvHalf for TcpRecvHalf {
    fn poll_recv(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, ChannelError>> {
        let mut rb = ReadBuf::new(buf);
        match Pin::new(&mut self.0).poll_read(cx, &mut rb) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(rb.filled().len())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(ChannelError::Io(e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub struct TcpSendHalf(OwnedWriteHalf);

impl HostGuestSendHalf for TcpSendHalf {
    fn poll_send(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize, ChannelError>> {
        Pin::new(&mut self.0)
            .poll_write(cx, buf)
            .map_err(ChannelError::Io)
    }
}

pub struct TcpChannelListener {
    listener: TcpListener,
}

impl TcpChannelListener {
    pub async fn accept(&mut self) -> Result<Box<dyn HostGuestChannel>, ChannelError> {
        let (stream, _) = self.listener.accept().await?;
        Ok(Box::new(TcpChannel { stream }))
    }
}

/// AF_VSOCK channel for Linux. Returns UnsupportedPlatform until Phase 3
/// provides the actual file descriptor from the hypervisor backend.
pub struct VsockChannel;

impl HostGuestChannel for VsockChannel {
    fn split(
        self: Box<Self>,
    ) -> Result<(Box<dyn HostGuestRecvHalf>, Box<dyn HostGuestSendHalf>), ChannelError> {
        Err(ChannelError::UnsupportedPlatform(
            "AF_VSOCK requires Phase 3 hypervisor backend to provide the vsock file descriptor"
                .into(),
        ))
    }
}

/// AF_HYPERV channel for Windows. Returns UnsupportedPlatform until Phase 3
/// provides the actual hvsock handle from the HCS/WHP backend.
pub struct HyperVChannel;

impl HostGuestChannel for HyperVChannel {
    fn split(
        self: Box<Self>,
    ) -> Result<(Box<dyn HostGuestRecvHalf>, Box<dyn HostGuestSendHalf>), ChannelError> {
        Err(ChannelError::UnsupportedPlatform(
            "AF_HYPERV requires Phase 3 HCS/WHP backend to provide the hvsock handle".into(),
        ))
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use tokio::runtime::Runtime;

    #[test]
    fn test_vsock_channel_returns_unsupported() {
        let channel: Box<dyn HostGuestChannel> = Box::new(VsockChannel);
        match channel.split() {
            Err(ChannelError::UnsupportedPlatform(msg)) => {
                assert!(msg.contains("AF_VSOCK"));
            }
            Err(other) => panic!("expected UnsupportedPlatform, got: {}", other),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[test]
    fn test_hyperv_channel_returns_unsupported() {
        let channel: Box<dyn HostGuestChannel> = Box::new(HyperVChannel);
        match channel.split() {
            Err(ChannelError::UnsupportedPlatform(msg)) => {
                assert!(msg.contains("AF_HYPERV"));
            }
            Err(other) => panic!("expected UnsupportedPlatform, got: {}", other),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[test]
    fn test_tcp_loopback_send_recv() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let addr = "127.0.0.1:0";
            let mut listener = TcpChannel::listen(addr).await.unwrap();
            let bind_addr = listener.listener.local_addr().unwrap().to_string();

            let server = tokio::spawn(async move {
                let channel = Box::new(listener.accept().await.unwrap());
                let (mut rx, mut tx) = channel.split().unwrap();
                let mut buf = vec![0u8; 1024];
                let n = RecvFuture::new(rx.as_mut(), &mut buf).await.unwrap();
                assert_eq!(&buf[..n], b"hello");
                SendFuture::new(tx.as_mut(), b"world").await.unwrap();
            });

            let channel = Box::new(TcpChannel::connect(&bind_addr).await.unwrap());
            let (mut rx, mut tx) = channel.split().unwrap();
            SendFuture::new(tx.as_mut(), b"hello").await.unwrap();
            let mut buf = vec![0u8; 1024];
            let n = RecvFuture::new(rx.as_mut(), &mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"world");

            server.await.unwrap();
        });
    }
}
