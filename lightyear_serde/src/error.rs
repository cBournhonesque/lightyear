use std::fmt::Display;

/// The error message when failing to serialize/deserialize to/from the bit stream.
#[derive(Clone)]
pub struct Error;

pub type Result<T> = std::result::Result<T, Error>;

impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bin deserialize error",)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}

impl ::serde::ser::Error for Error {
    fn custom<T>(msg: T) -> Self where T: Display {
        Error
    }
}

impl ::serde::de::Error for Error {
    fn custom<T>(msg: T) -> Self where T: Display {
        Error
    }
}

impl std::error::Error for Error {}
