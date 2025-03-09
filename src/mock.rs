use crate::responder::{pending, MapResponder, ResponseStream, StreamResponse};
use crate::*;
use axum::http::header::HeaderMap;
use futures::stream::{Stream, StreamExt};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::sync::{oneshot, RwLock};

pub(crate) type MockList = Arc<RwLock<Vec<Mock>>>;

#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub(crate) enum MatchStatus {
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
    pub(crate) responder: Arc<dyn ResponseStream + Send + Sync + 'static>,
    pub(crate) expected_calls: Arc<Times>,
    pub(crate) calls: Arc<AtomicU64>,
    pub(crate) name: Option<String>,
    pub(crate) priority: u8,
}

impl Mock {
    /// Start building a [`Mock`] specifying the first matcher.
    ///
    /// TODO this should return a builder actually.
    pub fn given(matcher: impl Match + Send + Sync + 'static) -> MockBuilder {
        MockBuilder {
            matcher: vec![Arc::new(matcher)],
            responder: Arc::new(pending()),
            name: None,
            priority: 5,
        }
    }

    /// You can use this to verify the mock separately to the one you put into the server (if
    /// you've cloned it).
    pub fn verify(&self) -> bool {
        let calls = self.calls.load(Ordering::SeqCst);
        debug!("mock hit over {} calls", calls);
        // If this mock doesn't need calling we don't need to check the hit matches
        self.expected_calls.contains(calls)
    }

    pub(crate) fn check_request(
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

    pub(crate) fn check_message(&self, state: &mut MatchState) -> (MatchStatus, u64) {
        let values = self
            .matcher
            .iter()
            .map(|x| x.temporal_match(state))
            .collect::<Vec<Option<bool>>>();

        self.check_matcher_responses(&values)
    }

    pub(crate) fn check_matcher_responses(&self, values: &[Option<bool>]) -> (MatchStatus, u64) {
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

    pub(crate) fn expected_mask(&self) -> u64 {
        u64::MAX >> (64 - self.matcher.len() as u64)
    }

    pub(crate) fn register_hit(&self) {
        self.calls.fetch_add(1, Ordering::Acquire);
    }
}

/// A fluent builder to construct a [`Mock`] instance given matchers and a [`ResponseStream`].
#[derive(Debug)]
pub struct MockBuilder {
    matcher: Vec<Arc<dyn Match + Send + Sync + 'static>>,
    pub(crate) responder: Arc<dyn ResponseStream + Send + Sync + 'static>,
    pub(crate) name: Option<String>,
    pub(crate) priority: u8,
}

impl MockBuilder {
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
    pub fn expect(mut self, times: impl Into<Times>) -> Mock {
        Mock {
            matcher: self.matcher,
            responder: self.responder,
            expected_calls: Arc::new(times.into()),
            calls: Default::default(),
            name: self.name,
            priority: self.priority,
        }
    }

    /// Add another request matcher to the `Mock` you are building.
    ///
    /// **All** specified [`matchers`] must match for the overall [`Mock`] you are building.
    ///
    /// [`matchers`]: crate::matchers
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
}
