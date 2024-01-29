/*!  A connection is an abstraction over an unreliable transport of a connection between a client and server
*/
pub(crate) mod client;
mod config;
pub mod netcode;

#[cfg(feature = "rivet")]
pub mod rivet;
pub(crate) mod server;
