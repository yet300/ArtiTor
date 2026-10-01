//! Physical listener ownership independent of callback-producing task stacks.
use std::io;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::task::Poll;
use tokio::net::{TcpListener, TcpStream};

pub(super) struct ListeningSocket(Mutex<Option<TcpListener>>);

impl ListeningSocket {
    pub(super) fn new(listener: TcpListener) -> Self {
        Self(Mutex::new(Some(listener)))
    }

    /// The only physical listener is dropped before this returns. The accept
    /// future never retains a borrow across Pending, so neither a task join nor
    /// runtime progress is needed (including single-worker callback re-entry).
    /// This leaf lock never acquires engine/registry locks or invokes callbacks.
    pub(super) fn close(&self) {
        self.0.lock().unwrap().take();
    }

    pub(super) async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        std::future::poll_fn(|cx| {
            let socket = self.0.lock().unwrap();
            match socket.as_ref() {
                Some(listener) => listener.poll_accept(cx),
                None => Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "listener closed",
                ))),
            }
        })
        .await
    }
}
