//! A collection of different matching strategies provided out-of-the-box by `wiremocket`.
//!
//! If the set of matchers provided out-of-the-box is not enough for your specific testing needs
//! you can implement your own thanks to the [`Match`] trait.
//!
//! Furthermore, `Fn` closures that take an immutable [`tungstenite::Message`] reference as input
//! and return an `Option<bool>` as input automatically implement [`Match`] and can be used where
//! a matcher is expected for unary stream matching.
//!
//! Check [`Match`]'s documentation for examples.
use crate::Match;
use axum::http::header::{HeaderMap, HeaderName, HeaderValue};
use std::collections::HashMap;
use std::fmt::Debug;
use tungstenite::Message;

/// Match exactly the path of a request.
pub struct PathExactMatcher(String);

/// Shorthand for [`PathExactMatcher::new`].
pub fn path<T>(path: T) -> PathExactMatcher
where
    T: Into<String>,
{
    PathExactMatcher::new(path)
}

impl PathExactMatcher {
    /// Creates a new `PathExactMatcher`.
    pub fn new<T: Into<String>>(path: T) -> Self {
        Self(path.into())
    }
}

impl Match for PathExactMatcher {
    fn request_match(
        &self,
        path: &str,
        _headers: &HeaderMap,
        _query: &HashMap<String, String>,
    ) -> Option<bool> {
        Some(self.0 == path)
    }
}

/// Match exactly the header of a request.
pub struct HeaderExactMatcher(HeaderName, Vec<HeaderValue>);

impl Match for HeaderExactMatcher {
    fn request_match(
        &self,
        _path: &str,
        headers: &HeaderMap,
        _query: &HashMap<String, String>,
    ) -> Option<bool> {
        let all_values = headers.get_all(&self.0);
        Some(self.1.iter().all(|x| all_values.iter().any(|v| v == x)))
    }
}

impl HeaderExactMatcher {
    /// Create a new `HeaderExactMatcher`. Multiple `values` are provided for instances when the
    /// header should be present more than once.
    pub fn new<Name, Value>(name: Name, mut values: Vec<Value>) -> Self
    where
        Name: TryInto<HeaderName>,
        <Name as TryInto<HeaderName>>::Error: Debug,
        Value: TryInto<HeaderValue>,
        <Value as TryInto<HeaderValue>>::Error: Debug,
    {
        let name = name.try_into().expect("invalid header name");
        let values = values
            .drain(..)
            .map(|x| x.try_into().expect("invalid value to match on"))
            .collect();
        Self(name, values)
    }
}

/// Match exactly the header name of a request. It checks that the header is present but does not
/// validate the value.
pub struct HeaderExistsMatcher(HeaderName);

impl Match for HeaderExistsMatcher {
    fn request_match(
        &self,
        _path: &str,
        headers: &HeaderMap,
        _query: &HashMap<String, String>,
    ) -> Option<bool> {
        Some(headers.contains_key(&self.0))
    }
}

impl HeaderExistsMatcher {
    /// Creates a new `HeaderExistsMatcher`.
    pub fn new<Name>(name: Name) -> Self
    where
        Name: TryInto<HeaderName>,
        <Name as TryInto<HeaderName>>::Error: Debug,
    {
        let name = name.try_into().expect("invalid header name");
        Self(name)
    }
}

/// Match exactly the query parameter of a request.
pub struct QueryParamExactMatcher {
    name: String,
    value: String,
}

impl Match for QueryParamExactMatcher {
    fn request_match(
        &self,
        _path: &str,
        _headers: &HeaderMap,
        query: &HashMap<String, String>,
    ) -> Option<bool> {
        Some(query.get(&self.name) == Some(&self.value))
    }
}

impl QueryParamExactMatcher {
    /// Create a new `QueryParamExactMatcher`.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// Match when a query parameter contains the specified value as a substring.
pub struct QueryParamContainsMatcher {
    name: String,
    value: String,
}

impl Match for QueryParamContainsMatcher {
    fn request_match(
        &self,
        _path: &str,
        _headers: &HeaderMap,
        query: &HashMap<String, String>,
    ) -> Option<bool> {
        if let Some(s) = query.get(&self.name) {
            Some(s.contains(&self.value))
        } else {
            Some(false)
        }
    }
}

impl QueryParamContainsMatcher {
    /// Create a new `QueryParamContainsMatcher`
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// Only match requests that do not contain a specified query parameter.
pub struct QueryParamIsMissingMatcher(String);

impl Match for QueryParamIsMissingMatcher {
    fn request_match(
        &self,
        _path: &str,
        _headers: &HeaderMap,
        query: &HashMap<String, String>,
    ) -> Option<bool> {
        Some(!query.contains_key(&self.0))
    }
}

impl QueryParamIsMissingMatcher {
    /// Create a new `QueryParamIsMissingMatcher`.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

/// Match requests where the session is terminated with a `CloseFrame` from the client.
pub struct CloseFrameReceivedMatcher;

impl Match for CloseFrameReceivedMatcher {
    fn unary_match(&self, msg: &Message) -> Option<bool> {
        match msg {
            Message::Close(_) => Some(true),
            _ => None,
        }
    }
}

#[cfg(feature = "serde_json")]
pub use json::*;

/// Optional matchers that require the `serde_json` feature to be active.
#[cfg(feature = "serde_json")]
pub mod json {
    use super::*;
    use serde_json::Value;

    /// Match that every `Message::Binary` and `Message::Text` received by the server is valid
    /// json.
    pub struct ValidJsonMatcher;

    impl Match for ValidJsonMatcher {
        fn unary_match(&self, msg: &Message) -> Option<bool> {
            match msg {
                Message::Text(t) => Some(serde_json::from_str::<Value>(t).is_ok()),
                Message::Binary(b) => Some(serde_json::from_slice::<Value>(b.as_ref()).is_ok()),
                _ => None,
            }
        }
    }
}
