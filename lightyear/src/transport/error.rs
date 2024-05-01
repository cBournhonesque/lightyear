pub type Result<T> = std::result::Result<T, Error>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("transport is not connected. Did you call connect()?")]
    NotConnected,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
    #[error(transparent)]
    WebTransport(#[from] wtransport::error::ConnectingError),
    #[cfg(all(feature = "websocket", not(target_family = "wasm")))]
    #[error(transparent)]
    WebSocket(#[from] tokio_tungstenite::tungstenite::error::Error),
}
