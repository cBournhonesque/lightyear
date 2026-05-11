//! Late-join catch-up for deterministic replication.
//!
//! # Problem
//!
//! In deterministic replication the server and all clients run the same
//! simulation driven only by inputs. For an already-connected client, the
//! initial state of an entity is established once (via `replicate_once` of
//! physics components at entity spawn) and then every peer simulates forward
//! from that point using inputs that are rebroadcast by the server.
//!
//! When a new client joins mid-game, that initial snapshot is already in the
//! past. Simply `replicate_once`-ing the *current* physics state would not
//! help because the new client does not yet have the remote inputs needed
//! to simulate forward from the snapshot tick.
//!
//! # Approach (client-driven, bundled)
//!
//! Information flows from the client — which actually knows what it has —
//! back to the server:
//!
//! 1. At join, the server replicates "structural" data for existing entities
//!    (markers like `PlayerId`, `DeterministicPredicted`) and starts
//!    rebroadcasting inputs for those entities. Physics components
//!    registered via [`AppCatchUpExt::register_catchup_components`] are
//!    **hidden by default** via a replicon per-component visibility filter
//!    until each client requests a catch-up snapshot.
//!
//! 2. On the client, user code marks replicated deterministic entities with
//!    [`AwaitingCatchUpSnapshot`]. Once the [`InputTimeline`] is synced, the
//!    plugin sends one bodyless [`CatchUpRequest`] for the current pending
//!    catch-up bundle and inserts [`CatchUpRequestSent`] to track the
//!    in-flight request.
//!
//! 3. On the server, the [`CatchUpRequest`] handler waits until the server
//!    reports that the snapshot tick is safe, then inserts [`HasCaughtUp`] on
//!    the client's link entity. Replicon's catch-up visibility filter observes
//!    that marker and reveals the hidden catch-up component scope for every
//!    [`CatchUpGated`] entity. Because the marker is inserted once in one
//!    frame, replicon emits a single bundled init message at a single server
//!    tick `S` containing the gated entities' state.
//!
//! 4. The client receives all catch-up components at server tick `S`.
//!    `add_confirmed_write` routes those writes into `PredictionHistory<C>`
//!    as confirmed entries at `S`. User code waits until all gated entities
//!    have their catch-up components present and then calls
//!    [`request_forced_rollback_to_catch_up_tick`] once. That schedules a
//!    single forced rollback from `S` — re-simulating deterministically
//!    forward to the current tick across *all* entities simultaneously.
//!
//! 5. [`HasCaughtUp`] is not removed. After the initial catch-up, the rest of
//!    the simulation is deterministic, so later catch-up-gated components are
//!    replicated normally to that client.
//!
//! # Why bundled, not per-entity
//!
//! A per-entity catch-up (one [`CatchUpRequest`] per gated entity, each
//! producing its own snapshot at its own tick) causes divergence. The first
//! forced rollback replays every entity, but entities that neither have a
//! snapshot nor valid local history at that tick are replayed from stale local
//! state. Bundling guarantees a single consistent snapshot tick `S` for the
//! pending catch-up entities.
//!
//! # Why per-component visibility, not entity-level
//!
//! If we hid the whole entity, the client would not know the entity
//! exists and could not send the catch-up request (nor receive rebroadcast
//! inputs). By keeping the entity visible but hiding only the physics
//! components, the client sees the entity + marker components + rebroadcast
//! inputs and can request the bundled snapshot.

use alloc::vec::Vec;
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::*;
use bevy_ecs::resource::Resource;
use bevy_ecs::system::Commands;
use bevy_replicon::prelude::RepliconTick;
use bevy_replicon::prelude::{AppVisibilityExt, FilterScope, VisibilityFilter};
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use core::marker::PhantomData;
use lightyear_connection::client::{Client, Connected};
use lightyear_connection::client_of::ClientOf;
use lightyear_connection::direction::NetworkDirection;
use lightyear_core::tick::Tick;
use lightyear_inputs::server::InputSystems;
use lightyear_link::server::LinkOf;
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::{AppMessageExt, MessageSender};
use lightyear_messages::receive::MessageReceiver;
use lightyear_prediction::prelude::{PredictionSystems, StateRollbackMetadata};
use lightyear_replication::checkpoint::ReplicationCheckpointMap;
use lightyear_replication::prelude::{ConfirmHistory, ReplicationSystems};
use lightyear_sync::prelude::{InputTimeline, IsSynced};
use lightyear_transport::prelude::Channel;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::mode::CatchUpMode;

