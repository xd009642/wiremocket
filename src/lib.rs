#![doc = include_str!("../README.md")]
use crate::match_state::*;
use crate::responder::{pending, MapResponder, ResponseStream, StreamResponse};
pub use crate::utils::*;
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
use futures::{
    sink::SinkExt,
    stream::{Stream, StreamExt},
};
use std::collections::HashMap;
use std::future::IntoFuture;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Instant;
use tokio::sync::{broadcast, oneshot, watch, Mutex, RwLock};
use tracing::{debug, error, Instrument};
use tungstenite::{
    protocol::{frame::Utf8Bytes, CloseFrame},
    Message,
};

pub mod match_state;
pub mod matchers;
pub mod responder;
pub mod utils;

/// Re-exports every part of the public API for ease of use.
pub mod prelude {
    pub use super::*;
    pub use crate::match_state::*;
    pub use crate::matchers::*;
    pub use crate::responder::*;
}

type MockList = Arc<RwLock<Vec<Mock>>>;

/// Server here we'd apply our mock servers and ability to verify requests. Based off of
/// https://docs.rs/wiremock/latest/wiremock/struct.MockServer.html
pub struct MockServer {
    addr: String,
    shutdown: Option<oneshot::Sender<()>>,
    mocks: MockList,
    active_requests: Mutex<watch::Receiver<usize>>,
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
/// behaviour it should exhibit in terms of source/sink messages. Also has matchers to allow
/// you to do things like "make sure all messages are valid json"
///
/// # Mock precedence.
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
/// The way this is accomplished is to move matching from a simple "yes/no/undetermined" to a 4 state
/// system:
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
/// For the `Mock` to pass then all of the matchers added will have to have evaluated as
/// `Some(true)` at least once. If any return `Some(false)` after the `Mock` has been selected then
/// the call won't be registered but no other failure will occur.
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
    /// Start building a [`Mock`] specifying the first matcher.
    ///
    /// TODO this should return a builder actually.
    pub fn given(matcher: impl Match + Send + Sync + 'static) -> Self {
        Self {
            matcher: vec![Arc::new(matcher)],
            responder: Arc::new(pending()),
            name: None,
            priority: 5,
            expected_calls: Default::default(),
            calls: Default::default(),
        }
    }

    /// Assign a name to your mock.  
    ///
    /// The mock name will be used in error messages (e.g. if the mock expectation
    /// is not satisfied) and debug logs to help you identify what failed.
    pub fn named<T: Into<String>>(mut self, mock_name: T) -> Self {
        self.name = Some(mock_name.into());
        self
    }

    /// Set an expectation on the number of times this [`Mock`] should match in the current
    /// test case.
    ///
    /// Unlike wiremock no expectations are checked when the server is shutting down. Although this
    /// may change in the future.
    ///
    /// By default, no expectation is set for [`Mock`]s.
    ///
    /// ### When is this useful?
    ///
    /// `expect` can turn out handy when you'd like to verify that a certain side-effect has
    /// (or has not!) taken place.
    ///
    /// For example:
    /// - check that a 3rd party notification API (e.g. email service) is called when an event
    ///   in your application is supposed to trigger a notification;
    /// - check that a 3rd party API is NOT called when the response of a call is expected
    ///   to be retrieved from a cache (`.expect(0)`).
    ///
    /// This technique is also called [spying](https://martinfowler.com/bliki/TestDouble.html).
    pub fn expect(mut self, times: impl Into<Times>) -> Self {
        self.expected_calls = Arc::new(times.into());
        self
    }

    pub fn add_matcher(mut self, matcher: impl Match + Send + Sync + 'static) -> Self {
        assert!(self.matcher.len() < 65, "Cannot have more than 65 matchers");
        self.matcher.push(Arc::new(matcher));
        self
    }

    /// Specify a priority for this [`Mock`].
    /// Use this when you mount many [`Mock`] in a [`MockServer`]
    /// and those mocks have interlaced request matching conditions
    /// e.g. `mock A` accepts path `/abcd` and `mock B` a path regex `[a-z]{4}`
    /// It is recommended to set the highest priority (1) for mocks with exact conditions (`mock A` in this case)
    /// `1` is the highest priority, `255` the lowest, default to `5`
    /// If two mocks have the same priority, priority is defined by insertion order (first one mounted has precedence over the others).
    pub fn with_priority(mut self, priority: u8) -> Self {
        assert!(priority > 0, "priority must be strictly greater than 0!");
        self.priority = priority;
        self
    }

    /// Sets a responder for the `Mock`. If you have a simpler function you can use
    /// `Mock::one_to_one_response` or `Mock::response_stream` to determine how the `Mock` responds.
    pub fn set_responder(mut self, responder: impl ResponseStream + Send + Sync + 'static) -> Self {
        self.responder = Arc::new(responder);
        self
    }

