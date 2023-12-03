//! WebTransport protocol implementation in *pure* Rust, *async-friendly*, and *API-simple*.
//!
//! For a quick start with this library, refer to [`Endpoint`].
//!
//! ## About WebTransport
//! WebTransport is a modern protocol built on [QUIC](https://en.wikipedia.org/wiki/QUIC)
//! and [HTTP/3](https://en.wikipedia.org/wiki/HTTP/3), providing an alternative to
//! HTTP and WebSocket.
//!
//! It's designed for efficient client-server communication with *low latency* and
//! bi-directional *multistream* data exchange capabilities, making it suitable for a wide range of
//! applications.
//!
//! WebTransport guarantees *secure* and *reliable* communication by leveraging encryption
//! and authentication to protect your data during transmission.
//!
//! WebTransport offers two key communication channels: *streams* and *datagrams*.
//!
//! ### Streams
//! WebTransport streams are communication channels that provide *ordered* and
//! *reliable* data transfer.
//!
//! WebTransport streams allow sending multiple sets of data at once within a single session.
//! Each stream operates independently, ensuring that the order and reliability
//! of one stream do not affect the others.
//!
//! Streams can be: *uni-directional* or *bi-directional*.
//!
//! *Order Preserved, Guaranteed Delivery, Flow-Controlled, Secure (All Traffic Encrypted),
//! and Multiple Independent Streams*.
//!
//! ## Datagrams
//! WebTransport datagrams are lightweight and *unordered* communication channels,
//! prioritizing quick data exchange without guarantees of reliability or sequence.
//!
//! *Unordered, No Guaranteed Delivery, No Flow-Controlled, Secure (All Traffic Encrypted),
//! Independent Messages*.
//!
//!
//! # Examples
//! Explore operational server and client examples below. The elegantly simple yet potent
//! API empowers you to get started with minimal code.
//!
//! ## Server
//! ```no_run
//! # use anyhow::Result;
//! use wtransport::Certificate;
//! use wtransport::Endpoint;
//! use wtransport::ServerConfig;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let config = ServerConfig::builder()
//!         .with_bind_default(4433)
//!         .with_certificate(Certificate::load("cert.pem", "key.pem").await?)
//!         .build();
//!
//!     let server = Endpoint::server(config)?;
//!
//!     loop {
//!         let incoming_session = server.accept().await;
//!         let incoming_request = incoming_session.await?;
//!         let connection = incoming_request.accept().await?;
//!         // ...
//!     }
//! }
//! ```
//! See [repository server example](https://github.com/BiagioFesta/wtransport/blob/master/wtransport/examples/server.rs)
//! for the complete code.
//!
//! ## Client
//! ```no_run
//! # use anyhow::Result;
//! use wtransport::ClientConfig;
//! use wtransport::Endpoint;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let connection = Endpoint::client(ClientConfig::default())?
//!         .connect("https://localhost:4433")
//!         .await?;
//!     // ...
//!   # Ok(())
//! }
//! ```
//! See [repository client example](https://github.com/BiagioFesta/wtransport/blob/master/wtransport/examples/client.rs)
//! for the complete code.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs, clippy::doc_markdown)]

/// Client and server configurations.
pub mod config;

/// WebTransport connection.
pub mod connection;

/// Endpoint module.
pub mod endpoint;

/// Errors definitions module.
pub mod error;

/// Interfaces for sending and receiving data.
pub mod stream;

/// TLS specific configurations.
pub mod tls;

/// Datagrams module.
pub mod datagram;

#[doc(inline)]
pub use config::ClientConfig;

#[doc(inline)]
pub use config::ServerConfig;

#[doc(inline)]
pub use tls::Certificate;

#[doc(inline)]
pub use endpoint::Endpoint;

#[doc(inline)]
pub use connection::Connection;

#[doc(inline)]
pub use stream::RecvStream;

#[doc(inline)]
pub use stream::SendStream;

#[doc(inline)]
#[cfg(feature = "quinn")]
#[cfg_attr(docsrs, doc(cfg(feature = "quinn")))]
pub use quinn;

mod driver;
