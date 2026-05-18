//! TCP connection cap for the axum HTTP servers.
//!
//! `axum::serve(listener, app)` accepts connections with no upper bound. A
//! slow-loris attack or a connection storm accumulates file descriptors until
//! the process hits the OS limit, at which point `accept()` fails and the
//! server stops serving everyone — not just the attacker.
//!
//! [`LimitedListener`] wraps any [`axum::serve::Listener`] and gates `accept()`
//! on a [`Semaphore`]. A permit is acquired *before* the connection is pulled
//! off the kernel backlog and is held for the entire lifetime of the
//! connection (it lives inside the returned IO handle and is released on
//! drop). Once the cap is reached, excess connections wait in the listen
//! backlog and are eventually refused by the OS, which is the desired
//! backpressure rather than unbounded fd growth.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::serve::Listener;
use tokio::io::{self, AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Default maximum number of concurrent connections per listener when
/// `MAX_CONNECTIONS` is unset or unparseable.
pub const DEFAULT_MAX_CONNECTIONS: usize = 2048;

/// Read `MAX_CONNECTIONS` from the environment, falling back to
/// [`DEFAULT_MAX_CONNECTIONS`]. A value of `0` is treated as invalid and
/// falls back to the default — a literal zero would wedge the server by
/// refusing every connection.
pub fn max_connections_from_env() -> usize {
    parse_max_connections(std::env::var("MAX_CONNECTIONS").ok().as_deref())
}

fn parse_max_connections(raw: Option<&str>) -> usize {
    raw.and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MAX_CONNECTIONS)
}

/// A [`Listener`] that caps the number of live connections at `max`.
pub struct LimitedListener<L> {
    inner: L,
    semaphore: Arc<Semaphore>,
}

impl<L> LimitedListener<L> {
    /// Wrap `inner`, allowing at most `max` concurrent connections.
    pub fn new(inner: L, max: usize) -> Self {
        Self {
            inner,
            semaphore: Arc::new(Semaphore::new(max)),
        }
    }
}

impl<L: Listener> Listener for LimitedListener<L> {
    type Io = LimitedIo<L::Io>;
    type Addr = L::Addr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        // Acquire a permit *before* taking the connection off the backlog so we
        // never own an fd we are not allowed to serve. `Semaphore` is never
        // closed here, so `acquire_owned` cannot fail.
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("connection-limit semaphore is never closed");
        let (io, addr) = self.inner.accept().await;
        (
            LimitedIo {
                inner: io,
                _permit: permit,
            },
            addr,
        )
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

/// Connection IO that releases its semaphore permit when dropped.
pub struct LimitedIo<Io> {
    inner: Io,
    _permit: OwnedSemaphorePermit,
}

impl<Io: AsyncRead + Unpin> AsyncRead for LimitedIo<Io> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl<Io: AsyncWrite + Unpin> AsyncWrite for LimitedIo<Io> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn env_parsing() {
        assert_eq!(parse_max_connections(None), DEFAULT_MAX_CONNECTIONS);
        assert_eq!(parse_max_connections(Some("0")), DEFAULT_MAX_CONNECTIONS);
        assert_eq!(
            parse_max_connections(Some("not-a-number")),
            DEFAULT_MAX_CONNECTIONS
        );
        assert_eq!(parse_max_connections(Some("512")), 512);
    }

    #[tokio::test]
    async fn permit_is_released_on_drop() {
        let sem = Arc::new(Semaphore::new(1));
        let permit = sem.clone().acquire_owned().await.unwrap();
        let io = LimitedIo {
            inner: tokio::io::empty(),
            _permit: permit,
        };
        assert_eq!(sem.available_permits(), 0);
        drop(io);
        assert_eq!(sem.available_permits(), 1);
    }

    #[tokio::test]
    async fn accept_blocks_at_limit_and_resumes_on_drop() {
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tcp.local_addr().unwrap();
        let mut limited = LimitedListener::new(tcp, 1);

        // Connect a client and drain the single permit.
        let _client1 = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (io1, _) = limited.accept().await;
        assert_eq!(limited.semaphore.available_permits(), 0);

        // A second accept must block while the permit is held.
        let _client2 = tokio::net::TcpStream::connect(addr).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), limited.accept())
                .await
                .is_err(),
            "accept should block when the connection cap is reached"
        );

        // Releasing the first connection frees the permit; accept should now complete.
        drop(io1);
        tokio::time::timeout(Duration::from_millis(100), limited.accept())
            .await
            .expect("accept should succeed after a connection is released");
    }
}
