# webmocket

🚧 **This crate is currently very WIP** 🚧

'webmocket' provides mocking so you can perform black-box testing of Rust
applications that interact with websocket APIs. It's heavily inspired by
[wiremock-rs](https://github.com/LukeMathWalker/wiremock-rs/) and is an
experimentation of how it could look like in a similar API. For a relevant
wiremock issue look [here](https://github.com/LukeMathWalker/wiremock-rs/issues/113).

## What's implemented So Far?

* Simple request checking of initial parameters and message stream (headers, paths and websocket messages)
* Checking of preconditions

## What's Yet To Come?

* Response streams
* Better verification reports
* Similar quality of UX to wiremock
* Figuring out if it lives separately or joins forces with wiremock-rs
