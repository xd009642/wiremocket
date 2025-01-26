use bytes::Bytes;
use futures::SinkExt;
use serde_json::json;
use std::time::Duration;
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tungstenite::Message;
use webmocket::*;

#[tokio::test]
async fn can_connect() {
    let server = MockServer::start().await;

    println!("connecting to: {}", server.uri());

    let (stream, response) = connect_async(server.uri()).await.unwrap();
}

#[tokio::test]
async fn only_json_matcher() {
    let server = MockServer::start().await;

    server
        .register(Mock::given(ValidJsonMatcher).expect(1..))
        .await;

    let (mut stream, response) = connect_async(server.uri()).await.unwrap();

    let val = json!({"hello": "world"});

    stream.send(Message::text(val.to_string())).await.unwrap();

    let b = Bytes::from(serde_json::to_vec(&val).unwrap());

    stream.send(Message::binary(b)).await.unwrap();

    sleep(Duration::from_millis(100)).await;

    server.verify().await;
}

#[tokio::test]
#[should_panic]
async fn deny_invalid_json() {
    let server = MockServer::start().await;

    server
        .register(Mock::given(ValidJsonMatcher).expect(1))
        .await;

    let (mut stream, response) = connect_async(server.uri()).await.unwrap();

    stream.send(Message::text("I'm not json")).await.unwrap();

    sleep(Duration::from_millis(100)).await;

    server.verify().await;
}
