//! API slightly based off wiremock in that you start a server
use std::time::Instant;
use tungstenite::Message;

/// Server here we'd apply our mock servers and ability to verify requests. Based off of
/// https://docs.rs/wiremock/latest/wiremock/struct.MockServer.html
pub struct MockServer;

/// Specify things like the routes this responds to i.e. `/api/ws-stream` query parameters, and
/// behaviour it should exhibit in terms of source/sink messages. Also, will have matchers to allow
/// you to do things like "make sure all messages are valid json"
pub struct Mock {
    matcher: Vec<Box<dyn Match + 'static>>,
}

impl Mock {
    pub fn given(matcher: impl Match + 'static) -> Self {
        Self {
            matcher: vec![Box::new(matcher)],
        }
    }

    pub fn add_matcher(&mut self, matcher: impl Match + 'static) {
        self.matcher.push(Box::new(matcher));
    }
}

pub struct RecordedConnection {
    incoming: Vec<(Instant, Message)>,
    outgoing: Vec<(Instant, Message)>,
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
    pub fn start() -> Self {
        todo!()
    }

    /// Register a mock on an instance of the mock server.
    pub fn register(&self, mock: Mock) {}

    /// Return the base uri of this running instance of MockServer, e.g.
    /// ws://127.0.0.1:4372.
    ///
    /// Use this method to compose uris when interacting with this instance of
    /// MockServer via a websocket client.
    pub fn uri(&self) -> String {
        todo!("Return URI of our mock server");
    }

    /// Return a vector with all the recorded connections to the server. In each recorded
    /// connection you can see the incoming and outgoing messages and when they happened
    pub fn sessions(&self) -> Vec<RecordedConnection> {
        todo!("Record some connection info");
    }

    pub fn verify(&self) {
        todo!()
    }
}

pub trait Match {
    fn unary_match(&self, msg: Message) -> bool {
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
        fn unary_match(&self, msg: Message) -> bool {
            match msg {
                Message::Text(t) => serde_json::from_str::<Value>(&t).is_ok(),
                Message::Binary(b) => serde_json::from_slice::<Value>(b.as_slice()).is_ok(),
                _ => true, // We can't be judging pings/pongs/closes
            }
        }
    }
}
