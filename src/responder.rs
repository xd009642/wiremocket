use async_stream::stream;
use futures::{
    stream::{self, BoxStream},
    Stream, StreamExt,
};
use std::future::ready;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::sleep;
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

pub trait ResponseStream {
    fn handle(&self, input: broadcast::Receiver<Message>) -> BoxStream<'static, Message>;
}

pub struct StreamResponse {
    stream_ctor: Arc<dyn Fn() -> BoxStream<'static, Message> + Send + Sync + 'static>,
}

impl StreamResponse {
    pub fn new<F, S>(ctor: F) -> Self 
    where
        F: Fn() -> S + Send + Sync + 'static,
        S: Stream<Item = Message> + Send + Sync + 'static,
    {
        let stream_ctor = Arc::new(move || ctor().boxed());
        Self {
            stream_ctor
        }
    }
}

impl ResponseStream for StreamResponse {
    fn handle(&self, _: broadcast::Receiver<Message>) -> BoxStream<'static, Message> {
        (self.stream_ctor)()
    }
}

pub fn pending() -> StreamResponse {
    StreamResponse::new(stream::pending)
}

pub struct MapResponder {
    map: Arc<dyn Fn(Message) -> Message + Send + Sync + 'static>,
}

impl MapResponder {
    pub fn new<F: Fn(Message) -> Message + Send + Sync + 'static>(f: F) -> Self {
        Self {
            map: Arc::new(f)
        }
    }
}

impl ResponseStream for MapResponder {
    fn handle(&self, input: broadcast::Receiver<Message>) -> BoxStream<'static, Message> {
        let map_fn = Arc::clone(&self.map);

        let mut input = BroadcastStream::new(input);

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

pub fn echo_response() -> MapResponder {
    MapResponder::new(|msg| msg)
}
