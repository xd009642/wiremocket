#![doc = include_str!("../README.md")]
use crate::match_state::*;
use crate::responder::ResponseStream;
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
use tokio::sync::{broadcast, oneshot, watch, Mutex};
use tracing::{debug, error, Instrument};
use tungstenite::{
    protocol::{frame::Utf8Bytes, CloseFrame},
    Message,
};

pub use crate::mock::*;
pub use crate::utils::*;

pub mod match_state;
pub mod matchers;
pub mod mock;
pub mod responder;
pub mod utils;

/// Re-exports every part of the public API for ease of use.
pub mod prelude {
    pub use super::*;
    pub use crate::match_state::*;
    pub use crate::matchers::*;
    pub use crate::responder::*;
}

/// Server here we'd apply our mock servers and ability to verify requests. Based off of
/// https://docs.rs/wiremock/latest/wiremock/struct.MockServer.html
pub struct MockServer {
    addr: String,
    shutdown: Option<oneshot::Sender<()>>,
    mocks: MockList,
    active_requests: Mutex<watch::Receiver<usize>>,
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
    socket: WebSocket,
    mocks: MockList,
    active_mocks: Vec<usize>,
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
    let (msg_tx, msg_rx) = broadcast::channel(128);

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