/// Message sent once from a client to the server requesting the bundled
/// one-time snapshot of catch-up-gated components for *every*
/// [`CatchUpGated`] entity.
///
/// Bodyless — the server decides which entities to include based on which
/// entities carry [`CatchUpGated`] at the time the request is received.
/// This guarantees that all catch-up-gated state arrives in a single
/// coherent replicon update at a single server tick, which is a
/// prerequisite for a single forced rollback to reconcile everything at
/// once.
#[derive(Event, Serialize, Deserialize, Clone, Debug, Default)]
pub struct CatchUpRequest;

/// Client-side event emitted when the bundled catch-up snapshot has landed.
///
/// The plugin emits this in [`CatchUpSystems::DetectSnapshotReady`], which is
/// scheduled after [`ReplicationSystems::Receive`] and before
/// [`PredictionSystems::Rollback`]. User code can react in
/// [`CatchUpSystems::OnSnapshotReady`] to add application-specific components
/// (for example physics bundles) before the forced rollback runs.
#[derive(Message, Clone, Debug)]
pub struct CatchUpSnapshotReady {
    /// Any entity from the coherent catch-up bundle. This can be passed to
    /// [`request_forced_rollback_to_catch_up_tick`] once user code has added
    /// the components needed for rollback replay.
    pub reference_entity: Entity,
    /// The Replicon checkpoint tick shared by every awaiting entity in this
    /// bundled snapshot.
    pub replicon_tick: RepliconTick,
    /// The authoritative Lightyear simulation tick for [`replicon_tick`].
    ///
    /// [`replicon_tick`]: Self::replicon_tick
    pub server_tick: Tick,
    /// The awaiting entities that share this bundled snapshot tick.
    pub entities: Vec<Entity>,
}

/// Tracks whether [`AppCatchUpExt::register_catchup_components`] has
/// registered the Replicon visibility filter for catch-up components.
#[derive(Resource, Default)]
pub struct CatchUpRegistry {
    initialized: bool,
}

impl CatchUpRegistry {
    /// Returns true if [`AppCatchUpExt::register_catchup_components`] has
    /// been called.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}

/// Extension trait for registering the catch-up visibility scope.
pub trait AppCatchUpExt {
    /// Register `T` (typically a tuple of physics components, e.g.
    /// `(Position, Rotation, LinearVelocity, AngularVelocity)`) as the
    /// catch-up scope.
    ///
    /// Registers a single Replicon [`VisibilityFilter`] covering all
    /// components in `T` at once.
    ///
    /// Calling this more than once is a no-op — the plugin supports a
    /// single catch-up scope.
    ///
    /// The components in `T` must also be registered for replication
    /// separately (typically via `replicate_once::<C>()` and
    /// `add_rollback::<C>().add_confirmed_write()`).
    fn register_catchup_components<T: FilterScope + Send + Sync + 'static>(&mut self) -> &mut Self;
}

impl AppCatchUpExt for App {
    fn register_catchup_components<T: FilterScope + Send + Sync + 'static>(&mut self) -> &mut Self {
        if self.world().resource::<CatchUpRegistry>().is_initialized() {
            return self;
        }
        self.add_visibility_filter::<CatchUpVisibility<T>>();
        self.add_observer(on_catch_up_gated_added::<T>);
        self.add_observer(on_has_caught_up_added::<T>);
        self.world_mut()
            .resource_mut::<CatchUpRegistry>()
            .initialized = true;

        let gated: Vec<Entity> = {
            let world = self.world_mut();
            let mut query = world.query_filtered::<Entity, With<CatchUpGated>>();
            query.iter(world).collect()
        };
        for entity in gated {
            self.world_mut()
                .entity_mut(entity)
                .insert(CatchUpVisibility::<T>::default());
        }
        let caught_up_clients: Vec<Entity> = {
            let world = self.world_mut();
            let mut query = world.query_filtered::<Entity, With<HasCaughtUp>>();
            query.iter(world).collect()
        };
        for entity in caught_up_clients {
            self.world_mut()
                .entity_mut(entity)
                .insert(CatchUpVisibility::<T>::default());
        }
        self
    }
}

#[derive(Component)]
#[component(immutable)]
struct CatchUpVisibility<T: FilterScope + Send + Sync + 'static>(PhantomData<fn() -> T>);

impl<T: FilterScope + Send + Sync + 'static> Default for CatchUpVisibility<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T: FilterScope + Send + Sync + 'static> VisibilityFilter for CatchUpVisibility<T> {
    type ClientComponent = Self;
    type Scope = T;

