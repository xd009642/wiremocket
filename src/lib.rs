//! API slightly based off wiremock in that you start a server
use crate::responder::{pending, ResponseStream};
use crate::utils::*;
use axum::{
    extract::{
        ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade},
        Path, Query,
    },
    http::header::HeaderMap,
    response::Response,
    routing::any,
    Extension, Router,
};
use std::collections::HashMap;
use std::future::IntoFuture;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Instant;
use tokio::sync::{oneshot, RwLock};
use tracing::{debug, Instrument};
use tungstenite::{
    protocol::{frame::Utf8Bytes, CloseFrame},
    Message,
};

pub mod matchers;
pub mod responder;
pub mod utils;

pub mod prelude {
    pub use super::*;
    pub use crate::matchers::*;
}

type MockList = Arc<RwLock<Vec<Mock>>>;

/// Server here we'd apply our mock servers and ability to verify requests. Based off of
/// https://docs.rs/wiremock/latest/wiremock/struct.MockServer.html
pub struct MockServer {
    addr: String,
    shutdown: Option<oneshot::Sender<()>>,
    mocks: MockList,
}

/// Specify things like the routes this responds to i.e. `/api/ws-stream` query parameters, and
/// behaviour it should exhibit in terms of source/sink messages. Also, will have matchers to allow
/// you to do things like "make sure all messages are valid json"
///
/// Mock precedence.
///
/// Given a mock can generate a sequence of responses we have to adequately handle instances where
/// multiple mocks match. Despite a temptation to allow multiple mocks to be active and merge
/// multiple response streams this is complicated and will make it harder to specify tests that
/// act deterministically where failures can be debugged.
///
/// With that in mind how do we select which mock is active when we have matchers that act on the
/// initial request parameters and ones which act on the received messages? Our matching parameter
/// is already more complicated being a `Option<bool>` to handle the case where there's no
/// path/header matching and only body matching.
///
/// ## The Plan
#[derive(Clone)]
pub struct Mock {
    matcher: Vec<Arc<dyn Match + Send + Sync + 'static>>,
    responder: Arc<dyn ResponseStream + Send + Sync + 'static>,
    expected_calls: Arc<Times>,
    calls: Arc<AtomicU64>,
    name: Option<String>,
    priority: u8,
}

impl Mock {
    pub fn given(matcher: impl Match + Send + Sync + 'static) -> Self {
        Self {
            matcher: vec![Arc::new(matcher)],
            expected_calls: Default::default(),
            responder: Arc::new(pending()),
            calls: Default::default(),
            name: None,
            priority: 5,
        }
    }

    pub fn named<T: Into<String>>(mut self, mock_name: T) -> Self {
        self.name = Some(mock_name.into());
        self
    }

    pub fn expect(mut self, times: impl Into<Times>) -> Self {
        self.expected_calls = Arc::new(times.into());
        self
    }

    pub fn add_matcher(mut self, matcher: impl Match + Send + Sync + 'static) -> Self {
        self.matcher.push(Arc::new(matcher));
        self
    }

    /// 1 is highest default is 5 (like wiremock)
    pub fn with_priority(mut self, priority: u8) -> Self {
        assert!(priority > 0, "priority must be strictly greater than 0!");
        self.priority = priority;
        self
    }

    /// You can use this to verify the mock separately to the one you put into the server (if
    /// you've cloned it).
    pub fn verify(&self) -> bool {
        self.expected_calls
            .contains(self.calls.load(Ordering::SeqCst))
    }

    pub fn check_request(
        &self,
        path: &str,
        headers: &HeaderMap,
        params: &HashMap<String, String>,
    ) -> Option<bool> {
        let values = self
            .matcher
            .iter()
            .map(|x| x.request_match(path, headers, params))
            .collect::<Vec<Option<bool>>>();

        if values.iter().copied().all(can_consider) {
            // TODO should this really be !values.contains(&None) ?
            if values.contains(&Some(true)) {
                self.calls.fetch_add(1, Ordering::Acquire);
            }
            if values.contains(&None) {
                None
            } else {
                Some(true)
            }
        } else {
            Some(false)
        }
    }

