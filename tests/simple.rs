use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::connect_async;
use tracing_test::traced_test;
use tungstenite::client::IntoClientRequest;
use tungstenite::Message;
use wiremocket::prelude::*;

#[tokio::test]
#[traced_test]
async fn can_connect() {
    let server = MockServer::start().await;

    println!("connecting to: {}", server.uri());

    let (stream, _response) = connect_async(server.uri()).await.unwrap();
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

    let (stream, _response) = connect_async(server.uri()).await.unwrap();

    std::mem::drop(stream);

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

    let (mut stream, _response) = connect_async(server.uri()).await.unwrap();

    let val = json!({"hello": "world"});

    stream.send(Message::text(val.to_string())).await.unwrap();

    let b = Bytes::from(serde_json::to_vec(&val).unwrap());

    stream.send(Message::binary(b)).await.unwrap();
    // Make sure ping doesn't change anything
    stream.send(Message::Ping(vec![].into())).await.unwrap();

    std::mem::drop(stream);

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

    let (mut stream, _response) = match connect_async(server.uri()).await {
        Ok(s) => s,
        Err(_) => return, // So we don't accidentally pass
    };

    if stream.send(Message::text("I'm not json")).await.is_err() {
        return;
    }
    // Make sure ping doesn't change anything
    let val = json!({"hello": "world"}).to_string().as_bytes().to_vec();
    if stream.send(Message::Ping(val.into())).await.is_err() {
        return;
    }

    std::mem::drop(stream);

    server.verify().await;
}

#[tokio::test]
#[traced_test]
async fn match_path() {
    let server = MockServer::start().await;

    server
        .register(Mock::given(path("api/stream")).expect(1..))
        .await;

    let (mut stream, _response) = connect_async(format!("{}/api/stream", server.uri()))
        .await
        .unwrap();

    // Send a message just to show it doesn't change anything.
    let val = json!({"hello": "world"});
    stream.send(Message::text(val.to_string())).await.unwrap();

    std::mem::drop(stream);

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

    let (mut stream, _response) = connect_async(request).await.unwrap();

    // Send a message just to show it doesn't change anything.
    let val = json!({"hello": "world"});
    stream.send(Message::text(val.to_string())).await.unwrap();

    std::mem::drop(stream);

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

    let (mut stream, _response) = match connect_async(server.uri()).await {
        Ok(s) => s,
        Err(_) => return,
    };

    // Send a message just to show it doesn't change anything.
    let val = json!({"hello": "world"});
    if stream.send(Message::text(val.to_string())).await.is_err() {
        return;
    }

    std::mem::drop(stream);

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

    let (stream, _response) = connect_async(request).await.unwrap();

    std::mem::drop(stream);

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

    let (stream, _response) = match connect_async(request).await {
        Ok(s) => s,
        Err(_) => return,
    };

    std::mem::drop(stream);

    server.verify().await;
}

#[tokio::test]
#[traced_test]
async fn query_param_matchers() {
    let server = MockServer::start().await;

    let mock = Mock::given(QueryParamExactMatcher::new("hello", "world"))
        .add_matcher(QueryParamContainsMatcher::new("foo", "ar"))
        .add_matcher(QueryParamIsMissingMatcher::new("not_here"));

    server.register(mock.expect(1..)).await;

    let uri = format!("{}/what?hello=world&foo=bar&not_here=1", server.uri());
    let (stream, _response) = connect_async(uri).await.unwrap();
    std::mem::drop(stream);
    assert!(!server.mocks_pass().await);

    let uri = format!("{}/what?foo=bar", server.uri());
    let (stream, _response) = connect_async(uri).await.unwrap();
    std::mem::drop(stream);
    assert!(!server.mocks_pass().await);

    let uri = format!("{}/what?foo=br&hello=world", server.uri());
    let (stream, _response) = connect_async(uri).await.unwrap();
    std::mem::drop(stream);
    assert!(!server.mocks_pass().await);

    let uri = format!("{}/what?hello=world&foo=bar", server.uri());

    let (stream, _response) = connect_async(uri).await.unwrap();

    std::mem::drop(stream);

    server.verify().await;

    // I shouldn't need the path param
    let uri = format!("{}/what?hello=world&foo=bar", server.uri());

    let (stream, _response) = connect_async(uri).await.unwrap();

    std::mem::drop(stream);

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

    let (mut stream, _response) = connect_async(format!("{}/api", server.uri()))
        .await
        .unwrap();

    // Send a message just to show it doesn't change anything.
    let val = json!({"hello": "world"});
    stream.send(Message::text(val.to_string())).await.unwrap();
    std::mem::drop(stream);

    assert!(!server.mocks_pass().await);

    let (mut stream, _response) = connect_async(format!("{}/api/stream", server.uri()))
        .await
        .unwrap();

    // Send a message just to show it doesn't change anything.
    let val = json!({"hello": "world"});
    stream.send(Message::text(val.to_string())).await.unwrap();

    std::mem::drop(stream);

    assert!(server.mocks_pass().await);
}