    fn is_visible(&self, _client: Entity, component: Option<&Self::ClientComponent>) -> bool {
        component.is_some()
    }
}

/// Marker component added by server-side user code to entities whose
/// catch-up-gated components should be hidden from clients until the client
/// has completed the initial bundled catch-up snapshot.
///
/// On [`Add`], the registered [`CatchUpVisibility`] filter is inserted on
/// the same entity. Replicon hides the registered catch-up component scope
/// from clients that do not yet have [`HasCaughtUp`] on their client link
/// entity.
///
/// In the deterministic_replication example this is inserted on the player
/// entity next to `Replicate::to_clients(NetworkTarget::All)`.
#[derive(Component, Default)]
pub struct CatchUpGated;

/// Server-side marker inserted on a client's link entity once the client
/// has received at least one bundled catch-up snapshot.
#[derive(Component, Debug, Default)]
#[component(immutable)]
pub struct HasCaughtUp;

/// Client-side marker inserted on the client entity once the bodyless
/// [`CatchUpRequest`] has been sent. Prevents duplicate requests while the
/// initial catch-up bundle is in flight; the marker is removed when a forced
/// rollback is scheduled.
#[derive(Component, Debug, Default)]
pub struct CatchUpRequestSent;

/// Server-side marker inserted on a `ClientOf` entity once a
/// [`CatchUpRequest`] has been received but not yet applied. The request
/// remains pending until the server is able to produce a coherent snapshot
/// — specifically until [`CatchUpServerReadiness::all_clients_ready`] is
/// `true`, which means the server has received input from every client up
/// through its current simulated tick.
///
/// User code (or a concrete-input-type convenience plugin) sets
/// `CatchUpServerReadiness::all_clients_ready` based on observing input
/// buffers of the actual action types used by the application. Keeping the
/// readiness signal outside the plugin avoids tying the plugin to a
/// specific input crate.
#[derive(Component, Debug, Default)]
pub struct CatchUpRequestReceived;

/// Server-side resource set by user code (or a convenience plugin)
/// signalling that it is now safe to apply pending [`CatchUpRequest`]s.
///
/// The correctness requirement: the server must only flip catch-up
/// visibility at a tick `T` such that for every client in the simulation,
/// the server has already received input up through tick `T`. Otherwise
/// the snapshot at `T` was computed using predicted/decayed input for some
/// clients, and the requesting client's rollback-replay from `T` forward
/// will diverge (because the client's own input buffer has the real
/// inputs, not the server's extrapolated ones).
///
/// The plugin cannot evaluate this condition generically — it doesn't know
/// the user's concrete input types. User code sets this resource based on
/// observing `InputBuffer<S::Snapshot, S::Action>::last_remote_tick` on
/// each client's input entities.
///
/// While `all_clients_ready == false`, any received [`CatchUpRequest`] is
/// kept pending on the `ClientOf` via [`CatchUpRequestReceived`]. When the
/// flag flips to `true`, pending requests are applied on the next frame by
/// inserting [`HasCaughtUp`] on the client link entity (replicon emits the
/// bundled snapshot at that server tick).
#[derive(Resource, Debug, Default)]
pub struct CatchUpServerReadiness {
    pub all_clients_ready: bool,
}

/// Client-side resource that gates when [`send_catchup_request`] is allowed
/// to send a [`CatchUpRequest`].
///
/// The late-join plugin can tell that the input timeline is synced, but it
/// cannot know whether an application's concrete input buffers have enough
/// real input history to replay from the eventual snapshot tick. Applications
/// that use state-based catch-up for input-driven deterministic simulations
/// can set this to `false` until their local and rebroadcast input buffers are
/// ready. The default is `true` to preserve the old behavior for users that
/// do not need an extra gate.
#[derive(Resource, Debug)]
pub struct CatchUpClientReadiness {
    pub can_request: bool,
}

impl Default for CatchUpClientReadiness {
    fn default() -> Self {
        Self { can_request: true }
    }
}

#[derive(Resource, Default)]
struct CatchUpSnapshotReadyState {
    last_emitted_replicon_tick: Option<RepliconTick>,
}

