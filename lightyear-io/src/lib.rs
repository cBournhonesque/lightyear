/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod error {
    use thiserror::Error;

    /// Errors that can occur with IO operations
    #[derive(Debug, Error)]
    pub enum IoError {
        #[error("Failed to send data: {0}")]
        SendError(String),
        
        #[error("Failed to receive data: {0}")]
        ReceiveError(String),
        
        #[error("Connection error: {0}")]
        ConnectionError(String),
    }
}

pub mod transport {
    //! Provides an abstraction over an unreliable transport
    use alloc::vec::Vec;
    use bytes::Bytes;
    
    use crate::error::IoError;

    /// A transport is a way to send and receive raw bytes over the network.
    pub trait Transport {
        /// Send raw bytes over the network.
        fn send_bytes(&mut self, bytes: Bytes) -> Result<(), IoError>;
        
        /// Receive raw bytes from the network.
        fn receive_bytes(&mut self) -> Result<Vec<Bytes>, IoError>;
        
        /// Check if the transport is connected.
        fn is_connected(&self) -> bool;
    }
}

pub mod middleware {
    //! Middleware for transforming data before sending or after receiving
    
    pub mod compression {
        //! Compression middleware
        
        /// Configuration for compression middleware
        #[derive(Debug, Clone)]
        pub struct CompressionConfig {
            /// Whether compression is enabled
            pub enabled: bool,
            /// The compression level (0-9)
            pub level: u32,
        }
        
        impl Default for CompressionConfig {
            fn default() -> Self {
                Self {
                    enabled: true,
                    level: 3,
                }
            }
        }
    }
    
    pub mod conditioner {
        //! Network conditioner for simulating network conditions
        
        /// Configuration for the link conditioner
        #[derive(Debug, Clone)]
        pub struct LinkConditionerConfig {
            /// Whether the conditioner is enabled
            pub enabled: bool,
            /// Simulated latency in milliseconds
            pub latency_ms: u64,
            /// Jitter in milliseconds
            pub jitter_ms: u64,
            /// Packet loss percentage (0-100)
            pub packet_loss: f32,
        }
        
        impl Default for LinkConditionerConfig {
            fn default() -> Self {
                Self {
                    enabled: false,
                    latency_ms: 0,
                    jitter_ms: 0,
                    packet_loss: 0.0,
                }
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
