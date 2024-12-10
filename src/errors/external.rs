//! [errors](/error/#external-errors)
//!
//! This module supplies API consuming error types.
//! It is intended to be user-facing so that internal errors are not exposed.
//! See [error](/error/) for more information.

// TODO: Once actual client-side errors become apparent (e.g.
// when ensuring certain properties are met, replace the `Box` with
// a concrete type.
use std::fmt::Debug;

#[derive(Debug)]
pub struct WebmocketErr<T: Debug + ?Sized>(pub Box<T>);
