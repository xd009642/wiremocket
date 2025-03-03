# wiremocket

[![Build Status](https://github.com/xd009642/wiremocket/workflows/Build/badge.svg)](https://github.com/xd009642/wiremocket/actions)
[![Latest Version](https://img.shields.io/crates/v/wiremocket.svg)](https://crates.io/crates/wiremocket)
[![License:MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![docs.rs](https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square)](https://docs.rs/wiremocket)]

'wiremocket' provides mocking so you can perform black-box testing of Rust
applications that interact with websocket APIs. It's heavily inspired by
[wiremock-rs](https://github.com/LukeMathWalker/wiremock-rs/) and is an
experimentation of how it could look like in a similar API. For a relevant
wiremock issue look [here](https://github.com/LukeMathWalker/wiremock-rs/issues/113).

There's still some work to do, but this is very nearly at an initial
version!

## How to install

```bash
cargo add wiremocket --dev
```

## Getting started

Here is an example of a wiremocket mock which makes sure all text messages
are valid json:

```rust
use serde_json::json;
use tokio_tungstenite::connect_async;
use tracing_test::traced_test;
use tungstenite::Message;
use wiremocket::prelude::*;

#[tokio::test]
async fn only_json_matcher() {
    let server = MockServer::start().await;

    server
        .register(Mock::given(ValidJsonMatcher).expect(1..))
        .await;

    let (mut stream, response) = connect_async(server.uri()).await.unwrap();

    let val = json!({"hello": "world"});

    stream.send(Message::text(val.to_string())).await.unwrap();

    stream.send(Message::Close(None)).await.unwrap();

    std::mem::drop(stream);

    server.verify().await;
}
```

More advanced matching based on the stream of messages and more advanced
response stream generation are also possible. Please check the docs for
more details!