    pub fn check_message(&self, message: &Message) -> Option<bool> {
        let values = self
            .matcher
            .iter()
            .map(|x| x.unary_match(&message))
            .collect::<Vec<Option<bool>>>();

        if values.iter().copied().all(can_consider) {
            if values.contains(&Some(true)) {
                self.calls.fetch_add(1, Ordering::Acquire);
                Some(true)
            } else {
                None
            }
        } else {
            Some(false)
        }
    }
}

pub struct RecordedConnection {
    incoming: Vec<(Instant, Message)>,
    outgoing: Vec<(Instant, Message)>,
}

async fn ws_handler_pathless(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    params: Query<HashMap<String, String>>,
    mocks: Extension<MockList>,
) -> Response {
    ws_handler(ws, Path(String::new()), headers, params, mocks).await
}

#[inline(always)]
fn can_consider(match_res: Option<bool>) -> bool {
    matches!(match_res, Some(true) | None)
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Path(path): Path<String>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    mocks: Extension<MockList>,
) -> Response {
    let mut active_mocks = vec![];
    let mut full_matches = vec![];
    {
        debug!("checking request level matches");
        let mocks = mocks.read().await.clone();
        for (index, mock) in mocks.iter().enumerate() {
            let mock_status = mock.check_request(&path, &headers, &params);
            if mock_status != Some(false) {
                active_mocks.push(index);
                if mock_status == Some(true) {
                    full_matches.push(index);
                }
            }
        }
    }
    debug!("about to upgrade websocket connection");
    ws.on_upgrade(|socket| async move { handle_socket(socket, mocks.0, active_mocks).await })
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

async fn handle_socket(mut socket: WebSocket, mocks: MockList, active_mocks: Vec<usize>) {
    // Clone the mocks present when the connection comes in
    let mocks: Vec<Mock> = mocks.read().await.clone();
    let active_mocks = active_mocks
        .iter()
        .filter_map(|m| mocks.get(*m))
        .collect::<Vec<&Mock>>();
    while let Some(msg) = socket.recv().await {
        if let Ok(msg) = msg {
            let msg = convert_message(msg);
            debug!("Checking: {:?}", msg);
            for mock in &active_mocks {
                mock.check_message(&msg);
            }
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

        let router = Router::new()
            .route("/{*path}", any(ws_handler))
            .route("/", any(ws_handler_pathless))
            .layer(Extension(mocks.clone()));
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let addr = format!("ws://{}", listener.local_addr().unwrap());

        debug!("axum listening on: {}", addr);

        let (tx, rx) = oneshot::channel();
        let listening_server = axum::serve(listener, router).with_graceful_shutdown(async move {
            let _ = rx.await;
        });

        tokio::task::spawn(listening_server.into_future().in_current_span());

        Self {
            addr,
            shutdown: Some(tx),
            mocks,
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
        for (index, mock) in self.mocks.read().await.iter().enumerate() {
            println!("Checking {:?} {:?}", mock.expected_calls, mock.calls);
            match &mock.name {
                None => debug!("Checking mock[{}]", index),
                Some(name) => debug!("Checking mock: {}", name),
            }
            assert!(mock.verify())
        }
    }

    pub async fn mocks_pass(&self) -> bool {
        self.mocks.read().await.iter().all(|x| x.verify())
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        let tx = self.shutdown.take().unwrap();
        let _ = tx.send(());
    }
}

pub trait Match {
    fn request_match(
        &self,
        path: &str,
        headers: &HeaderMap,
        query: &HashMap<String, String>,
    ) -> Option<bool> {
        None
    }

    fn unary_match(&self, message: &Message) -> Option<bool> {
        None
    }
}

impl<F> Match for F
where
    F: Fn(&Message) -> bool,
    F: Send + Sync,
{
    fn unary_match(&self, msg: &Message) -> Option<bool> {
        Some(self(msg))
    }
}
