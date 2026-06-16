use bevy_ecs::prelude::*;
use bevy_ecs::resource::Resource;
use bevy_replicon::prelude::{FilterScope, RepliconTick};
use lightyear_core::tick::Tick;
use lightyear_inputs::input_message::ActionStateSequence;
use serde::{Deserialize, Serialize};

/// Message sent from a client to the server requesting the bundled
/// one-time snapshot of catch-up-gated components for *every*
/// [`CatchUpGated`] entity.
///
/// The request carries the latest server tick for which the client has enough
/// rebroadcast input to replay. The server accepts only when its reveal tick
/// is no newer than this value, then sends [`CatchUpSnapshotReady`] as a
/// replicated event carrying the accepted snapshot metadata.
#[derive(Event, Serialize, Deserialize, Clone, Debug, Default)]
pub struct CatchUpRequest {
    pub input_safe_tick: Tick,
}

/// Observer event emitted when a catch-up snapshot is ready.
///
/// The server sends this as a replicated event when it accepts a catch-up
/// request. The client stores that metadata, waits for the matching Replicon
/// checkpoint to be confirmed, then triggers this same event locally. User
/// code should observe `On<CatchUpSnapshotReady>` and query [`CatchUpGated`]
/// entities to add application-specific local components before the forced
/// rollback runs.
#[derive(Event, Serialize, Deserialize, Clone, Debug)]
pub struct CatchUpSnapshotReady {
    /// The accepted Replicon checkpoint tick that revealed the bundled
    /// catch-up snapshot.
    pub replicon_tick: RepliconTick,
    /// The authoritative Lightyear simulation tick used as the forced rollback
    /// target.
    ///
    /// [`replicon_tick`]: Self::replicon_tick
    pub server_tick: Tick,
}

/// Tracks whether [`AppCatchUpExt::register_catchup`] has
/// registered the Replicon visibility filter for catch-up components.
#[derive(Resource, Default)]
pub struct CatchUpRegistry {
    pub(crate) initialized: bool,
}

impl CatchUpRegistry {
    /// Returns true if [`AppCatchUpExt::register_catchup`] has been called.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}

/// Extension trait for registering deterministic catch-up.
pub trait AppCatchUpExt {
    /// Register the deterministic catch-up component scope and input type.
    ///
    /// On servers, `T` (typically a tuple of physics components, e.g.
    /// `(Position, Rotation, LinearVelocity, AngularVelocity)`) becomes the
    /// Replicon visibility scope hidden behind [`CatchUpGated`].
    ///
    /// On clients, `S` contributes the input-buffer coverage check used to
    /// decide when the client can safely request a catch-up snapshot.
    ///
    /// The components in `T` must also be registered for replication
    /// separately (typically via `replicate_once::<C>()` and
    /// `add_rollback::<C>().add_confirmed_write()`).
    ///
    /// Calling this more than once is a no-op for the server visibility scope.
    fn register_catchup<T, S>(&mut self) -> &mut Self
    where
        T: CatchUpComponentScope + Send + Sync + 'static,
        S: ActionStateSequence;
}

#[doc(hidden)]
pub trait CatchUpComponentScope: FilterScope {}

impl<T: FilterScope> CatchUpComponentScope for T {}

/// Marker component added by server-side user code to entities whose
/// catch-up-gated components should be hidden from clients until the client
/// has completed the initial bundled catch-up snapshot.
///
/// On [`Add`], the registered visibility filter is inserted on the same
/// entity. Replicon hides the registered catch-up component scope from clients
/// that do not yet have [`HasCaughtUp`] on their client link entity.
///
/// In the deterministic_replication example this is inserted on the player
/// entity next to `Replicate::to_clients(NetworkTarget::All)`.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CatchUpGated;

/// Server-side marker inserted on a client's link entity once the client
/// has received at least one bundled catch-up snapshot.
#[derive(Component, Debug, Default)]
#[component(immutable)]
pub struct HasCaughtUp;

/// Re-export of [`lightyear_prediction::rollback::AwaitingCatchUpSnapshot`]
/// so user code can stay in the catch-up vocabulary.
///
/// This is a **per-entity marker component** (not a resource). The late-join
/// plugin inserts it on catch-up-gated client entities while they are
/// expecting the bundled snapshot, and removes it once the forced rollback is
/// scheduled.
///
/// [`crate::late_join::CatchUpManager`] tracks when this internal catch-up
/// state should suppress checksum computation.
///
pub use lightyear_prediction::rollback::AwaitingCatchUpSnapshot;

/// System sets for the late-join catch-up plugin.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub enum CatchUpSystems {
    /// Client-side: reset the accumulated per-input safe tick before
    /// registered input buffer checks run.
    ResetReadiness,
    /// Server-side: accepts safe [`CatchUpRequest`] messages.
    HandleRequests,
    /// Client-side: detect that the accepted reveal checkpoint is confirmed.
    DetectSnapshotReady,
    /// Client-side: internal registered-input checks that decide whether the
    /// snapshot can be replayed from its server tick.
    CheckClientReplayReadiness,
    /// Client-side: automatically request the forced rollback after
    /// [`CatchUpSnapshotReady`] observers have run.
    FinalizeSnapshot,
}