/// Re-export of [`lightyear_prediction::rollback::AwaitingCatchUpSnapshot`]
/// so user code can stay in the catch-up vocabulary.
///
/// This is a **per-entity marker component** (not a resource). User code
/// inserts it on catch-up-gated client entities while they are expecting
/// the bundled snapshot, and removes it via
/// [`request_forced_rollback_to_catch_up_tick`] once the forced rollback
/// is scheduled.
///
/// While this marker is present on *any* entity, [`ChecksumSendPlugin`]
/// skips checksum computation so the client doesn't send checksums for
/// state known to be stale.
///
/// [`ChecksumSendPlugin`]: crate::prelude::ChecksumSendPlugin
pub use lightyear_prediction::rollback::AwaitingCatchUpSnapshot;

/// Plugin that wires up the late-join catch-up machinery.
///
/// Clients send a bodyless [`CatchUpRequest`] once they're synced and have at
/// least one entity marked [`AwaitingCatchUpSnapshot`]. Once server readiness
/// is met, the server inserts [`HasCaughtUp`] on the client entity. Replicon
/// then sends the catch-up-gated components as a single bundled init message
/// at a single server tick, which the client uses to drive a forced rollback.
/// The [`HasCaughtUp`] marker is kept after success.
pub struct LateJoinCatchUpPlugin<C: Channel> {
    _marker: core::marker::PhantomData<fn() -> C>,
}

impl<C: Channel> Default for LateJoinCatchUpPlugin<C> {
    fn default() -> Self {
        Self {
            _marker: core::marker::PhantomData,
        }
    }
}

/// System sets for the late-join catch-up plugin.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub enum CatchUpSystems {
    /// Server-side: schedule your per-input-type readiness system in this set
    /// so it runs before [`apply_pending_catch_ups`] and correctly gates when
    /// the bundled snapshot is sent to the requesting client. The readiness
    /// system should update [`CatchUpServerReadiness::all_clients_ready`]
    /// based on whether every player's `InputBuffer::last_remote_tick >=
    /// server_current_tick`.
    UpdateReadiness,
    /// Server-side: drains [`CatchUpRequest`] messages into
    /// [`CatchUpRequestReceived`] markers.
    HandleRequests,
    /// Server-side: if readiness is met, apply pending catch-ups by flipping
    /// catch-up visibility on the client's pending [`CatchUpGated`] entities.
    ApplyPending,
    /// Client-side: detect that a coherent bundled snapshot has landed.
    DetectSnapshotReady,
    /// Client-side: user hook for reacting to [`CatchUpSnapshotReady`] before
    /// prediction rollback runs.
    OnSnapshotReady,
}

impl<C: Channel> Plugin for LateJoinCatchUpPlugin<C> {
    fn build(&self, app: &mut App) {
        if !app.is_message_registered::<CatchUpRequest>() {
            app.register_message::<CatchUpRequest>()
                .add_direction(NetworkDirection::ClientToServer);
        }
        app.init_resource::<CatchUpRegistry>();
        app.init_resource::<CatchUpMode>();
        app.init_resource::<CatchUpServerReadiness>();
        app.init_resource::<CatchUpClientReadiness>();
        app.init_resource::<CatchUpSnapshotReadyState>();
        app.add_message::<CatchUpSnapshotReady>();
        app.add_observer(mark_client_caught_up_if_no_gated_on_connect);
        app.configure_sets(
            PreUpdate,
            (
                CatchUpSystems::UpdateReadiness,
                CatchUpSystems::HandleRequests,
                CatchUpSystems::ApplyPending,
            )
                .chain()
                .after(MessageSystems::Receive)
                .after(InputSystems::ReceiveInputs),
        );
        app.configure_sets(
            PreUpdate,
            (
                CatchUpSystems::DetectSnapshotReady,
                CatchUpSystems::OnSnapshotReady,
            )
                .chain()
                .after(ReplicationSystems::Receive)
                .before(PredictionSystems::Rollback),
        );
        app.add_systems(
            PreUpdate,
            (
                handle_catch_up_requests.in_set(CatchUpSystems::HandleRequests),
                apply_pending_catch_ups.in_set(CatchUpSystems::ApplyPending),
                detect_catch_up_snapshot_ready.in_set(CatchUpSystems::DetectSnapshotReady),
            ),
        );
        app.add_systems(
            PostUpdate,
            send_catchup_request::<C>.before(MessageSystems::Send),
        );
    }
}

/// When a user inserts [`CatchUpGated`] on a server entity, attach the
/// Replicon visibility filter that hides the catch-up-scoped components
/// from clients until their link entity has [`HasCaughtUp`].
fn on_catch_up_gated_added<T: FilterScope + Send + Sync + 'static>(
    trigger: On<Add, CatchUpGated>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    debug!(
        ?entity,
        "CatchUpGated added; inserting catch-up visibility filter"
    );
    commands
        .entity(entity)
        .insert(CatchUpVisibility::<T>::default());
}

