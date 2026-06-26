use alloc::vec::Vec;
use bevy_ecs::prelude::*;
use bevy_replicon::prelude::{FilterScope, RepliconTick, SingleComponent};
use core::any::TypeId;
use lightyear_core::tick::Tick;
use lightyear_inputs::input_message::ActionStateSequence;
use serde::{Deserialize, Serialize};

/// Message sent from a client to the server requesting the bundled
/// one-time snapshot of catch-up-gated components for *every*
/// `CatchUpGated` entity.
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
/// checkpoint to be confirmed, requests the forced rollback, and then triggers
/// this same event locally after rollback preparation has restored the snapshot
/// components but before rollback replay begins. If the server sends
/// [`CatchUpSnapshotReady::not_required`], the client skips the forced rollback
/// and triggers this event locally before removing `CatchUpGated`. User code
/// should observe `On<CatchUpSnapshotReady>` and query `CatchUpGated` entities
/// to add application-specific local components before replay or skip
/// completes.
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

impl CatchUpSnapshotReady {
    /// Sentinel used by the server to tell a client that no catch-up snapshot
    /// is required. Both ticks are set to `u32::MAX` so the client can
    /// distinguish this from an accepted authoritative snapshot.
    pub fn not_required() -> Self {
        Self {
            replicon_tick: RepliconTick::new(u32::MAX),
            server_tick: Tick(u32::MAX),
        }
    }

    /// Returns true when this event is the server-authoritative
    /// "catch-up not required" sentinel.
    pub fn is_not_required(&self) -> bool {
        self.replicon_tick.get() == u32::MAX && self.server_tick == Tick(u32::MAX)
    }
}

/// Tracks which Replicon visibility filters were registered by
/// [`AppCatchUpExt`].
#[derive(Resource, Default)]
pub struct CatchUpRegistry {
    pub(crate) registered_filters: Vec<TypeId>,
}

impl CatchUpRegistry {
    /// Returns true if any server-side catch-up visibility scope has been
    /// registered.
    pub fn is_initialized(&self) -> bool {
        !self.registered_filters.is_empty()
    }

    pub(crate) fn register_filter<F: 'static>(&mut self) -> bool {
        let id = TypeId::of::<F>();
        if self.registered_filters.contains(&id) {
            return false;
        }
        self.registered_filters.push(id);
        true
    }
}

/// Extension trait for registering deterministic catch-up.
pub trait AppCatchUpExt {
    /// Register a single deterministic catch-up component/resource and input
    /// type.
    ///
    /// On server apps, `C` is registered as Replicon's
    /// [`SingleComponent<C>`] visibility scope hidden behind `CatchUpGated`.
    /// Since Bevy resources are components stored on resource entities, the
    /// same API applies to resources.
    ///
    /// On clients, `S` contributes the input-buffer coverage check used to
    /// decide when the client can safely request a catch-up snapshot.
    ///
    /// The component/resource must also be registered for replication
    /// separately.
    ///
    /// Calling this more than once for the same scope is a no-op.
    fn register_catchup<C, S>(&mut self) -> &mut Self
    where
        C: Component,
        SingleComponent<C>: FilterScope + Send + Sync + 'static,
        S: ActionStateSequence;

    /// Register an arbitrary Replicon catch-up filter scope and input type.
    ///
    /// Use this for tuple scopes such as
    /// `(Position, Rotation, LinearVelocity, AngularVelocity)`. For a single
    /// component/resource, prefer [`AppCatchUpExt::register_catchup`].
    fn register_catchup_filter<T, S>(&mut self) -> &mut Self
    where
        T: FilterScope + Send + Sync + 'static,
        S: ActionStateSequence;
}

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
    /// Client-side: after rollback preparation has restored the snapshot
    /// components, activate local-only application state before replay begins.
    ActivateCatchUp,
}
