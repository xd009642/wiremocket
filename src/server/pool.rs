use crate::server::bare::BareMockServer;
use deadpool::managed::{Manager, Metrics, Object, Pool, RecycleResult};
use std::convert::Infallible;
use std::sync::LazyLock;
use tracing::debug;

static MOCK_SERVER_POOL: LazyLock<Pool<MockServerPoolManager>> = LazyLock::new(|| {
    // We are choosing an arbitrarily high max_size because we never want a test to "wait" for
    // a `BareMockServer` instance to become available.
    //
    // We might expose in the future a way for a crate user to tune this value.
    Pool::builder(MockServerPoolManager)
        .max_size(1000)
        .build()
        .expect("Building a server pool is not expected to fail. Please report an issue")
});

pub(crate) type PooledMockServer = Object<MockServerPoolManager>;

pub(super) async fn get_pooled_mock_server() -> PooledMockServer {
    MOCK_SERVER_POOL
        .get()
        .await
        .expect("Failed to get a MockServer from the pool")
}

#[derive(Debug)]
pub(crate) struct MockServerPoolManager;

impl Manager for MockServerPoolManager {
    type Error = Infallible;
    type Type = BareMockServer;

    async fn create(&self) -> Result<Self::Type, Self::Error> {
        debug!("Creating a server");
        Ok(BareMockServer::start().await)
    }

    async fn recycle(&self, obj: &mut Self::Type, metrics: &Metrics) -> RecycleResult<Self::Error> {
        debug!("Recyling a server");
        obj.reset().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::*;
    use tokio_tungstenite::connect_async;

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn pools_connections() {
        let pooled = get_pooled_mock_server().await;

        let server = MockServer::new(InnerServer::Pooled(pooled));

        println!("connecting to: {}", server.uri());
        let og_url = server.uri();

        let (stream, _response) = connect_async(server.uri()).await.unwrap();

        std::mem::drop(stream);

        server.mocks_pass().await;

        std::mem::drop(server);

        let pooled = get_pooled_mock_server().await;

        let server = MockServer::new(InnerServer::Pooled(pooled));

        println!("connecting to: {}", server.uri());
        assert_eq!(og_url, server.uri());

        let (stream, _response) = connect_async(server.uri()).await.unwrap();

        std::mem::drop(stream);
        std::mem::drop(server);
    }
}
