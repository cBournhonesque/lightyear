#![allow(clippy::all)]
// #![cfg_attr(test, feature(test))]
// #![cfg_attr(doc, feature(doc_cfg))]
#![forbid(unsafe_code)]
#![allow(clippy::items_after_test_module)]

//! Bitcode is a crate for encoding and decoding using a tinier
//! binary serialization strategy. You can easily go from having
//! an object in memory, quickly serialize it to bytes, and then
//! deserialize it back just as fast!
//!
//! The format is not necessarily stable between versions. If you want
//! a stable format, consider [bincode](https://docs.rs/bincode/latest/bincode/).
//!
//! ### Usage
//!
//! ```edition2021
//! // The object that we will encode.
//! let target: Vec<String> = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
//!
//! let encoded: Vec<u8> = bitcode::encode(&target).unwrap();
//! let decoded: Vec<String> = bitcode::decode(&encoded).unwrap();
//! assert_eq!(target, decoded);
//! ```
//!
//! ### Advanced Usage
//!
//! Bitcode has several hints that can be applied.
//! Hints may have an effect on the encoded length.
//! Most importantly hints will never cause errors if they don't hold true.
//!
//! ```edition2021
//! // We mark enum variants that are more likely with a higher frequency.
//! // This allows bitcode to use shorter encodings for them.
//! #[derive(Copy, Clone, bitcode::Encode, bitcode::Decode)]
//! enum Fruit {
//!     #[bitcode_hint(frequency = 10)]
//!     Apple,
//!     #[bitcode_hint(frequency = 5)]
//!     Banana,
//!     // Unspecified frequencies are 1.
//!     Blueberry,
//!     Lime,
//!     Lychee,
//!     Watermelon,
//! }
//!
//! // A cart full of 16 apples takes 2 bytes to encode (1 bit per Apple).
//! let apple_cart: usize = bitcode::encode(&[Fruit::Apple; 16]).unwrap().len();
//! assert_eq!(apple_cart, 2);
//!
//! // A cart full of 16 bananas takes 4 bytes to encode (2 bits per Banana).
//! let banana_cart: usize = bitcode::encode(&[Fruit::Banana; 16]).unwrap().len();
//! assert_eq!(banana_cart, 4);
//!
//! // A cart full of 16 blueberries takes 8 bytes to encode (4 bits per Blueberry).
//! let blueberry_cart: usize = bitcode::encode(&[Fruit::Blueberry; 16]).unwrap().len();
//! assert_eq!(blueberry_cart, 8);
//! ```
//!
//! ```edition2021
//! // We expect most user ages to be in the interval [10, 100), so we specify that as the expected
//! // range. If we're right most of the time, users will take fewer bits to encode.
//! #[derive(bitcode::Encode, bitcode::Decode)]
//! struct User {
//!     #[bitcode_hint(expected_range = "10..100")]
//!     age: u32
//! }
//!
//! // A user with an age inside the expected range takes up to a byte to encode.
//! let expected_age: usize = bitcode::encode(&User { age: 42 }).unwrap().len();
//! assert_eq!(expected_age, 1);
//!
//! // A user with an age outside the expected range takes more than 4 bytes to encode.
//! let unexpected_age: usize = bitcode::encode(&User { age: 31415926 }).unwrap().len();
//! assert!(unexpected_age > 4);
//! ```
//!
//! ```edition2021
//! // We expect that most posts won't have that many views or likes, but some can. By using gamma
//! // encoding, posts with fewer views/likes will take fewer bits to encode.
//! #[derive(bitcode::Encode, bitcode::Decode)]
//! #[bitcode_hint(gamma)]
//! struct Post {
//!     views: u64,
//!     likes: u64,
//! }
//!
//! // An average post just takes 1 byte to encode.
//! let average_post = bitcode::encode(&Post {
//!     views: 4,
//!     likes: 1,
//! }).unwrap().len();
//! assert_eq!(average_post, 1);
//!
//! // A popular post takes 11 bytes to encode, luckily these posts are rare.
//! let popular_post = bitcode::encode(&Post {
//!     views: 27182818,
//!     likes: 161803,
//! }).unwrap().len();
//! assert_eq!(popular_post, 11)
//! ```

// https://doc.rust-lang.org/beta/unstable-book/library-features/test.html
#[cfg(test)]
extern crate test;

// Fixes derive macro in tests/doc tests.
#[cfg(test)]
extern crate self as bitcode;

pub use buffer::Buffer;
pub use code::{Decode, Encode};
use std::fmt::{self, Display, Formatter};

