//! API slightly based off wiremock in that you start a server
use axum::{
    extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade},
    response::Response,
    routing::any,
    Extension, Router,
};
use std::future::IntoFuture;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{oneshot, RwLock};
use tungstenite::{
    protocol::{frame::Utf8Bytes, CloseFrame},
    Message,
};

type MockList = Arc<RwLock<Vec<Mock>>>;

/// Server here we'd apply our mock servers and ability to verify requests. Based off of
/// https://docs.rs/wiremock/latest/wiremock/struct.MockServer.html
pub struct MockServer {
    addr: String,
    shutdown: Option<oneshot::Sender<()>>,
    mocks: MockList,
    verify_status: Arc<RwLock<bool>>,
}

/// Specify things like the routes this responds to i.e. `/api/ws-stream` query parameters, and
/// behaviour it should exhibit in terms of source/sink messages. Also, will have matchers to allow
/// you to do things like "make sure all messages are valid json"
#[derive(Clone)]
pub struct Mock {
    matcher: Vec<Arc<dyn Match + Send + Sync + 'static>>,
}

impl Mock {
    pub fn given(matcher: impl Match + Send + Sync + 'static) -> Self {
        Self {
            matcher: vec![Arc::new(matcher)],
        }
    }

    pub fn add_matcher(&mut self, matcher: impl Match + Send + Sync + 'static) {
        self.matcher.push(Arc::new(matcher));
    }
}

pub struct RecordedConnection {
    incoming: Vec<(Instant, Message)>,
    outgoing: Vec<(Instant, Message)>,
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    mocks: Extension<MockList>,
    match_status: Extension<Arc<RwLock<bool>>>,
) -> Response {
    ws.on_upgrade(|socket| async move { handle_socket(socket, mocks.0, match_status.0).await })
}

fn convert_message(msg: AxumMessage) -> Message {
    match msg {
        AxumMessage::Text(t) => Message::Text(Utf8Bytes::from(t.as_str())),
        AxumMessage::Binary(b) => Message::Binary(b),
        AxumMessage::Ping(p) => Message::Ping(p),
        AxumMessage::Pong(p) => Message::Pong(p),
        AxumMessage::Close(cf) => Message::Close(cf.map(|cf| CloseFrame {
            code: cf.code.into(),
            reason: Utf8Bytes::from(cf.reason.as_str()),
        })),
    }
}

async fn handle_socket(mut socket: WebSocket, mocks: MockList, match_status: Arc<RwLock<bool>>) {
    // Clone the mocks present when the connection comes in
    let mocks: Vec<Mock> = mocks.read().await.clone();
    println!("{} mocks loaded", mocks.len());
    while let Some(msg) = socket.recv().await {
        if let Ok(msg) = msg {
            let msg = convert_message(msg);
            println!("Checking: {:?}", msg);
            // TODO need to figure out relationship between mocks and matchers!
            for mock in &mocks {
                for matcher in &mock.matcher {
                    if !matcher.unary_match(&msg) {
                        println!("{:?} was not json", msg);
                        *match_status.write().await = false;
                    }
                }
            }
        } else {
            println!("Invalid message");
            *match_status.write().await = false;
        }
    }
}

impl MockServer {
    /// Start a new instance of a MockServer listening on a random port.
    ///
    /// Each instance of MockServer is fully isolated: start takes care of
    /// finding a random port available on your local machine which is assigned
    /// to the new MockServer.
    ///
    /// You should use one instance of MockServer for each REST API that your
    /// application interacts with and needs mocking for testing purposes.
    pub async fn start() -> Self {
        let mocks: MockList = Default::default();
        let verify_status: Arc<RwLock<bool>> = Arc::new(RwLock::new(true));

        let router = Router::new()
            .route("/", any(ws_handler))
            .layer(Extension(mocks.clone()))
            .layer(Extension(verify_status.clone()));
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let addr = format!("ws://{}", listener.local_addr().unwrap());

        let (tx, rx) = oneshot::channel();
        let listening_server = axum::serve(listener, router).with_graceful_shutdown(async move {
            let _ = rx.await;
        });

        tokio::task::spawn(listening_server.into_future());

        Self {
            addr,
            shutdown: Some(tx),
            mocks,
            verify_status,
        }
    }

    /// Register a mock on an instance of the mock server.
    pub async fn register(&self, mock: Mock) {
        self.mocks.write().await.push(mock);
    }

    /// Return the base uri of this running instance of MockServer, e.g.
    /// ws://127.0.0.1:4372.
    ///
    /// Use this method to compose uris when interacting with this instance of
    /// MockServer via a websocket client.
    pub fn uri(&self) -> String {
        self.addr.clone()
    }

    /// Return a vector with all the recorded connections to the server. In each recorded
    /// connection you can see the incoming and outgoing messages and when they happened
    pub fn sessions(&self) -> Vec<RecordedConnection> {
        todo!("Record some connection info");
    }

    pub async fn verify(&self) {
        assert!(*self.verify_status.read().await)
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        let tx = self.shutdown.take().unwrap();
        let _ = tx.send(());
    }
}

pub trait Match {
    fn unary_match(&self, msg: &Message) -> bool {
        true
    }
}

#[cfg(feature = "serde_json")]
pub use json::*;

#[cfg(feature = "serde_json")]
pub mod json {
    use super::*;
    use serde_json::Value;

    pub struct ValidJsonMatcher;

    impl Match for ValidJsonMatcher {
        fn unary_match(&self, msg: &Message) -> bool {
            match msg {
                Message::Text(t) => serde_json::from_str::<Value>(&t).is_ok(),
                Message::Binary(b) => serde_json::from_slice::<Value>(b.as_ref()).is_ok(),
                _ => true, // We can't be judging pings/pongs/closes
            }
        }
    }
}
