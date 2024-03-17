/*! Modules related to the client
*/

pub mod components;

pub mod config;

pub mod connection;

pub mod events;

pub mod input;

pub mod interpolation;

pub mod plugin;

pub mod prediction;

pub mod sync;

mod diagnostics;
mod easings;
#[cfg_attr(docsrs, doc(cfg(feature = "leafwing")))]
#[cfg(feature = "leafwing")]
pub mod input_leafwing;
pub(crate) mod message;
pub(crate) mod metadata;
pub(crate) mod networking;
pub mod replication;
