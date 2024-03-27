/*!  A connection is an abstraction over an unreliable transport of a connection between a client and server
*/
pub(crate) mod client;
pub mod netcode;

pub(crate) mod server;

#[cfg_attr(docsrs, doc(cfg(all(feature = "steam", not(target_family = "wasm")))))]
#[cfg(all(feature = "steam", not(target_family = "wasm")))]
pub(crate) mod steam;
