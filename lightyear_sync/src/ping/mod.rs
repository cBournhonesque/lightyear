//! Module to handle sending ping/pong messages and compute connection statistics (rtt, jitter, etc.)

pub mod manager;

pub mod message;

pub mod diagnostics;
pub mod store;
pub mod plugin;


/// Default channel to send pings. This is a Sequenced Unreliable channel, because
/// there is no point in getting older pings.
pub struct PingChannel;