/// Once a server marks a client as caught up, insert the same internal
/// visibility filter component on the client link entity. Replicon's filter
/// observer reevaluates existing [`CatchUpGated`] entities when this
/// component is inserted.
fn on_has_caught_up_added<T: FilterScope + Send + Sync + 'static>(
    trigger: On<Add, HasCaughtUp>,
    clients: Query<(), With<ClientVisibility>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    if clients.get(entity).is_err() {
        return;
    }
    debug!(
        ?entity,
        "HasCaughtUp added; inserting catch-up visibility marker on client"
    );
    commands
        .entity(entity)
        .insert(CatchUpVisibility::<T>::default());
}

/// If a client joins before any catch-up-gated entities exist, it is already
/// part of the deterministic simulation and does not need the late-join
/// snapshot flow. Mark it caught up so future gated entities replicate
/// normally to it.
fn mark_client_caught_up_if_no_gated_on_connect(
    trigger: On<Add, Connected>,
    clients: Query<(), (With<ClientOf>, With<LinkOf>)>,
    gated: Query<(), With<CatchUpGated>>,
    mut commands: Commands,
) {
    let client = trigger.entity;
    if clients.get(client).is_err() || !gated.is_empty() {
        return;
    }
    debug!(
        ?client,
        "client connected before any catch-up-gated entities; marking caught up"
    );
    commands.entity(client).insert(HasCaughtUp);
}

/// Mark the client as [`HasCaughtUp`], which lets Replicon's catch-up
/// visibility filter reveal catch-up-scoped components for every
/// [`CatchUpGated`] entity.
///
/// Exposed as a standalone helper so tests and custom triggers can invoke
/// it without constructing a message receiver.
pub fn apply_catch_up_for_client(world: &mut World, client_link_entity: Entity) {
    debug!(?client_link_entity, "applying bundled catch-up for client");
    if let Ok(mut entity_mut) = world.get_entity_mut(client_link_entity) {
        entity_mut.insert(HasCaughtUp);
        entity_mut.remove::<CatchUpRequestReceived>();
    }
}

/// Server system: drain incoming [`CatchUpRequest`] messages and mark
/// each requesting client with [`CatchUpRequestReceived`]. The actual
/// visibility flip is deferred to [`apply_pending_catch_ups`], which runs
/// only after [`CatchUpServerReadiness::all_clients_ready`] becomes true.
/// Deferring ensures the bundled snapshot is computed from real (not
/// predicted) input for every client.
fn handle_catch_up_requests(
    mut query: Query<
        (Entity, &mut MessageReceiver<CatchUpRequest>),
        (With<ClientOf>, Without<CatchUpRequestReceived>),
    >,
    mut commands: Commands,
) {
    for (client_link_entity, mut receiver) in query.iter_mut() {
        let mut saw_request = false;
        for _msg in receiver.receive() {
            saw_request = true;
        }
        if saw_request {
            debug!(
                ?client_link_entity,
                "received CatchUpRequest; marking pending"
            );
            commands
                .entity(client_link_entity)
                .insert(CatchUpRequestReceived);
        }
    }
}

/// Server system: apply catch-up snapshots once the server reports it has
/// received input from every client up through its current tick
/// ([`CatchUpServerReadiness::all_clients_ready`] is `true`).
///
/// First-time clients still need to send [`CatchUpRequest`]. Once a client
/// has [`HasCaughtUp`], the marker is kept permanently; later deterministic
/// state is part of the normal deterministic simulation and no second
/// catch-up visibility flow is needed.
fn apply_pending_catch_ups(world: &mut World) {
    let ready = world.resource::<CatchUpServerReadiness>().all_clients_ready;
    if !ready {
        return;
    }
    let pending: Vec<Entity> = world
        .query_filtered::<Entity, (With<ClientOf>, With<CatchUpRequestReceived>)>()
        .iter(world)
        .collect();
    if pending.is_empty() {
        return;
    }
    debug!(
        count = pending.len(),
        "server readiness reached; applying pending catch-up requests"
    );
    for client_link_entity in pending {
        apply_catch_up_for_client(world, client_link_entity);
    }
}

