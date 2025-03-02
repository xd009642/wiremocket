use crate::Match;
use axum::http::header::{HeaderMap, HeaderName, HeaderValue};
use std::collections::HashMap;
use std::fmt::Debug;
use tungstenite::Message;

pub struct PathExactMatcher(String);

pub fn path<T>(path: T) -> PathExactMatcher
where
    T: Into<String>,
{
    PathExactMatcher::new(path)
}

impl PathExactMatcher {
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
    pub fn new<Name>(name: Name) -> Self
    where
        Name: TryInto<HeaderName>,
        <Name as TryInto<HeaderName>>::Error: Debug,
    {
        let name = name.try_into().expect("invalid header name");
        Self(name)
    }
}

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
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

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
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

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
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

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

#[cfg(feature = "serde_json")]
pub mod json {
    use super::*;
    use serde_json::Value;

    pub struct ValidJsonMatcher;

    impl Match for ValidJsonMatcher {
        fn unary_match(&self, msg: &Message) -> Option<bool> {
            match msg {
                Message::Text(t) => Some(serde_json::from_str::<Value>(&t).is_ok()),
                Message::Binary(b) => Some(serde_json::from_slice::<Value>(b.as_ref()).is_ok()),
                _ => None,
            }
        }
    }
}
