use crate::Match;
use axum::http::header::HeaderMap;
use std::collections::HashMap;
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
    ) -> bool {
        self.0 == path
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
