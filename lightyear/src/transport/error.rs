use no_std_io2::io as io;
#[cfg(not(feature = "std"))]
use {
    alloc::string::{String, ToString},
};
pub type Result<T> = core::result::Result<T, Error>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("transport is not connected. Did you call connect()?")]
    NotConnected,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
    #[error(transparent)]
    WebTransport(#[from] wtransport::error::ConnectingError),
    #[cfg(all(feature = "websocket", not(target_family = "wasm")))]
    #[error(transparent)]
    WebSocket(#[from] tokio_tungstenite::tungstenite::error::Error),
    #[error("could not send message via channel: {0}")]
    Channel(String),
    #[error("requested by user")]
    UserRequest,
    #[cfg(feature = "lz4")]
    #[error("lz4 compression error")]
    CompressError(#[from] lz4_flex::block::CompressError),
    #[cfg(feature = "lz4")]
    #[error("lz4 decompression error")]
    DecompressError(#[from] lz4_flex::block::DecompressError),
}

#[allow(unused_qualifications)]
impl<T> ::core::convert::From<async_channel::SendError<T>> for Error {
    #[allow(deprecated)]
    fn from(source: async_channel::SendError<T>) -> Self {
        Error::Channel(source.to_string())
    }
}

#[allow(unused_qualifications)]
impl<T> ::core::convert::From<async_channel::TrySendError<T>> for Error {
    #[allow(deprecated)]
    fn from(source: async_channel::TrySendError<T>) -> Self {
        Error::Channel(source.to_string())
    }
}

#[allow(unused_qualifications)]
impl<T> ::core::convert::From<crossbeam_channel::SendError<T>> for Error {
    #[allow(deprecated)]
    fn from(source: crossbeam_channel::SendError<T>) -> Self {
        Error::Channel(source.to_string())
    }
}

#[allow(unused_qualifications)]
impl<T> ::core::convert::From<tokio::sync::mpsc::error::SendError<T>> for Error {
    #[allow(deprecated)]
    fn from(source: tokio::sync::mpsc::error::SendError<T>) -> Self {
        Error::Channel(source.to_string())
    }
}