    /// This `Mock` will respond with a stream of `Messages` independent of the inputs received.
    pub fn response_stream<F, S>(mut self, ctor: F) -> Self
    where
        F: Fn() -> S + Send + Sync + 'static,
        S: Stream<Item = Message> + Send + Sync + 'static,
    {
        self.responder = Arc::new(StreamResponse::new(ctor));
        self
    }

    /// For each `Message` from the client respond with a `Message`.
    pub fn one_to_one_response<F>(mut self, map_fn: F) -> Self
    where
        F: Fn(Message) -> Message + Send + Sync + 'static,
    {
        self.responder = Arc::new(MapResponder::new(map_fn));
        self
    }

    /// You can use this to verify the mock separately to the one you put into the server (if
    /// you've cloned it).
    pub fn verify(&self) -> bool {
        let calls = self.calls.load(Ordering::SeqCst);
        debug!("mock hit over {} calls", calls);
        // If this mock doesn't need calling we don't need to check the hit matches
        self.expected_calls.contains(calls)
    }

    fn check_request(
        &self,
        path: &str,
        headers: &HeaderMap,
        params: &HashMap<String, String>,
    ) -> (MatchStatus, u64) {
        let values = self
            .matcher
            .iter()
            .map(|x| x.request_match(path, headers, params))
            .collect::<Vec<Option<bool>>>();

        self.check_matcher_responses(&values)
    }

    fn check_message(&self, state: &mut MatchState) -> (MatchStatus, u64) {
        let values = self
            .matcher
            .iter()
            .map(|x| x.temporal_match(state))
            .collect::<Vec<Option<bool>>>();

        self.check_matcher_responses(&values)
    }

    fn check_matcher_responses(&self, values: &[Option<bool>]) -> (MatchStatus, u64) {
        if values.iter().copied().all(can_consider) {
            let contains_true = values.contains(&Some(true));
            let contains_none = values.contains(&None);

            if contains_true {
                let mut current_mask = 0u64;
                for (i, _val) in values.iter().enumerate().filter(|(_, i)| **i == Some(true)) {
                    current_mask |= 1 << i as u64;
                }

                if !contains_none {
                    (MatchStatus::Full, current_mask)
                } else {
                    (MatchStatus::Partial, current_mask)
                }
            } else {
                (MatchStatus::Potential, 0)
            }
        } else {
            (MatchStatus::Mismatch, 0)
        }
    }

