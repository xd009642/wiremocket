use crate::match_state::*;
use crate::mock::*;
use crate::server::bare::*;
use crate::server::pool::{get_pooled_mock_server, PooledMockServer};
use crate::utils::*;
use std::ops::Deref;

pub mod bare;
pub mod pool;

#[derive(Debug)]
pub struct MockServer(InnerServer);

#[derive(Debug)]
pub enum InnerServer {
    Bare(BareMockServer),
    Pooled(PooledMockServer),
}

impl Deref for InnerServer {
    type Target = BareMockServer;

    fn deref(&self) -> &Self::Target {
        match self {
            InnerServer::Bare(b) => b,
            InnerServer::Pooled(p) => p,
        }
    }
}

impl MockServer {
    pub(super) fn new(inner: InnerServer) -> Self {
        Self(inner)
    }

    pub async fn start() -> Self {
        Self(InnerServer::Pooled(get_pooled_mock_server().await))
    }

    pub async fn reset(&self) {
        self.0.reset().await;
    }

    /// Register a mock on an instance of the mock server.
    pub async fn register(&self, mock: Mock) {
        self.0.register(mock).await
    }

    /// Return the base uri of this running instance of BareMockServer, e.g.
    /// ws://127.0.0.1:4372.
    ///
    /// Use this method to compose uris when interacting with this instance of
    /// BareMockServer via a websocket client.
    pub fn uri(&self) -> String {
        self.0.uri()
    }

    /// Asserts on [`BareMockServer::mocks_pass`]
    pub async fn verify(&self) {
        self.0.verify().await
    }

    /// Returns true if all mocks pass. This method will wait for all on-going websocket
    /// connections to close before responding as match status is updated at the end of a request.
    /// If a connection is open then this method can deadlock.
    pub async fn mocks_pass(&self) -> bool {
        self.0.mocks_pass().await
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        //futures::executor::block_on(self.verify())
    }
}