#[cfg(feature = "derive")]
pub use bitcode_derive::{Decode, Encode};

#[cfg(any(test, feature = "serde"))]
pub use crate::serde::{deserialize, serialize};

pub mod buffer;
mod code;
mod code_impls;
pub mod encoding;
mod guard;
mod nightly;
pub mod read;
mod register_buffer;
pub mod word;
pub mod word_buffer;
pub mod write;

#[doc(hidden)]
pub mod __private;

#[cfg(any(test, feature = "serde"))]
pub mod serde;

#[cfg(all(test, not(miri)))]
mod benches;
#[cfg(test)]
mod bit_buffer;
#[cfg(all(test, debug_assertions))]
mod tests;

/// Encodes a `T:` [`Encode`] into a [`Vec<u8>`].
///
/// Won't ever return `Err` unless using `#[bitcode(with_serde)]`.
///
/// **Warning:** The format is subject to change between versions.
pub fn encode<T: ?Sized>(t: &T) -> Result<Vec<u8>>
where
    T: Encode,
{
    Ok(Buffer::new().encode(t)?.to_vec())
}

/// Decodes a [`&[u8]`][`prim@slice`] into an instance of `T:` [`Decode`].
///
/// **Warning:** The format is subject to change between versions.
pub fn decode<T>(bytes: &[u8]) -> Result<T>
where
    T: Decode,
{
    Buffer::new().decode(bytes)
}

impl Buffer {
    /// Encodes a `T:` [`Encode`] into a [`&[u8]`][`prim@slice`]. Can reuse the buffer's
    /// allocations.
    ///
    /// Won't ever return `Err` unless using `#[bitcode(with_serde)]`.
    ///
    /// Even if you call `to_vec` on the [`&[u8]`][`prim@slice`], it's still more efficient than
    /// [`encode`].
    ///
    /// **Warning:** The format is subject to change between versions.
    pub fn encode<T: ?Sized>(&mut self, t: &T) -> Result<&[u8]>
    where
        T: Encode,
    {
        code::encode_internal(&mut self.0, t)
    }

    /// Decodes a [`&[u8]`][`prim@slice`] into an instance of `T:` [`Decode`]. Can reuse
    /// the buffer's allocations.
    ///
    /// **Warning:** The format is subject to change between versions.
    pub fn decode<T>(&mut self, bytes: &[u8]) -> Result<T>
    where
        T: Decode,
    {
        code::decode_internal(&mut self.0, bytes)
    }
}

/// Decoding / (De)serialization errors.
///
/// # Debug mode
///
/// In debug mode, the error contains a reason.
///
/// # Release mode
///
/// In release mode, the error is a zero-sized type for efficiency.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct Error(ErrorImpl);

#[cfg(not(debug_assertions))]
type ErrorImpl = ();

#[cfg(debug_assertions)]
type ErrorImpl = E;

impl Error {
    /// Replaces an invalid message. E.g. read_variant_index calls read_len but converts
    /// `E::Invalid("length")` to `E::Invalid("variant index")`.
    #[cfg(any(test, feature = "serde"))]
    pub(crate) fn map_invalid(self, _s: &'static str) -> Self {
        #[cfg(debug_assertions)]
        return Self(match self.0 {
            E::Invalid(_) => E::Invalid(_s),
            _ => self.0,
        });
        #[cfg(not(debug_assertions))]
        self
    }

    // Doesn't implement PartialEq because that would be part of the public api.
    pub(crate) fn same(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

/// Inner error that can be converted to [`Error`] with [`E::e`].
#[derive(Debug, PartialEq)]
pub(crate) enum E {
    #[allow(unused)] // Only used by serde feature.
    Custom(String),
    Eof,
    ExpectedEof,
    Invalid(&'static str),
    #[allow(unused)] // Only used by serde feature.
    NotSupported(&'static str),
}

impl E {
    fn e(self) -> Error {
        #[cfg(debug_assertions)]
        return Error(self);
        #[cfg(not(debug_assertions))]
        Error(())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        #[cfg(debug_assertions)]
        return Display::fmt(&self.0, f);
        #[cfg(not(debug_assertions))]
        f.write_str("bitcode error")
    }
}

impl Display for E {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Custom(s) => write!(f, "custom: {s}"),
            Self::Eof => write!(f, "eof"),
            Self::ExpectedEof => write!(f, "expected eof"),
            Self::Invalid(s) => write!(f, "invalid {s}"),
            Self::NotSupported(s) => write!(f, "{s} is not supported"),
        }
    }
}

impl std::error::Error for Error {}