/// Client system: emits [`CatchUpSnapshotReady`] once all entities currently
/// marked [`AwaitingCatchUpSnapshot`] have a confirmation at the same Replicon
/// checkpoint and that checkpoint can be resolved to a Lightyear simulation
/// tick.
fn detect_catch_up_snapshot_ready(
    mode: Res<CatchUpMode>,
    client: Option<Single<(), With<Client>>>,
    awaiting: Query<(Entity, Option<Ref<ConfirmHistory>>), With<AwaitingCatchUpSnapshot>>,
    checkpoints: Res<ReplicationCheckpointMap>,
    mut state: ResMut<CatchUpSnapshotReadyState>,
    mut events: MessageWriter<CatchUpSnapshotReady>,
) {
    if *mode == CatchUpMode::InputOnly || client.is_none() {
        return;
    }

    let mut reference_entity = None;
    let mut bundled_tick = None;
    let mut entities = Vec::new();
    let mut any_changed = false;

    for (entity, confirm) in &awaiting {
        let Some(confirm) = confirm else {
            return;
        };
        any_changed |= confirm.is_changed();
        let tick = confirm.last_tick();
        match bundled_tick {
            Some(expected) if expected != tick => return,
            Some(_) => {}
            None => {
                bundled_tick = Some(tick);
                reference_entity = Some(entity);
            }
        }
        entities.push(entity);
    }

    let Some(replicon_tick) = bundled_tick else {
        state.last_emitted_replicon_tick = None;
        return;
    };
    if !any_changed || state.last_emitted_replicon_tick == Some(replicon_tick) {
        return;
    }
    let Some(server_tick) = checkpoints.get(replicon_tick) else {
        return;
    };
    let Some(reference_entity) = reference_entity else {
        return;
    };

    state.last_emitted_replicon_tick = Some(replicon_tick);
    events.write(CatchUpSnapshotReady {
        reference_entity,
        replicon_tick,
        server_tick,
        entities,
    });
}

/// Client system: send a [`CatchUpRequest`] once the input timeline is
/// synced and at least one entity is waiting for a catch-up snapshot, unless
/// the client is in [`CatchUpMode::InputOnly`] or a request is already in
/// flight.
///
/// The channel type parameter `C` is the channel the request is sent on;
/// use a reliable channel so the request is not lost.
fn send_catchup_request<C: Channel>(
    mode: Res<CatchUpMode>,
    readiness: Res<CatchUpClientReadiness>,
    awaiting: Query<(), With<AwaitingCatchUpSnapshot>>,
    client: Option<
        Single<
            (Entity, &mut MessageSender<CatchUpRequest>),
            (
                With<Client>,
                With<IsSynced<InputTimeline>>,
                Without<CatchUpRequestSent>,
            ),
        >,
    >,
    mut commands: Commands,
) {
    if *mode == CatchUpMode::InputOnly {
        return;
    }
    if !readiness.can_request {
        return;
    }
    if awaiting.is_empty() {
        return;
    }
    let Some(client) = client else {
        return;
    };
    let (client_entity, mut sender) = client.into_inner();
    debug!(?client_entity, "sending CatchUpRequest to server");
    sender.send::<C>(CatchUpRequest);
    commands.entity(client_entity).insert(CatchUpRequestSent);
}

/// Resolve the server tick at which `entity`'s most recent replication
/// update was produced (via [`ConfirmHistory`] + [`ReplicationCheckpointMap`])
/// and request a single forced rollback to that tick via
/// [`StateRollbackMetadata::request_forced_rollback`].
///
/// Call this once from user code after the bundled catch-up snapshot has
/// landed on every [`CatchUpGated`] entity — typically when the last
/// catch-up-gated component becomes present across them. Because all gated
/// components arrive in one replicon update at one server tick, the
/// reference tick resolved from *any* one such entity is the correct
/// forced-rollback tick for the whole set.
///
/// Also removes [`AwaitingCatchUpSnapshot`] from every entity that has it:
/// the bundled snapshot has landed and the forced rollback is scheduled,
/// so subsequent replicated writes should land on the live component, not
/// on `PredictionHistory`.
///
/// Returns `true` if a rollback was requested.
pub fn request_forced_rollback_to_catch_up_tick(
    world: &mut World,
    reference_entity: Entity,
) -> bool {
    let Some(confirm) = world.get::<ConfirmHistory>(reference_entity) else {
        return false;
    };
    let replicon_tick = confirm.last_tick();
    let Some(server_tick) = world
        .resource::<ReplicationCheckpointMap>()
        .get(replicon_tick)
    else {
        return false;
    };
    let Some(mut state_metadata) = world.get_resource_mut::<StateRollbackMetadata>() else {
        return false;
    };
    debug!(
        ?reference_entity,
        ?server_tick,
        ?replicon_tick,
        "requesting bundled forced rollback to catch-up tick"
    );
    state_metadata.request_forced_rollback(server_tick);
    // Remove `AwaitingCatchUpSnapshot` from every entity so future
    // replicated writes go to the live component instead of history.
    let entities: Vec<Entity> = world
        .query_filtered::<Entity, With<AwaitingCatchUpSnapshot>>()
        .iter(world)
        .collect();
    for entity in entities {
        if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.remove::<AwaitingCatchUpSnapshot>();
        }
    }
    let clients: Vec<Entity> = world
        .query_filtered::<Entity, (With<Client>, With<CatchUpRequestSent>)>()
        .iter(world)
        .collect();
    for client in clients {
        if let Ok(mut entity_mut) = world.get_entity_mut(client) {
            entity_mut.remove::<CatchUpRequestSent>();
        }
    }
    true
}

