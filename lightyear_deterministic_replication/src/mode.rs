//! Catch-up mode for deterministic replication.
//!
//! Declares whether the application uses state-based catch-up (the server
//! sends a one-shot snapshot of all [`CatchUpGated`] entities when a client
//! requests it) or relies solely on input replication.
//!
//! [`CatchUpGated`]: crate::late_join::CatchUpGated

use bevy_ecs::resource::Resource;

/// Catch-up mode for the deterministic replication plugin.
///
/// - [`CatchUpMode::InputOnly`]: no state snapshot. The simulation is driven
///   purely by replicated inputs. A client that joins mid-game will only be
///   able to simulate forward from the point its inputs start flowing; the
///   `DeterministicPredicted::skip_despawn` / `DisableRollback` mechanism is
///   required to prevent rollbacks from disturbing entities before the local
///   peer knows about them.
///
/// - [`CatchUpMode::StateBasedCatchUp`]: on [`CatchUpRequest`] the server
///   sends a single coherent snapshot of every [`CatchUpGated`] entity at a
///   single tick. The client seeds `PredictionHistory` from that snapshot
///   and fires a single forced rollback to reconcile forward. In this mode
///   the `skip_despawn` / `DisableRollback` guard is redundant for
///   catch-up-gated entities — the snapshot carries authoritative state.
///
/// This is a configuration resource consulted by
/// [`LateJoinCatchUpPlugin`] and by user code that needs to branch on
/// the mode (e.g. to decide whether to send a `CatchUpRequest` at
/// connection time).
///
/// [`CatchUpRequest`]: crate::late_join::CatchUpRequest
/// [`CatchUpGated`]: crate::late_join::CatchUpGated
/// [`LateJoinCatchUpPlugin`]: crate::late_join::LateJoinCatchUpPlugin
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CatchUpMode {
    /// No state snapshot; inputs only.
    InputOnly,
    /// State-based catch-up via a single bundled snapshot per client.
    #[default]
    StateBasedCatchUp,
}
