use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tracing_test::traced_test;
use tungstenite::client::IntoClientRequest;
use tungstenite::Message;
use wiremocket::prelude::*;

struct BinaryStreamMatcher;

impl Match for BinaryStreamMatcher {
    fn temporal_match(&self, match_state: &mut MatchState) -> Option<bool> {
        let json = ValidJsonMatcher;
        let len = match_state.len();
        let last = match_state.last();
        if len == 1 && json.unary_match(last).unwrap() {
            match_state.keep_message(0);
            Some(true)
        } else if last.is_binary() {
            // We won't keep any binary messages!
            if len > 1 && match_state.get_message(len - 2).is_none() {
                Some(true)
            } else {
                Some(false)
            }
        } else if last.is_close() {
            if len == 1 {
                None
            } else {
                let message = match_state.get_message(len - 2);
                if let Some(message) = message {
                    let res = json.unary_match(message);
                    println!(
                        "Got close frame, checking {}: {:?}\ti{:?}",
                        len - 2,
                        res,
                        message
                    );
                    res
                } else {
                    Some(false)
                }
            }
        } else if last.is_text() {
            let res = json.unary_match(last);
            match_state.keep_message(len - 1);
            res
        } else {
            None
        }
    }
}

#[tokio::test]
#[traced_test]
async fn binary_stream_matcher_passes() {
    let server = MockServer::start().await;

    // So our pretend API here will send off a json and then after that every packet will be binary
    // and then the last one a json followed by a close
    server
        .register(
            Mock::given(path("api/binary_stream"))
                .add_matcher(BinaryStreamMatcher)
                .add_matcher(CloseFrameReceivedMatcher)
                .expect(1..),
        )
        .await;

    println!("connecting to: {}", server.uri());

    let (mut stream, response) = connect_async(format!("{}/api/binary_stream", server.uri()))
        .await
        .unwrap();

    let val = json!({"command": "start"});
    stream.send(Message::text(val.to_string())).await.unwrap();
    let val = json!({"command": "stop"});
    stream.send(Message::text(val.to_string())).await.unwrap();
    stream.send(Message::Close(None)).await.unwrap();

    std::mem::drop(stream);

    server.verify().await;

    let (mut stream, response) = connect_async(format!("{}/api/binary_stream", server.uri()))
        .await
        .unwrap();

    let data: Vec<u8> = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 1, 2, 3, 4, 5, 6, 7];

    let val = json!({"command": "start"});
    stream.send(Message::text(val.to_string())).await.unwrap();
    stream.send(Message::Binary(data.into())).await.unwrap();
    let val = json!({"command": "stop"});
    stream.send(Message::text(val.to_string())).await.unwrap();
    stream.send(Message::Close(None)).await.unwrap();

    std::mem::drop(stream);

    server.verify().await;
}

#[tokio::test]
#[traced_test]
async fn binary_stream_matcher_fails() {
    let server = MockServer::start().await;

    // So our pretend API here will send off a json and then after that every packet will be binary
    // and then the last one a json followed by a close
    server
        .register(
            Mock::given(path("api/binary_stream"))
                .add_matcher(BinaryStreamMatcher)
                .add_matcher(CloseFrameReceivedMatcher)
                .expect(1..),
        )
        .await;

    let data: Vec<u8> = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 1, 2, 3, 4, 5, 6, 7];

    println!("connecting to: {}", server.uri());

    println!("Testing no start message");
    let (mut stream, response) = connect_async(format!("{}/api/binary_stream", server.uri()))
        .await
        .unwrap();

    stream
        .send(Message::Binary(data.clone().into()))
        .await
        .unwrap();
    let val = json!({"command": "stop"});
    stream.send(Message::text(val.to_string())).await.unwrap();
    stream.send(Message::Close(None)).await.unwrap();

    std::mem::drop(stream);

    assert!(!server.mocks_pass().await);

    println!("Testing no end message");
    let (mut stream, response) = connect_async(format!("{}/api/binary_stream", server.uri()))
        .await
        .unwrap();

    let val = json!({"command": "start"});
    stream.send(Message::text(val.to_string())).await.unwrap();
    stream
        .send(Message::Binary(data.clone().into()))
        .await
        .unwrap();
    stream.send(Message::Close(None)).await.unwrap();

    std::mem::drop(stream);

    assert!(!server.mocks_pass().await);

    let (mut stream, response) = connect_async(format!("{}/api/binary_stream", server.uri()))
        .await
        .unwrap();

    println!("Testing no close");

    let val = json!({"command": "start"});
    stream.send(Message::text(val.to_string())).await.unwrap();
    stream.send(Message::Binary(data.into())).await.unwrap();
    let val = json!({"command": "stop"});
    stream.send(Message::text(val.to_string())).await.unwrap();

    std::mem::drop(stream);

    assert!(!server.mocks_pass().await);
}
