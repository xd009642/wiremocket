use crate::MockServer;
use deadpool::managed::{Manager, Metrics, Pool, RecycleResult};
use std::convert::Infallible;
use std::sync::LazyLock;

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


#[derive(Debug)]
pub(crate) struct MockServerPoolManager;

impl Manager for MockServerPoolManager {
    type Error = Infallible;
    type Type = MockServer;

    async fn create(&self) -> Result<MockServer, Infallible> {
        Ok(MockServer::start().await)
    }

    async fn recycle(&self, obj: &mut Self::Type, metrics: &Metrics) -> RecycleResult<Self::Error> {
        obj.reset().await;
        Ok(()) 
    }
}
