#![doc = include_str!("../README.md")]
use crate::match_state::*;
use axum::http::header::HeaderMap;
use std::collections::HashMap;
use tracing::debug;
use tungstenite::Message;

pub use crate::mock::*;
pub use crate::server::*;
pub use crate::utils::*;

pub mod match_state;
pub mod matchers;
pub mod mock;
pub mod responder;
pub mod server;
pub mod utils;

/// Re-exports every part of the public API for ease of use.
pub mod prelude {
    pub use super::*;
    pub use crate::match_state::*;
    pub use crate::matchers::*;
    pub use crate::responder::*;
}

#[inline(always)]
fn can_consider(match_res: Option<bool>) -> bool {
    matches!(match_res, Some(true) | None)
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
/// example, if we have a [`CloseFrameReceivedMatcher`](crate::matchers::CloseFrameReceivedMatcher)
/// this only checks if a close frame is received, every other component of the request is irrelevant
/// and when checking them will return a `None`. Likewise the `PatchExactMatcher` can only return
/// `Some(true)` when we look at the initial request parameters. Once the body is being received
/// it's irrelevant.
///
/// # Temporal Matching
///
/// Here we care about the state of the stream of messages. When `Match::temporal_match` is called
/// it will be after a message is received. [`MatchState::last`](crate::match_state::MatchState::last)
/// will return the most recent message.
///
/// To avoid storing all messages if your `Match` implementation will require access to a message
/// in future passes use [`MatchState::keep_message`](crate::match_state::MatchState::keep_message)
/// to retain the message in the buffer. Likewise when your `Match` doesn't want the message anymore call
/// [`MatchState::forget_message`](crate::match_state::MatchState::forget_message).
///
/// One thing to note is because unary message matching is a special case of temporal message
/// handling the default temporal matcher calls the unary method with
/// [`MatchState::last`](crate::match_state::MatchState::last) as the argument.
///
/// ## Note
///
/// If you call [`MatchState::forget_message`](crate::match_state::MatchState::forget_message)
/// twice for the same index in the same `Match` instance during a connection you may evict a
/// message which another `Match` required for temporal checking. Take care you don't over-forget
/// messages to avoid tests failing erroneous (or worse passing eroneously).
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
    #[allow(unused_variables)]
    fn request_match(
        &self,
        path: &str,
        headers: &HeaderMap,
        query: &HashMap<String, String>,
    ) -> Option<bool> {
        None
    }

    /// Check single websocket messages in isolation.
    #[allow(unused_variables)]
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
    F: Fn(&Message) -> Option<bool>,
    F: Send + Sync,
{
    fn unary_match(&self, msg: &Message) -> Option<bool> {
        self(msg)
    }
}