#[cfg(test)]
mod tests {
    //! Unit tests for the late-join catch-up registry and filter marker
    //! wiring. Replicon's own tests cover the private visibility mask state;
    //! these tests verify that `CatchUpGated` gets the registered filter
    //! component and that applying catch-up permanently marks the client with
    //! `HasCaughtUp`.
    use super::*;
    use bevy_app::App;
    use bevy_replicon::prelude::SingleComponent;
    use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
    use bevy_replicon::server::visibility::registry::FilterRegistry;
    use bevy_replicon::shared::replication::registry::ReplicationRegistry;
    use lightyear_connection::client_of::ClientOf;

    #[derive(Component, Default)]
    struct A;
    #[derive(Component, Default)]
    struct B;
    #[derive(Component, Default)]
    struct C;

    #[derive(Resource, Default)]
    struct ReadyEvents(Vec<CatchUpSnapshotReady>);

    fn collect_ready_events(
        mut reader: MessageReader<CatchUpSnapshotReady>,
        mut events: ResMut<ReadyEvents>,
    ) {
        events.0.extend(reader.read().cloned());
    }

    /// Build an app with the full catch-up wiring: replicon filter
    /// registry + the plugin's registry and observer.
    fn test_app() -> App {
        let mut app = App::new();
        app.init_resource::<FilterRegistry>();
        app.init_resource::<ReplicationRegistry>();
        app.init_resource::<CatchUpRegistry>();
        app.register_catchup_components::<(A, B, C)>();
        app
    }

    fn spawn_client(app: &mut App) -> Entity {
        // `Connected`'s on_insert hook asserts that the entity carries a
        // `RemoteId`, so the test client is shaped like a real remote-client
        // link entity.
        let server = app.world_mut().spawn_empty().id();
        app.world_mut()
            .spawn((
                ClientOf,
                ClientVisibility::default(),
                lightyear_link::server::LinkOf { server },
                lightyear_core::id::RemoteId(lightyear_core::id::PeerId::Server),
            ))
            .id()
    }

    #[test]
    fn register_is_idempotent() {
        let mut app = App::new();
        app.init_resource::<FilterRegistry>();
        app.init_resource::<ReplicationRegistry>();
        app.init_resource::<CatchUpRegistry>();
        assert!(!app.world().resource::<CatchUpRegistry>().is_initialized());

        app.register_catchup_components::<(A, B, C)>();
        assert!(app.world().resource::<CatchUpRegistry>().is_initialized());

        // Second call is a no-op and must not register the same filter twice.
        app.register_catchup_components::<(A, B, C)>();
        assert!(app.world().resource::<CatchUpRegistry>().is_initialized());
    }

    #[test]
    fn register_with_single_component_still_works() {
        // `SingleComponent<C>` is a valid `FilterScope`, so this is the
        // fallback for users who only want to hide one component.
        let mut app = App::new();
        app.init_resource::<FilterRegistry>();
        app.init_resource::<ReplicationRegistry>();
        app.init_resource::<CatchUpRegistry>();
        app.register_catchup_components::<SingleComponent<A>>();
        assert!(app.world().resource::<CatchUpRegistry>().is_initialized());
    }

    #[test]
    fn catch_up_gated_does_not_panic_with_no_clients() {
        let mut app = test_app();
        let entity = app.world_mut().spawn(CatchUpGated).id();
        app.update();
        assert!(
            app.world()
                .get::<CatchUpVisibility<(A, B, C)>>(entity)
                .is_some()
        );
    }

