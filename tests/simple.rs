use bytes::Bytes;
use futures::SinkExt;
use serde_json::json;
use std::time::Duration;
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tracing_test::traced_test;
use tungstenite::client::IntoClientRequest;
use tungstenite::Message;
use webmocket::prelude::*;

#[tokio::test]
#[traced_test]
async fn can_connect() {
    let server = MockServer::start().await;

    println!("connecting to: {}", server.uri());

    let (stream, response) = connect_async(server.uri()).await.unwrap();
}

#[tokio::test]
#[traced_test]
async fn no_matches() {
    let server = MockServer::start().await;

    server
        .register(Mock::given(ValidJsonMatcher).expect(0))
        .await;

    let other_mock = Mock::given(path("/api")).named("path").expect(0);

    server.register(other_mock).await;

    let (mut stream, response) = connect_async(server.uri()).await.unwrap();

    server.verify().await;
    assert!(logs_contain("mock[0]"));
    assert!(logs_contain("mock: path"));
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
    // Make sure ping doesn't change anything
    stream.send(Message::Ping(vec![].into())).await.unwrap();

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
    // Make sure ping doesn't change anything
    let val = json!({"hello": "world"}).to_string().as_bytes().to_vec();
    stream.send(Message::Ping(val.into())).await.unwrap();

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

    // Send a message just to show it doesn't change anything.
    let val = json!({"hello": "world"});
    stream.send(Message::text(val.to_string())).await.unwrap();
    sleep(Duration::from_millis(200)).await;

    server.verify().await;
}

#[tokio::test]
#[traced_test]
async fn header_exists() {
    let server = MockServer::start().await;

    server
        .register(Mock::given(HeaderExistsMatcher::new("api-key")).expect(1..))
        .await;

    let mut request = server.uri().into_client_request().unwrap();
    request
        .headers_mut()
        .insert("api-key", "42".parse().unwrap());

    let (mut stream, response) = connect_async(request).await.unwrap();

    // Send a message just to show it doesn't change anything.
    let val = json!({"hello": "world"});
    stream.send(Message::text(val.to_string())).await.unwrap();
    sleep(Duration::from_millis(200)).await;

    server.verify().await;
}

#[tokio::test]
#[traced_test]
#[should_panic]
async fn header_doesnt_exist() {
    let server = MockServer::start().await;

    server
        .register(Mock::given(HeaderExistsMatcher::new("api-key")).expect(1..))
        .await;
    let (mut stream, response) = connect_async(server.uri()).await.unwrap();

    // Send a message just to show it doesn't change anything.
    let val = json!({"hello": "world"});
    stream.send(Message::text(val.to_string())).await.unwrap();
    sleep(Duration::from_millis(200)).await;

    server.verify().await;
}

#[tokio::test]
#[traced_test]
async fn header_exactly_matches() {
    let server = MockServer::start().await;

    server
        .register(Mock::given(HeaderExactMatcher::new("api-key", vec!["42", "45"])).expect(1..))
        .await;

    let mut request = server.uri().into_client_request().unwrap();
    request
        .headers_mut()
        .append("api-key", "42".parse().unwrap());
    request
        .headers_mut()
        .append("api-key", "45".parse().unwrap());
    // You're allowed an extra one, as a treat
    request
        .headers_mut()
        .append("api-key", "47".parse().unwrap());

    let (stream, response) = connect_async(request).await.unwrap();

    server.verify().await;
}

#[tokio::test]
#[traced_test]
#[should_panic]
async fn header_doesnt_match() {
    let server = MockServer::start().await;

    server
        .register(Mock::given(HeaderExactMatcher::new("api-key", vec!["42", "45"])).expect(1..))
        .await;

    let mut request = server.uri().into_client_request().unwrap();
    request
        .headers_mut()
        .insert("api-key", "42".parse().unwrap());

    let (stream, response) = connect_async(request).await.unwrap();

    server.verify().await;
}

#[tokio::test]
#[traced_test]
async fn query_param_matchers() {
    let server = MockServer::start().await;

    let mut mock = Mock::given(QueryParamExactMatcher::new("hello", "world"))
        .add_matcher(QueryParamContainsMatcher::new("foo", "ar"))
        .add_matcher(QueryParamIsMissingMatcher::new("not_here"));

    server.register(mock.expect(1..)).await;

    // I shouldn't need the path param
    let uri = format!("{}/what?hello=world&foo=bar", server.uri());

    let (stream, response) = connect_async(uri).await.unwrap();

    server.verify().await;
}

#[tokio::test]
#[traced_test]
async fn combine_request_and_content_matchers() {
    let server = MockServer::start().await;

    server
        .register(
            Mock::given(path("api/stream"))
                .add_matcher(ValidJsonMatcher)
                .expect(1..),
        )
        .await;

    let (mut stream, response) = connect_async(format!("{}/api", server.uri()))
        .await
        .unwrap();

    // Send a message just to show it doesn't change anything.
    let val = json!({"hello": "world"});
    stream.send(Message::text(val.to_string())).await.unwrap();
    sleep(Duration::from_millis(200)).await;

    assert!(!server.mocks_pass().await);

    let (mut stream, response) = connect_async(format!("{}/api/stream", server.uri()))
        .await
        .unwrap();

    // Send a message just to show it doesn't change anything.
    let val = json!({"hello": "world"});
    stream.send(Message::text(val.to_string())).await.unwrap();
    sleep(Duration::from_millis(200)).await;

    assert!(server.mocks_pass().await);
}
