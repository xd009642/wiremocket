//! # webmocket
//!
//! A library for creating testing websockets.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use webmocket::prelude::*;
//!
//! fn main() -> Result<(), webmocket::Error> {
//!     let mut webmocket = Webmocket::default();
//!     todo!();
//! }
//! ```
pub(crate) mod constants;
pub mod errors;
pub mod message;
pub mod prelude;
pub mod server;

#[allow(unused_imports)]
#[cfg(test)]
mod tests {

    use super::*;
}
