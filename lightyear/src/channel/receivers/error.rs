//! Errors for receiving packets

pub type Result<T> = core::result::Result<T, ChannelReceiveError>;
#[derive(thiserror::Error, Debug)]
pub enum ChannelReceiveError {
    #[error("A message was received without a message ID")]
    MissingMessageId,
}