#[tokio::test]
#[traced_test]
async fn echo_response_test() {
    let server = MockServer::start().await;

    let responder = echo_response();

    server
        .register(
            Mock::given(path("api/stream"))
                .add_matcher(ValidJsonMatcher)
                .set_responder(responder)
                .expect(1..),
        )
        .await;

    let (mut stream, _response) = connect_async(format!("{}/api/stream", server.uri()))
        .await
        .unwrap();

    // Send a message just to show it doesn't change anything.
    let val = json!({"hello": "world"});
    let sent_message = Message::text(val.to_string());
    stream.send(sent_message.clone()).await.unwrap();

    let echoed = stream.next().await.unwrap().unwrap();

    assert_eq!(sent_message, echoed);

    std::mem::drop(stream);

    assert!(server.mocks_pass().await);
}

#[tokio::test]
#[traced_test]
async fn ensure_close_frame_sent() {
    let server = MockServer::start().await;

    server
        .register(
            Mock::given(path("api/stream"))
                .add_matcher(CloseFrameReceivedMatcher)
                .expect(1..),
        )
        .await;

    let (mut stream, _response) = connect_async(format!("{}/api/stream", server.uri()))
        .await
        .unwrap();

    // Send a message just to show it doesn't change anything.
    let val = json!({"hello": "world"});
    stream.send(Message::text(val.to_string())).await.unwrap();

    std::mem::drop(stream);

    assert!(!server.mocks_pass().await);

    let (mut stream, _response) = connect_async(format!("{}/api/stream", server.uri()))
        .await
        .unwrap();

    // Send a message just to show it doesn't change anything.
    let val = json!({"hello": "world"});
    stream.send(Message::text(val.to_string())).await.unwrap();
    stream.send(Message::Close(None)).await.unwrap();

    std::mem::drop(stream);

    assert!(server.mocks_pass().await);
}

#[tokio::test]
#[traced_test]
async fn matcher_priority() {
    let server = MockServer::start().await;

    server
        .register(Mock::given(path("api/stream")).with_priority(1).expect(1))
        .await;

    server
        .register(Mock::given(path("api/stream")).with_priority(2).expect(0))
        .await;

    let (mut stream, _response) = connect_async(format!("{}/api/stream", server.uri()))
        .await
        .unwrap();

    std::mem::drop(stream);

    assert!(server.mocks_pass().await);
}

#[tokio::test]
#[traced_test]
#[should_panic]
async fn matcher_priority_fails() {
    let server = MockServer::start().await;

    server
        .register(Mock::given(path("api/stream")).with_priority(3).expect(1))
        .await;

    server
        .register(Mock::given(path("api/stream")).with_priority(2).expect(0))
        .await;

    let (mut stream, _response) = match connect_async(format!("{}/api/stream", server.uri())).await
    {
        Ok(s) => s,
        Err(_) => return,
    };

    std::mem::drop(stream);

    assert!(server.mocks_pass().await);
}
