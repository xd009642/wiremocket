//! Determine how a given mock responds to the client.
use async_stream::stream;
use futures::{
    stream::{self, BoxStream},
    Stream, StreamExt,
};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tracing::warn;
use tungstenite::Message;

// Design thoughts I want:
//
// 1. Ability to send a message (always good)
// 2. Send messages in response to a message from the client
// 3. Send messages without receiving anything from the client
// 4. Interleave these potential different sources of messages
//
// Now maybe the easiest way to do this is just creating a stream that outputs messages, people can
// use futures crate things to make streams from iterators etc etc. But `Stream` isn't super
// ergonomic so maybe there's a better way. (Internally for this I'd use the fact the socket
// implements `Sink` to forward the `Stream` to the `Sink`.
//
// Single messages being sent out could be solved by putting the websocket (more likely something
// that sends to it into the `Match` trait so people can send responses on matches). Or I make the
// responder take the last client message as an output.

/// All [`Mock`]s must have a valid responder. By default the `pending` responder is provided which
/// never outputs a message.
pub trait ResponseStream {
    /// Given a broadcast of messages from the client returns a stream of responses.
    ///
    /// # Implementing
    ///
    /// The most complex implementation pattern is when the output messages rely on the input ones
    /// and there is not a 1-to-1 relationship between the two. In this case the easiest thing is
    /// likely to make use of the [`async_stream`](https://crates.io/crates/async_stream) and
    /// implement something as follows:
    ///
    /// ```rust
    /// use async_stream::stream;
    /// use futures::{stream::BoxStream, StreamExt};
    /// use tokio::sync::broadcast;
    /// use tokio_stream::wrappers::BroadcastStream;
    /// use tungstenite::Message;
    /// use wiremocket::prelude::ResponseStream;
    ///
    /// pub struct ExampleResponder;
    ///
    /// impl ResponseStream for ExampleResponder {
    ///
    ///     fn handle(&self, input: broadcast::Receiver<Message>) -> BoxStream<'static, Message> {
    ///
    ///         let mut input = BroadcastStream::new(input);
    ///
    ///         let stream = stream! {
    ///             for await value in input {
    ///                 match value {
    ///                     Ok(v) => {
    ///                         // Echoes the stream and if the message len is >10 sends back a
    ///                         // ping
    ///                         if v.len() > 10 {
    ///                             yield Message::Ping(Default::default());
    ///                         }
    ///                         yield v.clone();
    ///                     },
    ///                     Err(e) => {
    ///                         panic!("ohno");
    ///                     }
    ///                 }
    ///             }
    ///         };
    ///         stream.boxed()
    ///     }
    /// }
    /// ```
    fn handle(&self, input: broadcast::Receiver<Message>) -> BoxStream<'static, Message>;
}

/// Typpe to hold streaming responses where a stream of server messages independent of the client
/// input is returned.
pub struct StreamResponse {
    stream_ctor: Arc<dyn Fn() -> BoxStream<'static, Message> + Send + Sync + 'static>,
}

impl StreamResponse {
    /// Creates a new `StreamResponse`. Because the stream needs to be constructed per session
    /// which matches we need a function that creates the stream rather than the stream itself.
    pub fn new<F, S>(ctor: F) -> Self
    where
        F: Fn() -> S + Send + Sync + 'static,
        S: Stream<Item = Message> + Send + Sync + 'static,
    {
        let stream_ctor = Arc::new(move || ctor().boxed());
        Self { stream_ctor }
    }
}

impl ResponseStream for StreamResponse {
    fn handle(&self, _: broadcast::Receiver<Message>) -> BoxStream<'static, Message> {
        (self.stream_ctor)()
    }
}

/// Create a output stream that never emits any messages but stays open.
pub fn pending() -> StreamResponse {
    StreamResponse::new(stream::pending)
}

/// Responds with a 1-to-1 mapping of `Message->Message`. The server will return as many messages
/// as the client sends.
pub struct MapResponder {
    map: Arc<dyn Fn(Message) -> Message + Send + Sync + 'static>,
}

impl MapResponder {
    /// Create a new `MapResponder`.
    pub fn new<F: Fn(Message) -> Message + Send + Sync + 'static>(f: F) -> Self {
        Self { map: Arc::new(f) }
    }
}

impl ResponseStream for MapResponder {
    fn handle(&self, input: broadcast::Receiver<Message>) -> BoxStream<'static, Message> {
        let map_fn = Arc::clone(&self.map);

        let input = BroadcastStream::new(input);

        let stream = stream! {
            for await value in input {
                match value {
                    Ok(v) => yield map_fn(v),
                    Err(e) => {
                        warn!("Broadcast error: {}", e);
                    }
                }
            }
        };
        stream.boxed()
    }
}

/// Has the server respond with the client's message.
pub fn echo_response() -> MapResponder {
    MapResponder::new(|msg| msg)
}
