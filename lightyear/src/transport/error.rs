pub type Result<T> = std::result::Result<T, Error>;
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("transport is not connected. Did you call connect()?")]
    NotConnected,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    WebTransport(#[from] wtransport::error::ConnectingError),
    #[error(transparent)]
    WebSocket(#[from] tokio_tungstenite::tungstenite::error::Error),
}
