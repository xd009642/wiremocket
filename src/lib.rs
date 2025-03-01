//! API slightly based off wiremock in that you start a server
use crate::responder::{pending, ResponseStream, ResponseStreamBuilder};
use crate::utils::*;
use axum::{
    extract::{
        ws::{CloseFrame as AxumCloseFrame, Message as AxumMessage, WebSocket, WebSocketUpgrade},
        Path, Query,
    },
    http::header::HeaderMap,
    response::Response,
    routing::any,
    Extension, Router,
};
use futures::{sink::SinkExt, stream::StreamExt};
use std::collections::HashMap;
use std::future::IntoFuture;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Instant;
use tokio::sync::{mpsc, oneshot, RwLock};
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

#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash)]
enum MatchStatus {
    /// One or more matchers return Some(false)
    Mismatch,
    /// All matchers return None
    Potential,
    /// Some matchers return Some(true) some None
    Partial,
    /// Every matcher returns Some(rue)
    Full,
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
/// is already more complicated to handle the case where there's no path/header matching and only
/// body matching.
///
/// ## The Plan
///
/// The plan is to move matching from a simple "yes/no/undetermined" to a 4 state system:
///
/// 1. Mismatch: One or more matchers is false
/// 2. Potential: No matchers match but none have rejected the request
/// 3. Partial: Some matchers match the request
/// 4. Full: All matchers match the request - this is unambiguous
///
/// If we're in state 3 or 4 at the request point we'll pick the combination of most complete
/// status and highest priority. Otherwise, the list of potential matchers is used for the request
/// checking and each message we check and if we find a partial or higher we'll select the one with
/// the highest priority.
///
/// Once a mock has been selected as the active mock, we'll then start passing the messages into
/// the responder and the mock server will start sending messages back (if it's not a silent
/// responder).
///
/// ## Drawbacks
///
/// I'm not currently keeping track of which matchers are triggered and which aren't. This is maybe
/// a mistake and potentially I should be making sure that at some point during the request they've
/// all matched. I'm also not tracking which levels matchesr match at:
/// request/message/all-of-the-above. These two things may have to change in order to be actually
/// useful but I'm kicking the can on that decision down the road a bit.
#[derive(Clone)]
pub struct Mock {
    matcher: Vec<Arc<dyn Match + Send + Sync + 'static>>,
    responder: Arc<dyn ResponseStreamBuilder + Send + Sync + 'static>,
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
            responder: Arc::new(pending),
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

    fn check_request(
        &self,
        path: &str,
        headers: &HeaderMap,
        params: &HashMap<String, String>,
    ) -> MatchStatus {
        let values = self
            .matcher
            .iter()
            .map(|x| x.request_match(path, headers, params))
            .collect::<Vec<Option<bool>>>();

        self.check_matcher_responses(&values)
    }

    fn check_message(&self, message: &Message) -> MatchStatus {
        let values = self
            .matcher
            .iter()
            .map(|x| x.unary_match(&message))
            .collect::<Vec<Option<bool>>>();

        self.check_matcher_responses(&values)
    }

    fn check_matcher_responses(&self, values: &[Option<bool>]) -> MatchStatus {
        if values.iter().copied().all(can_consider) {
            let contains_true = values.contains(&Some(true));
            let contains_none = values.contains(&None);

            if contains_true {
                if !contains_none {
                    MatchStatus::Full
                } else {
                    MatchStatus::Partial
                }
            } else {
                MatchStatus::Potential
            }
        } else {
            MatchStatus::Mismatch
        }
    }