    fn expected_mask(&self) -> u64 {
        u64::MAX >> (64 - self.matcher.len() as u64)
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
    request_counter: Extension<watch::Sender<usize>>,
) -> Response {
    ws_handler(
        ws,
        Path(String::new()),
        headers,
        params,
        mocks,
        request_counter,
    )
    .await
}

#[inline(always)]
fn can_consider(match_res: Option<bool>) -> bool {
    matches!(match_res, Some(true) | None)
}

struct ActiveMockCandidate {
    index: usize,
    mock_status: MatchStatus,
    priority: u8,
    mask: u64,
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Path(path): Path<String>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    mocks: Extension<MockList>,
    request_counter: Extension<watch::Sender<usize>>,
) -> Response {
    request_counter.send_modify(|x| *x += 1);
    let mut active_mocks = vec![];
    let mut matched_mock: Option<ActiveMockCandidate> = None;
    let mut current_mask = 0;
    {
        debug!("checking request level matches");
        let mocks = mocks.read().await.clone();
        for (index, mock) in mocks.iter().enumerate() {
            let (mock_status, mask) = mock.check_request(&path, &headers, &params);
            debug!("Mock status: {:?}", mock_status);
            if mock_status != MatchStatus::Mismatch {
                active_mocks.push(index);
            }

            if matches!(mock_status, MatchStatus::Full | MatchStatus::Partial) {
                if let Some(best_match) = &mut matched_mock {
                    if mock_status > best_match.mock_status
                        || (mock_status == best_match.mock_status
                            && mock.priority < best_match.priority)
                    {
                        best_match.index = index;
                        best_match.mock_status = mock_status;
                        best_match.priority = mock.priority;
                        best_match.mask = mask;
                    }
                } else {
                    matched_mock = Some(ActiveMockCandidate {
                        index,
                        mock_status,
                        priority: mock.priority,
                        mask,
                    });
                }
            }
        }
    }

    if let Some(active) = matched_mock {
        active_mocks = vec![active.index];
        current_mask = active.mask;
    }

    debug!("about to upgrade websocket connection");
    ws.on_upgrade(move |socket| async move {
        let res =
            tokio::task::spawn(handle_socket(socket, mocks.0, active_mocks, current_mask)).await;
        if let Err(res) = res {
            error!("Task panicked: {}", res);
        }
        request_counter.send_modify(|x| *x -= 1);
    })
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

async fn handle_socket(
    mut socket: WebSocket,
    mocks: MockList,
    mut active_mocks: Vec<usize>,
    mut mask: u64,
) {
    debug!("Active mock indexes are: {:?}", active_mocks);

    let mut no_mismatch = true;

    // Clone the mocks present when the connection comes in
    let mocks: Vec<Mock> = mocks.read().await.clone();
    let mut active_mocks = active_mocks
        .iter()
        .filter_map(|m| mocks.get(*m))
        .collect::<Vec<&Mock>>();

    let (sender, mut receiver) = socket.split();
    let (mut msg_tx, msg_rx) = broadcast::channel(128);

    let mut receiver_holder = Some(msg_rx);
    let mut sender_holder = Some(sender);

    let mut sender_task = if active_mocks.len() == 1 {
        let stream = active_mocks[0]
            .responder
            .handle(receiver_holder.take().unwrap());
        let sender = sender_holder.take().unwrap();
        let handle = tokio::task::spawn(async move {
            stream
                .map(|x| Ok(unconvert_message(x)))
                .forward(sender)
                .await
        });
        debug!("Spawned responder task");
        Some(handle)
    } else {
        debug!("Ambiguous matching, responder launch pending");
        None
    };

    let mut state = MatchState::new();

    while let Some(msg) = receiver.next().await {
        state.evict();
        if let Ok(msg) = msg {
            let msg = convert_message(msg);
            if let Err(e) = msg_tx.send(msg.clone()) {
                error!("Dropping messages");
            }
            debug!("Checking: {:?}", msg);
            state.push_message(msg);
            if active_mocks.len() == 1 {
                let (status, message_mask) = active_mocks[0].check_message(&mut state);
                mask |= message_mask;
                debug!("Active mock status: {:?}", status);

                if status == MatchStatus::Mismatch {
                    no_mismatch = false;
                }
            } else {
                let mut matched_mock: Option<ActiveMockCandidate> = None;
                let mut priorities = vec![];
                let mut masks = vec![];
                for (index, mock) in active_mocks.iter().enumerate() {
                    let (mock_status, mask_hits) = mock.check_message(&mut state);
                    if mock_status != MatchStatus::Mismatch {
                        priorities.push(mock.priority);
                    } else {
                        // TODO this will break if the user actually uses priority 255..
                        priorities.push(u8::MAX);
                    }
                    masks.push(mask_hits);
                    if matches!(mock_status, MatchStatus::Full | MatchStatus::Partial) {
                        if let Some(best_match) = &mut matched_mock {
                            if mock_status > best_match.mock_status
                                || (mock_status == best_match.mock_status
                                    && mock.priority < best_match.priority)
                            {
                                best_match.index = index;
                                best_match.mock_status = mock_status;
                                best_match.priority = mock.priority;
                                best_match.mask = mask_hits;
                            }
                        } else {
                            matched_mock = Some(ActiveMockCandidate {
                                index,
                                mock_status,
                                priority: mock.priority,
                                mask: mask_hits,
                            });
                        }
                    }
                }
                match matched_mock {
                    Some(active) => {
                        let active_mock = active_mocks.remove(active.index);
                        active_mocks = vec![active_mock];
                        mask |= active.mask;
                    }
                    None => {
                        let top_priority = priorities
                            .iter()
                            .enumerate()
                            .min_by_key(|(index, priority)| **priority)
                            .map(|(index, _)| index);
                        match top_priority {
                            Some(active) => {
                                let active_mock = active_mocks.remove(active);
                                active_mocks = vec![active_mock];
                                mask |= masks[active];

                                let stream = active_mocks[0]
                                    .responder
                                    .handle(receiver_holder.take().unwrap());
                                let sender = sender_holder.take().unwrap();
                                let handle = tokio::task::spawn(async move {
                                    stream
                                        .map(|x| Ok(unconvert_message(x)))
                                        .forward(sender)
                                        .await
                                });
                                debug!("Spawned responder task");
                                sender_task = Some(handle);
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
    debug!(
        "Checking actual mask {:b} vs expected {:b}",
        mask,
        active_mocks[0].expected_mask()
    );
    if mask == active_mocks[0].expected_mask() && no_mismatch {
        active_mocks[0].register_hit();
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

        let (tx, active_requests) = watch::channel(0);

        let router = Router::new()
            .route("/{*path}", any(ws_handler))
            .route("/", any(ws_handler_pathless))
            .layer(Extension(mocks.clone()))
            .layer(Extension(tx));
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
            active_requests: Mutex::new(active_requests),
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

    /// Asserts on [`MockServer::mocks_pass`]
    pub async fn verify(&self) {
        assert!(self.mocks_pass().await);
    }

    /// Returns true if all mocks pass. This method will wait for all on-going websocket
    /// connections to close before responding as match status is updated at the end of a request.
    /// If a connection is open then this method can deadlock.
    pub async fn mocks_pass(&self) -> bool {
        let mut active_requests = self.active_requests.lock().await;
        // If there's no more senders then in
        if let Err(e) = active_requests
            .wait_for(|x| {
                debug!("Current active requests: {}", x);
                *x == 0
            })
            .await
        {
            unreachable!("There should always be a sender while the server is running");
        }
        let mut res = true;
        for (index, mock) in self.mocks.read().await.iter().enumerate() {
            let mock_res = mock.verify();
            match &mock.name {
                None => debug!("Checking mock[{}]", index),
                Some(name) => debug!("Checking mock: {}", name),
            }
            debug!(
                "Expected {:?} Actual {:?}: {}",
                mock.expected_calls, mock.calls, mock_res
            );
            res &= mock_res;
        }
        res
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        let tx = self.shutdown.take().unwrap();
        let _ = tx.send(());
    }
}

/// Anything that implements `Match` can be used to constrain when a [`Mock`] is activated.
///
/// This type is more complicated than the `Match` trait in wiremock because of the differences in
/// websocket connections and the data interchanged. As such there are 3 domains where we can match
/// a request:
///
/// 1. At the initial request - headers, URL, query parameters
/// 2. A single websocket message in isolation
/// 3. Behaviour over the stream of messages
///
/// Point 2. is actually a subset of 3. but it is much simpler hence it's own method. Because a
/// `Match` may not be applied in all 3 domains there is the ability for it to return `None`. For
/// example, if we have a [`CloseFrameReceivedMatcher`] this only checks if a close frame is
/// received, every other component of the request is irrelevant and when checking them will return
/// a `None`. Likewise the `PatchExactMatcher` can only return `Some(true)` when we look at the
/// initial request parameters. Once the body is being received it's irrelevant.
///
/// # Temporal Matching
///
/// Here we care about the state of the stream of messages. When `Match::temporal_match` is called
/// it will be after a message is received. [`MatchState::last`] will return the most recent
/// message.
///
/// To avoid storing all messages if your `Match` implementation will require access to a message
/// in future passes use [`MatchState::keep_message`] to retain the message in the buffer. Likewise
/// when your `Match` doesn't want the message anymore call [`MatchStatus::forget_message`].
///
/// One thing to note is because unary message matching is a special case of temporal message
/// handling the default temporal matcher calls the unary method with [`MatchState::last`] as the
/// argument.
///
/// ## Note
///
/// If you call [`MatchStatus::forget_message`] twice for the same index in the same `Match`
/// instance during a connection you may evict a message which another `Match` required for
/// temporal checking. Take care you don't over-forget messages to avoid tests failing erroneous
/// (or worse passing eroneously).
///
/// This is done in part to allow for reduced resource usage (and ease of implementation).
///
/// # Implementing
///
/// [`Match::unary_match`] isn't called directly by wiremocket which instead relies on the default
/// implementation of [`Match::temporal_match`]. It is expected that a `Match` is implemented over
/// 1 of the listed domains.
///
/// If a `Match` returns `Some(true)` this is stored as part of the request state and failure for
/// every `Match` to return this during a request results in the verify check failing. Therefore
/// having a `Match` which checks multiple things that are independent of each other is likely to
/// lead to incorrect test results.
///
/// Simple implementation example on making a `Match` which checks if a `Ping` is received during
/// the session:
///
/// ```rust
/// use wiremocket::Match;
/// use tungstenite::Message;
///
/// pub struct PingOccursMatcher;
///
/// impl Match for PingOccursMatcher {
///     fn unary_match(&self, message: &Message) -> Option<bool> {
///         match message {
///             Message::Ping(_) => Some(true),
///             _ => None,
///         }
///     }
/// }
/// ```
pub trait Match {
    /// Check parameters available at connection initiation.
    fn request_match(
        &self,
        path: &str,
        headers: &HeaderMap,
        query: &HashMap<String, String>,
    ) -> Option<bool> {
        None
    }

    /// Check single websocket messages in isolation.
    fn unary_match(&self, message: &Message) -> Option<bool> {
        None
    }

    /// Check properties over multiple messages.
    fn temporal_match(&self, match_state: &mut MatchState) -> Option<bool> {
        self.unary_match(match_state.last())
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
