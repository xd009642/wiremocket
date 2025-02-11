use bytes::Bytes;
use futures::SinkExt;
use serde_json::json;
use std::time::Duration;
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tracing_test::traced_test;
use tungstenite::Message;
use webmocket::*;

#[tokio::test]
#[traced_test]
async fn can_connect() {
    let server = MockServer::start().await;

    println!("connecting to: {}", server.uri());

    let (stream, response) = connect_async(server.uri()).await.unwrap();
}

#[tokio::test]
#[traced_test]
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

    // TODO there should be a better way than this
    sleep(Duration::from_millis(100)).await;

    server.verify().await;
}

#[tokio::test]
#[traced_test]
#[should_panic]
async fn deny_invalid_json() {
    let server = MockServer::start().await;

    server
        .register(Mock::given(ValidJsonMatcher).expect(1))
        .await;

    let (mut stream, response) = connect_async(server.uri()).await.unwrap();

    stream.send(Message::text("I'm not json")).await.unwrap();

    // TODO there should be a better way than this
    sleep(Duration::from_millis(200)).await;

    server.verify().await;
}

#[tokio::test]
#[traced_test]
async fn match_path() {
    let server = MockServer::start().await;

    server
        .register(Mock::given(path("api/stream")).expect(1..))
        .await;

    let (mut stream, response) = connect_async(format!("{}/api/stream", server.uri()))
        .await
        .unwrap();

    server.verify().await;
}
