//! Module to handle sending ping/pong messages and compute connection statistics (rtt, jitter, etc.)

pub mod manager;

pub mod message;

pub mod diagnostics;
pub mod store;
pub mod plugin;


pub struct PingChannel;