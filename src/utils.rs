//! Utility functions and types used by wiremocket.
use std::ops::{
    Range, RangeBounds, RangeFrom, RangeFull, RangeInclusive, RangeTo, RangeToInclusive,
};

// All code below is adapted from https://docs.rs/wiremock/latest/wiremock/struct.Times.html

/// Specify how many times we expect a [`Mock`] to match via [`expect`].
/// It is used to set expectations on the usage of a [`Mock`] in a test case.
///
/// You can either specify an exact value, e.g.
/// ```rust
/// use wiremocket::Times;
///
/// let times: Times = 10.into();
/// ```
/// or a range
/// ```rust
/// use wiremocket::Times;
///
/// // Between 10 and 15 (not included) times
/// let times: Times = (10..15).into();
/// // Between 10 and 15 (included) times
/// let times: Times = (10..=15).into();
/// // At least 10 times
/// let times: Times = (10..).into();
/// // Strictly less than 15 times
/// let times: Times = (..15).into();
/// // Strictly less than 16 times
/// let times: Times = (..=15).into();
/// ```
///
/// [`expect`]: Mock::expect
#[derive(Clone, Debug, Default)]
pub struct Times(TimesEnum);

impl Times {
    pub(crate) fn contains(&self, n_calls: u64) -> bool {
        match &self.0 {
            TimesEnum::Exact(e) => e == &n_calls,
            TimesEnum::Unbounded(r) => r.contains(&n_calls),
            TimesEnum::Range(r) => r.contains(&n_calls),
            TimesEnum::RangeFrom(r) => r.contains(&n_calls),
            TimesEnum::RangeTo(r) => r.contains(&n_calls),
            TimesEnum::RangeToInclusive(r) => r.contains(&n_calls),
            TimesEnum::RangeInclusive(r) => r.contains(&n_calls),
        }
    }
}

impl std::fmt::Display for Times {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            TimesEnum::Exact(e) => write!(f, "== {}", e),
            TimesEnum::Unbounded(_) => write!(f, "0 <= x"),
            TimesEnum::Range(r) => write!(f, "{} <= x < {}", r.start, r.end),
            TimesEnum::RangeFrom(r) => write!(f, "{} <= x", r.start),
            TimesEnum::RangeTo(r) => write!(f, "0 <= x < {}", r.end),
            TimesEnum::RangeToInclusive(r) => write!(f, "0 <= x <= {}", r.end),
            TimesEnum::RangeInclusive(r) => write!(f, "{} <= x <= {}", r.start(), r.end()),
        }
    }
}

// Implementation notes: this has gone through a couple of iterations before landing to
// what you see now.
//
// The original draft had Times itself as an enum with two variants (Exact and Range), with
// the Range variant generic over `R: RangeBounds<u64>`.
//
// We switched to a generic struct wrapper around a private `R: RangeBounds<u64>` when we realised
// that you would have had to specify a range type when creating the Exact variant
// (e.g. as you do for `Option` when creating a `None` variant).
//
// We achieved the same functionality with a struct wrapper, but exact values had to converted
// to ranges with a single element (e.g. 15 -> 15..16).
// Not the most expressive representation, but we would have lived with it.
//
// We changed once again when we started to update our `MockActor`: we are storing all `Mock`s
// in a vector. Being generic over `R`, the range type leaked into the overall `Mock` (and `MountedMock`)
// type, thus making those generic as well over `R`.
// To store them in a vector all mocks would have had to use the same range internally, which is
// obviously an unreasonable restrictions.
// At the same time, we can't have a Box<dyn RangeBounds<u64>> because `contains` is a generic
// method hence the requirements for object safety are not satisfied.
//
// Thus we ended up creating this master enum that wraps all range variants with the addition
// of the Exact variant.
// If you can do better, please submit a PR.
// We keep them enum private to the crate to allow for future refactoring.
#[derive(Clone, Debug)]
pub(crate) enum TimesEnum {
    Exact(u64),
    Unbounded(RangeFull),
    Range(Range<u64>),
    RangeFrom(RangeFrom<u64>),
    RangeTo(RangeTo<u64>),
    RangeToInclusive(RangeToInclusive<u64>),
    RangeInclusive(RangeInclusive<u64>),
}

impl Default for TimesEnum {
    fn default() -> Self {
        Self::Unbounded(RangeFull)
    }
}

impl From<RangeFull> for Times {
    fn from(r: RangeFull) -> Self {
        Times(TimesEnum::Unbounded(r))
    }
}

impl From<u64> for Times {
    fn from(x: u64) -> Self {
        Times(TimesEnum::Exact(x))
    }
}

// A quick macro to help easing the implementation pain.
macro_rules! impl_from_for_range {
    ($type_name:ident) => {
        impl From<$type_name<u64>> for Times {
            fn from(r: $type_name<u64>) -> Self {
                Times(TimesEnum::$type_name(r))
            }
        }
    };
}

impl_from_for_range!(Range);
impl_from_for_range!(RangeTo);
impl_from_for_range!(RangeFrom);
impl_from_for_range!(RangeInclusive);
impl_from_for_range!(RangeToInclusive);