    #[test]
    fn catch_up_gated_inserts_filter_with_clients() {
        let mut app = test_app();
        let _client_a = spawn_client(&mut app);
        let _client_b = spawn_client(&mut app);
        app.update();

        let entity = app.world_mut().spawn(CatchUpGated).id();
        // Observer runs via commands.queue; another update flushes.
        app.update();
        assert!(
            app.world()
                .get::<CatchUpVisibility<(A, B, C)>>(entity)
                .is_some()
        );
    }

    #[test]
    fn client_connecting_later_does_not_panic_with_existing_gated_entities() {
        let mut app = test_app();
        let _entity_one = app.world_mut().spawn(CatchUpGated).id();
        let _entity_two = app.world_mut().spawn(CatchUpGated).id();
        app.update();

        let client = spawn_client(&mut app);
        app.world_mut()
            .entity_mut(client)
            .insert(lightyear_connection::client::Connected);
        app.update();
    }

    #[test]
    fn apply_catch_up_for_client_marks_client_caught_up() {
        let mut app = test_app();
        let client = spawn_client(&mut app);
        let _entity_one = app.world_mut().spawn(CatchUpGated).id();
        let _entity_two = app.world_mut().spawn(CatchUpGated).id();
        app.update();

        assert!(app.world().get::<HasCaughtUp>(client).is_none());
        apply_catch_up_for_client(app.world_mut(), client);
        assert!(app.world().get::<HasCaughtUp>(client).is_some());
    }

    #[test]
    fn apply_catch_up_for_client_without_registry_is_noop() {
        // No `register_catchup_components`.
        let mut app = App::new();
        app.init_resource::<FilterRegistry>();
        app.init_resource::<ReplicationRegistry>();
        app.init_resource::<CatchUpRegistry>();
        let client = app
            .world_mut()
            .spawn((ClientOf, ClientVisibility::default()))
            .id();
        let _entity = app.world_mut().spawn(CatchUpGated).id();
        apply_catch_up_for_client(app.world_mut(), client);
        // Even without a registered filter, HasCaughtUp is just the durable
        // client-side marker.
        assert!(app.world().get::<HasCaughtUp>(client).is_some());
    }

    #[test]
    fn new_gated_entity_after_catch_up_gets_filter_without_removing_client_marker() {
        let mut app = test_app();
        let client = spawn_client(&mut app);
        let entity_one = app.world_mut().spawn(CatchUpGated).id();
        app.update();

        assert!(
            app.world()
                .get::<CatchUpVisibility<(A, B, C)>>(entity_one)
                .is_some()
        );

        apply_catch_up_for_client(app.world_mut(), client);
        assert!(app.world().get::<HasCaughtUp>(client).is_some());

        let entity_two = app.world_mut().spawn(CatchUpGated).id();
        app.update();

        assert!(
            app.world()
                .get::<CatchUpVisibility<(A, B, C)>>(entity_two)
                .is_some()
        );
        assert!(app.world().get::<HasCaughtUp>(client).is_some());
    }

    #[test]
    fn snapshot_ready_event_fires_before_user_hook() {
        let mut app = App::new();
        app.init_resource::<CatchUpSnapshotReadyState>();
        app.init_resource::<CatchUpMode>();
        app.init_resource::<ReplicationCheckpointMap>();
        app.init_resource::<ReadyEvents>();
        app.add_message::<CatchUpSnapshotReady>();
        app.configure_sets(
            PreUpdate,
            (
                CatchUpSystems::DetectSnapshotReady,
                CatchUpSystems::OnSnapshotReady,
            )
                .chain(),
        );
        app.add_systems(
            PreUpdate,
            (
                detect_catch_up_snapshot_ready.in_set(CatchUpSystems::DetectSnapshotReady),
                collect_ready_events.in_set(CatchUpSystems::OnSnapshotReady),
            ),
        );

        app.world_mut().spawn(Client::default());
        let replicon_tick = RepliconTick::new(7);
        let server_tick = Tick(42);
        app.world_mut()
            .resource_mut::<ReplicationCheckpointMap>()
            .record(replicon_tick, server_tick);
        let entity = app
            .world_mut()
            .spawn((AwaitingCatchUpSnapshot, ConfirmHistory::new(replicon_tick)))
            .id();

        app.update();

        let events = &app.world().resource::<ReadyEvents>().0;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].reference_entity, entity);
        assert_eq!(events[0].replicon_tick, replicon_tick);
        assert_eq!(events[0].server_tick, server_tick);
        assert_eq!(events[0].entities, alloc::vec![entity]);
    }
}
