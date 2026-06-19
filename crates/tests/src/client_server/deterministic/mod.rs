//! Integration tests for deterministic replication.
//!
//! These tests exercise the same code paths as
//! `examples/deterministic_replication`, in three scenarios:
//! - [`state_based_catchup`]: both clients connect, server delays the
//!   bundled catch-up snapshot until it has received inputs from every
//!   client, then clients reconcile via one forced rollback.
//! - [`late_join`]: client 1 is already simulating + moving when client 2
//!   joins; client 2 catches up via the bundled snapshot mechanism.
//! - [`input_only`]: both clients connect, inputs are broadcast, both start
//!   moving in sync. No state snapshot — pure input replication. This mode
//!   only works when every peer has the same entities at the same spawn
//!   tick (e.g. entities spawned at startup on all peers, no server-side
//!   spawning on connect).

mod input_only;
mod protocol;
mod state_based_catchup;
mod stepper;

