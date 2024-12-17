use futures::SinkExt;
use serde_json::json;
use tokio_tungstenite::connect_async;
use tungstenite::Message;
use webmocket::*;

#[tokio::test]
async fn can_connect() {
    let server = MockServer::start();

    let (stream, response) = connect_async(server.uri()).await.unwrap();
}

#[tokio::test]
async fn only_json_matcher() {
    let server = MockServer::start();

    server.register(Mock::given(ValidJsonMatcher));

    let (mut stream, response) = connect_async(server.uri()).await.unwrap();

    let val = json!({"hello": "world"});

    stream.send(Message::text(val.to_string())).await.unwrap();
    stream
        .send(Message::binary(val.to_string().as_bytes()))
        .await
        .unwrap();

    server.verify()
}

#[tokio::test]
#[should_panic]
async fn deny_invalid_json() {
    let server = MockServer::start();

    server.register(Mock::given(ValidJsonMatcher));

    let (mut stream, response) = connect_async(server.uri()).await.unwrap();

    stream.send(Message::text("I'm not json")).await.unwrap();

    server.verify()
}
