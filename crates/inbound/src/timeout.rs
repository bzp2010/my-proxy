use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::{Instant, Sleep};

#[derive(Debug, Clone, Copy)]
enum Phase {
    WaitingForRequest,
    ReadingHeaders { deadline: Instant },
    Processing,
}

struct Shared {
    phase: Mutex<Phase>,
    idle_timeout: Duration,
    header_timeout: Duration,
}

/// A handle used by the HTTP layer to tell a connection's timeout wrapper
/// when a request has started being processed and when it is done.
#[derive(Clone)]
pub struct Handle {
    shared: Arc<Shared>,
}

impl Handle {
    /// Call once headers for a request have been parsed: disables all
    /// read timeouts until the response for that request is sent.
    pub fn begin_processing(&self) {
        *self.shared.phase.lock().unwrap() = Phase::Processing;
    }

    /// Call once a request has been fully handled: goes back to waiting
    /// for the next request under the inactivity timeout.
    pub fn end_request(&self) {
        *self.shared.phase.lock().unwrap() = Phase::WaitingForRequest;
    }
}

/// Wraps a stream with two read timeouts: an inactivity timeout while
/// waiting for a new request, and a fixed deadline (from the first byte)
/// to finish reading that request's headers. Writes are never timed out.
pub struct ConnectionTimeout<S> {
    inner: S,
    shared: Arc<Shared>,
    sleep: Pin<Box<Sleep>>,
    // The idle deadline already committed to for the current
    // "waiting for request" episode, if any.
    waiting_deadline: Option<Instant>,
}

impl<S> ConnectionTimeout<S> {
    pub fn new(inner: S, idle: Duration, header_read: Duration) -> (Self, Handle) {
        let shared = Arc::new(Shared {
            phase: Mutex::new(Phase::WaitingForRequest),
            idle_timeout: idle,
            header_timeout: header_read,
        });
        let wrapper = Self {
            inner,
            shared: shared.clone(),
            sleep: Box::pin(tokio::time::sleep(Duration::ZERO)),
            waiting_deadline: None,
        };
        (wrapper, Handle { shared })
    }
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "connection timed out")
}

impl<S: AsyncRead + Unpin> AsyncRead for ConnectionTimeout<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let phase = *this.shared.phase.lock().unwrap();

        let deadline = match phase {
            Phase::Processing => {
                this.waiting_deadline = None;
                return Pin::new(&mut this.inner).poll_read(cx, buf);
            }
            Phase::ReadingHeaders { deadline } => deadline,
            Phase::WaitingForRequest => *this
                .waiting_deadline
                .get_or_insert_with(|| Instant::now() + this.shared.idle_timeout),
        };

        if Instant::now() >= deadline {
            return Poll::Ready(Err(timed_out()));
        }

        this.sleep.as_mut().reset(deadline);

        let filled_before = buf.filled().len();
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                this.waiting_deadline = None;
                if matches!(phase, Phase::WaitingForRequest) && buf.filled().len() > filled_before
                {
                    let deadline = Instant::now() + this.shared.header_timeout;
                    *this.shared.phase.lock().unwrap() = Phase::ReadingHeaders { deadline };
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(err)) => {
                this.waiting_deadline = None;
                Poll::Ready(Err(err))
            }
            Poll::Pending => match this.sleep.as_mut().poll(cx) {
                Poll::Ready(()) => {
                    this.waiting_deadline = None;
                    Poll::Ready(Err(timed_out()))
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for ConnectionTimeout<S> {
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
}
