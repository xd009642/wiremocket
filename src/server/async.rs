//! [server::async](/src/server/async.rs)
//!
//! This module contains the procedures for spinning up a lightweight mock websocket server
//! using tokio and tunngstenite.
#[allow(unused_import_braces, unused_imports)]
#[rustfmt::skip]
use {
    crate::errors::external::WebmocketErr,
    futures_util::{SinkExt, StreamExt},
    pin_project_lite::pin_project,
    std::borrow::Borrow,
    std::sync::atomic::{AtomicU16, Ordering::{Acquire, Relaxed}},
    std::pin::Pin,
    tokio::net::{TcpListener, TcpStream, ToSocketAddrs},
    tokio::task::JoinHandle,
    tokio_tungstenite::accept_hdr_async,
    tokio_tungstenite::tungstenite::{
        connect,
        handshake::server::{Request, Response},
        Message,
    },
};

// A non-owning handle to a mock websocket server managed by tokio and tunngstenite.
pin_project! {
    pub struct Webmocket {
        port: &'static AtomicU16,
        #[pin]
        pub handle: Pin<Box<JoinHandle<()>>>,
    }
}

static PORT: AtomicU16 = AtomicU16::new(0);
static HOST: &str = "127.0.0.1";

impl Webmocket {
    pub async fn new() -> Result<Self, WebmocketErr<()>> {
        let port = PORT.load(Relaxed);
        let addr = format!("{}:{}", HOST, port);
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|_err| WebmocketErr(Box::new(())))?;
        {
            PORT.store(
                listener
                    .borrow()
                    .local_addr()
                    .expect("loopback device")
                    .port(),
                Relaxed,
            );
        }
        let handle = Box::pin(tokio::spawn(async move {
            while let Ok((_stream, _)) = listener.accept().await {
                todo!()
            }
        }));
        Ok(Self::async_builder()
            .await
            .port(&PORT)
            .handle(handle)
            .build())
    }

    async fn async_builder() -> WebmocketBuilder {
        WebmocketBuilder::default()
    }

    pub fn port(&self) -> u16 {
        self.port.load(Acquire)
    }
}

#[derive(Default)]
struct WebmocketBuilder {
    port: Option<&'static AtomicU16>,
    handle: Option<Pin<Box<JoinHandle<()>>>>,
}

impl WebmocketBuilder {
    fn port(self, new_port: &'static AtomicU16) -> Self {
        let Self { handle, .. } = self;
        Self {
            port: Some(new_port),
            handle,
        }
    }
    fn handle(self, new_handle: Pin<Box<JoinHandle<()>>>) -> Self {
        let Self { port, .. } = self;
        Self {
            port,
            handle: Some(new_handle),
        }
    }
    fn build(self) -> Webmocket {
        assert!(
            &self.port.is_some(),
            "Library user requires initialized port"
        );
        assert!(
            &self.handle.is_some(),
            "An existing tokio task was not spawned yet"
        );
        let WebmocketBuilder { port, handle } = self;
        let port = port.unwrap();
        let handle = handle.unwrap();
        Webmocket { port, handle }
    }
}
