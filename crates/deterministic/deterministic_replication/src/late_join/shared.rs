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
/// The request carries the latest server tick for which the client currently
/// has enough rebroadcast input to replay. The server buffers this readiness
/// signal until its authoritative tick has moved past `input_safe_tick`, then
/// accepts at its current authoritative tick and sends [`CatchUpSnapshotReady`]
/// as a replicated event carrying the accepted snapshot metadata.
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
    /// Returns true if the server-side catch-up visibility scope has been
    /// registered.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}

/// Extension trait for registering deterministic catch-up.
pub trait AppCatchUpExt {
    /// Register the deterministic catch-up component scope and input type.
    ///
    /// On server apps, `T` (typically a tuple of physics components, e.g.
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

/// Server-side marker inserted on a client's link entity once gated catch-up
/// state should be visible to that client.
#[derive(Component, Debug, Default)]
#[component(immutable)]
pub struct HasCaughtUp;

/// System sets for the late-join catch-up plugin.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub enum CatchUpSystems {
    /// Server-side: buffers and accepts [`CatchUpRequest`] messages.
    HandleRequests,
    /// Client-side: detect that we have received inputs from all clients so
    /// we can start the catchup process
    SendCatchUpRequest,
    /// Client-side: after we receive the catchup tick from the server, trigger a forced
    /// rollback to perform the catchup.
    TriggerCatchUpRollback,
}
