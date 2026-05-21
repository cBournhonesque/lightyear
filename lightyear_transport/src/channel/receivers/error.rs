//! Errors for receiving packets

pub type Result<T> = core::result::Result<T, ChannelReceiveError>;

/// Errors produced by channel receiver implementations.
#[derive(thiserror::Error, Debug)]
pub enum ChannelReceiveError {
    /// A receiver mode requiring message IDs received a message without one.
    #[error("A message was received without a message ID")]
    MissingMessageId,
}
