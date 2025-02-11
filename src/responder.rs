use futures::{
    stream::{self, BoxStream},
    Stream, StreamExt,
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tungstenite::Message;
use std::time::Duration;
use tokio::time::sleep;

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
    fn handle(self, input: mpsc::Receiver<Message>) -> BoxStream<'static, Message>;
}

impl<S> ResponseStream for S
where
    S: Stream<Item = Message> + Send + Sync + 'static,
{
    fn handle(self, _: mpsc::Receiver<Message>) -> BoxStream<'static, Message> {
        self.boxed()
    }
}

pub fn pending() -> impl ResponseStream + Send + Sync + 'static {
    stream::pending()
}

// TODO we need rate throttling UX at some point