    fn register_hit(&self) {
        self.calls.fetch_add(1, Ordering::Acquire);
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
    let mut matched_mock = None;
    {
        debug!("checking request level matches");
        let mocks = mocks.read().await.clone();
        for (index, mock) in mocks.iter().enumerate() {
            let mock_status = mock.check_request(&path, &headers, &params);

            if mock_status != MatchStatus::Mismatch {
                active_mocks.push(index);
            }

            if matches!(mock_status, MatchStatus::Full | MatchStatus::Partial) {
                if let Some((best_index, status, priority)) = &mut matched_mock {
                    if mock_status > *status
                        || (mock_status == *status && mock.priority < *priority)
                    {
                        *best_index = index;
                        *status = mock_status;
                        *priority = mock.priority
                    }
                } else {
                    matched_mock = Some((index, mock_status, mock.priority));
                }
            }
        }
    }

    if let Some((index, status, priority)) = matched_mock {
        active_mocks = vec![index];
        let mocks = mocks.read().await.clone();
        mocks[index].register_hit();
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

fn unconvert_message(msg: Message) -> AxumMessage {
    match msg {
        Message::Text(t) => AxumMessage::Text(t.as_str().into()),
        Message::Binary(b) => AxumMessage::Binary(b.into()),
        Message::Ping(p) => AxumMessage::Ping(p.into()),
        Message::Pong(p) => AxumMessage::Pong(p.into()),
        Message::Close(cf) => AxumMessage::Close(cf.map(|cf| AxumCloseFrame {
            code: cf.code.into(),
            reason: cf.reason.as_str().into(),
        })),
        Message::Frame(_) => unreachable!(),
    }
}

async fn handle_socket(mut socket: WebSocket, mocks: MockList, mut active_mocks: Vec<usize>) {
    // Clone the mocks present when the connection comes in
    let mocks: Vec<Mock> = mocks.read().await.clone();
    let mut active_mocks = active_mocks
        .iter()
        .filter_map(|m| mocks.get(*m))
        .collect::<Vec<&Mock>>();

    let (mut sender, mut receiver) = socket.split();
    let (mut msg_tx, msg_rx) = mpsc::channel(8);

    let mut sender_task = if active_mocks.len() == 1 {
        let responder = active_mocks[0].responder.create_response_stream();
        let stream = responder.handle(msg_rx);
        let handle = tokio::task::spawn(async move {
            stream
                .map(|x| Ok(unconvert_message(x)))
                .forward(sender)
                .await;
        });
        Some(handle)
    } else {
        None
    };

    while let Some(msg) = receiver.next().await {
        if let Ok(msg) = msg {
            let msg = convert_message(msg);
            debug!("Checking: {:?}", msg);
            if active_mocks.len() == 1 {
                if matches!(
                    active_mocks[0].check_message(&msg),
                    MatchStatus::Full | MatchStatus::Partial
                ) {
                    active_mocks[0].register_hit();
                }
            } else {
                let mut matched_mock = None;
                let mut priorities = vec![];
                for (index, mock) in active_mocks.iter().enumerate() {
                    priorities.push(mock.priority);
                    let mock_status = mock.check_message(&msg);
                    if matches!(mock_status, MatchStatus::Full | MatchStatus::Partial) {
                        if let Some((best_index, status, priority)) = &mut matched_mock {
                            if mock_status > *status
                                || (mock_status == *status && mock.priority < *priority)
                            {
                                *best_index = index;
                                *status = mock_status;
                                *priority = mock.priority
                            }
                        } else {
                            matched_mock = Some((index, mock_status, mock.priority));
                        }
                    }
                }
                match matched_mock {
                    Some((index, _, _)) => {
                        let active_mock = active_mocks.remove(index);
                        active_mocks = vec![active_mock];
                        mocks[index].register_hit();
                    }
                    None => {
                        let top_priority = priorities
                            .iter()
                            .enumerate()
                            .min_by_key(|(index, priority)| *priority)
                            .map(|(index, _)| index);
                        match top_priority {
                            Some(index) => {
                                let active_mock = active_mocks.remove(index);
                                active_mocks = vec![active_mock];
                                mocks[index].register_hit();
                            }
                            None => {
                                continue;
                            }
                        }
                    }
                }
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
