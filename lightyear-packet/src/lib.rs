/*! # Lightyear Packet

Packet handling for the lightyear networking library.
This crate provides abstractions for working with packets, channels, and reliability.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod error {
    use thiserror::Error;
    
    /// Errors that can occur with packet operations
    #[derive(Debug, Error)]
    pub enum PacketError {
        #[error("Serialization error: {0}")]
        SerializationError(String),
        
        #[error("Channel error: {0}")]
        ChannelError(String),
        
        #[error("IO error: {0}")]
        IoError(#[from] lightyear_io::error::IoError),
    }
}

pub mod message {
    use serde::{Deserialize, Serialize};
    
    /// A trait for messages that can be sent over the network
    pub trait Message: Serialize + for<'de> Deserialize<'de> + Send + Sync + 'static {}
    impl<T> Message for T where T: Serialize + for<'de> Deserialize<'de> + Send + Sync + 'static {}
}

pub mod channel {
    pub mod builder {
        //! Channel building

        /// Direction of a channel
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum ChannelDirection {
            /// Channel from client to server
            Upstream,
            /// Channel from server to client
            Downstream,
            /// Channel in both directions
            Bidirectional,
        }
        
        /// Mode of a channel
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum ChannelMode {
            /// Unreliable channel
            Unreliable,
            /// Reliable channel
            Reliable,
            /// Reliable ordered channel
            ReliableOrdered,
        }
        
        /// Settings for reliable channels
        #[derive(Debug, Clone)]
        pub struct ReliableSettings {
            /// Maximum number of resend attempts
            pub resend_attempts: u32,
        }
        
        impl Default for ReliableSettings {
            fn default() -> Self {
                Self {
                    resend_attempts: 10,
                }
            }
        }
        
        /// Settings for a channel
        #[derive(Debug, Clone)]
        pub struct ChannelSettings {
            /// Mode of the channel
            pub mode: ChannelMode,
            /// Direction of the channel
            pub direction: ChannelDirection,
            /// Settings for reliable channels
            pub reliable_settings: Option<ReliableSettings>,
        }
        
        /// Marker trait for channel types
        pub trait Channel: Send + Sync + 'static {}
        
        /// Marker trait for input channels
        pub trait InputChannel: Channel {}
        
        /// Builder for creating channels
        #[derive(Debug, Clone)]
        pub struct ChannelBuilder {
            settings: ChannelSettings,
        }
        
        impl ChannelBuilder {
            /// Create a new channel builder
            pub fn new(mode: ChannelMode, direction: ChannelDirection) -> Self {
                Self {
                    settings: ChannelSettings {
                        mode,
                        direction,
                        reliable_settings: None,
                    }
                }
            }
            
            /// Set reliable settings
            pub fn with_reliability(mut self, settings: ReliableSettings) -> Self {
                self.settings.reliable_settings = Some(settings);
                self
            }
            
            /// Build into a channel container
            pub fn build<C: Channel>(self) -> ChannelContainer<C> {
                ChannelContainer {
                    _marker: core::marker::PhantomData,
                    settings: self.settings,
                }
            }
        }
        
        /// Container for channel settings
        #[derive(Debug, Clone)]
        pub struct ChannelContainer<C: Channel> {
            _marker: core::marker::PhantomData<C>,
            settings: ChannelSettings,
        }
        
        impl<C: Channel> ChannelContainer<C> {
            /// Get channel settings
            pub fn settings(&self) -> &ChannelSettings {
                &self.settings
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
