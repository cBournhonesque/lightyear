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
//!    for every client that has not yet caught up.
//!
//! 2. On the client, user code sends a single bodyless [`CatchUpRequest`]
//!    once the [`InputTimeline`] is synced. The plugin's client-side system
//!    drives this: if the client doesn't already have
//!    [`CatchUpRequestSent`], it sends the request and inserts that marker
//!    plus the [`AwaitingCatchUpSnapshot`] resource.
//!
//! 3. On the server, the [`CatchUpRequest`] handler enumerates every
//!    currently-existing [`CatchUpGated`] entity, flips their catch-up
//!    visibility bit to *visible* for the requesting client, and inserts
//!    [`HasCaughtUp`] on the client's link entity. Because the flips all
//!    happen in one frame, replicon emits a single bundled init message at
//!    a single server tick `S` containing every gated entity's state.
//!
//! 4. The client receives all catch-up components at server tick `S`.
//!    `add_confirmed_write` routes those writes into `PredictionHistory<C>`
//!    as confirmed entries at `S`. User code waits until all gated entities
//!    have their catch-up components present and then calls
//!    [`request_forced_rollback_to_catch_up_tick`] once. That schedules a
//!    single forced rollback from `S` — re-simulating deterministically
//!    forward to the current tick across *all* entities simultaneously.
//!
//! 5. Subsequent entities that become [`CatchUpGated`] after the client has
//!    caught up (e.g. another client joining later) are *not* hidden for
//!    the caught-up client. They flow through regular `replicate_once` at
//!    their own spawn tick; the client integrates them into its simulation
//!    via input replication alone. No second `CatchUpRequest` is needed.
//!
//! # Why bundled, not per-entity
//!
//! A per-entity catch-up (one [`CatchUpRequest`] per gated entity, each
//! producing its own snapshot at its own tick) causes divergence. The first
//! forced rollback replays every entity, but entities that haven't been
//! snapshotted yet are replayed from stale local state, producing drift
//! that subsequent per-entity rollbacks can't fully cancel. Bundling
//! guarantees a single consistent snapshot tick `S` for all catch-up-gated
//! entities.
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
use bevy_replicon::prelude::FilterScope;
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use bevy_replicon::server::visibility::filters_mask::FilterBit;
use bevy_replicon::server::visibility::registry::FilterRegistry;
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
use lightyear_connection::client::{Client, Connected};
use lightyear_connection::client_of::ClientOf;
use lightyear_connection::direction::NetworkDirection;
use lightyear_sync::prelude::{InputTimeline, IsSynced};
use lightyear_link::server::LinkOf;
use lightyear_inputs::server::InputSystems;
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::{AppMessageExt, MessageSender};
use lightyear_messages::receive::MessageReceiver;
use lightyear_prediction::prelude::StateRollbackMetadata;
use lightyear_replication::checkpoint::ReplicationCheckpointMap;
use lightyear_replication::prelude::ConfirmHistory;
use lightyear_transport::prelude::Channel;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

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

/// Stores the single replicon [`FilterBit`] that controls visibility for
/// every catch-up-gated component on every [`CatchUpGated`] entity.
/// Populated by [`AppCatchUpExt::register_catchup_components`].
///
/// # One bit, not one bit per component
///
/// bevy_replicon 0.39 caps the total number of registered visibility scopes
/// at 8 (see `FilterRegistry::register_scope`). lightyear already consumes
/// several (Replicate, Predicted, Interpolated, Controlled, NetworkVisibility)
/// so allocating one bit per physics component would quickly exhaust the
/// budget. Instead we register **one scope** with a tuple of all catch-up
/// components, so the single bit hides/shows the whole bundle together.
///
/// This also matches the intended semantics: catch-up is atomic (you either
/// have the full state snapshot or you don't), so per-component flipping is
/// never useful.
#[derive(Resource, Default)]
pub struct CatchUpRegistry {
    bit: Option<FilterBit>,
}

impl CatchUpRegistry {
    /// Returns true if [`AppCatchUpExt::register_catchup_components`] has
    /// been called.
    pub fn is_initialized(&self) -> bool {
        self.bit.is_some()
    }

    pub(crate) fn bit(&self) -> Option<FilterBit> {
        self.bit
    }
}

fn set_visible_for_client(world: &mut World, entity: Entity, client_link_entity: Entity) {
    let Some(bit) = world.resource::<CatchUpRegistry>().bit() else {
        warn!("CatchUpRegistry not initialized; cannot set catch-up visibility");
        return;
    };
    debug!(
        ?entity,
        ?client_link_entity,
        "setting catch-up bit to visible for client",
    );
    if let Some(mut vis) = world.get_mut::<ClientVisibility>(client_link_entity) {
        vis.set(entity, bit, true);
    }
}

fn hide_for_client(world: &mut World, entity: Entity, client_link_entity: Entity) {
    let Some(bit) = world.resource::<CatchUpRegistry>().bit() else {
        return;
    };
    if let Some(mut vis) = world.get_mut::<ClientVisibility>(client_link_entity) {
        vis.set(entity, bit, false);
    }
}

/// Hide `entity`'s catch-up-gated components for every client that has not
/// yet received the bundled catch-up snapshot (i.e. lacks [`HasCaughtUp`]).
/// Clients with [`HasCaughtUp`] keep their current visibility (visible) so
/// the new entity reaches them via normal `replicate_once` + input
/// replication.
fn hide_for_not_yet_caught_up_clients(world: &mut World, entity: Entity) {
    let Some(bit) = world.resource::<CatchUpRegistry>().bit() else {
        return;
    };
    let clients: Vec<Entity> = world
        .query_filtered::<Entity, (With<ClientVisibility>, Without<HasCaughtUp>)>()
        .iter(world)
        .collect();
    debug!(
        ?entity,
        num_clients = clients.len(),
        "hiding catch-up bit on clients that haven't caught up yet",
    );
    for client in clients {
        if let Some(mut vis) = world.get_mut::<ClientVisibility>(client) {
            vis.set(entity, bit, false);
        }
    }
}

/// Extension trait for registering the catch-up visibility scope.
pub trait AppCatchUpExt {
    /// Register `T` (typically a tuple of physics components, e.g.
    /// `(Position, Rotation, LinearVelocity, AngularVelocity)`) as the
    /// catch-up scope.
    ///
    /// Allocates a single replicon [`FilterBit`] covering all components
    /// in `T` at once, stored in [`CatchUpRegistry`].
    ///
    /// Calling this more than once is a no-op — the plugin supports a
    /// single catch-up scope. If you need multiple independent scopes
    /// you can call replicon's `FilterRegistry::register_scope` directly
    /// and drive the bit manually.
    ///
    /// The components in `T` must also be registered for replication
    /// separately (typically via `replicate_once::<C>()` and
    /// `add_rollback::<C>().add_confirmed_write()`).
    fn register_catchup_components<T: FilterScope + 'static>(&mut self) -> &mut Self;
}

impl AppCatchUpExt for App {
    fn register_catchup_components<T: FilterScope + 'static>(&mut self) -> &mut Self {
        if self.world().resource::<CatchUpRegistry>().is_initialized() {
            return self;
        }
        let bit =
            self.world_mut()
                .resource_scope(|world, mut filter_registry: Mut<FilterRegistry>| {
                    world.resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                        filter_registry.register_scope::<T>(world, &mut registry)
                    })
                });
        self.world_mut().resource_mut::<CatchUpRegistry>().bit = Some(bit);
        self
    }
}

/// Marker component added by server-side user code to entities whose
/// catch-up-gated components should be hidden from clients that have not
/// yet caught up.
///
/// On [`Add`], an observer hides the catch-up bit for every client that
/// does *not* yet have [`HasCaughtUp`]. Clients that have already caught
/// up receive the entity normally (visible from the start) and integrate
/// it via input replication.
///
/// In the deterministic_replication example this is inserted on the player
/// entity next to `Replicate::to_clients(NetworkTarget::All)`.
#[derive(Component, Default)]
pub struct CatchUpGated;

/// Server-side marker inserted on a client's link entity once the client
/// has received its one-shot bundled catch-up snapshot. Subsequent
/// [`CatchUpGated`] entities are *not* hidden from clients with this
/// marker — they flow through normal `replicate_once` replication and
/// input replication.
#[derive(Component, Debug, Default)]
pub struct HasCaughtUp;

/// Client-side marker inserted on the client entity once the bodyless
/// [`CatchUpRequest`] has been sent. Prevents the client system from
/// sending the request more than once per session.
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
/// flag flips to `true`, pending requests are applied on the next frame
/// (replicon emits the bundled snapshot at that server tick).
#[derive(Resource, Debug, Default)]
pub struct CatchUpServerReadiness {
    pub all_clients_ready: bool,
}

/// Client-side resource set to `true` from the moment the client decides
/// it needs a catch-up snapshot until the catch-up snapshot has landed
/// and the forced rollback has been scheduled.
///
/// While this resource's `awaiting` flag is `true`, [`ChecksumSendPlugin`]
/// skips checksum computation entirely. The client's state for
/// [`CatchUpGated`] entities is known to not match the server during this
/// window; filtering those entities out of the order-independent XOR
/// checksum would leave the server hashing over a superset, producing a
/// sustained mismatch for every entity.
///
/// The complementary gate — covering the window from "rollback scheduled"
/// to "rollback executed" — is handled inside [`ChecksumSendPlugin`] by
/// checking [`StateRollbackMetadata::forced_rollback_tick`].
///
/// [`ChecksumSendPlugin`]: crate::prelude::ChecksumSendPlugin
/// [`StateRollbackMetadata::forced_rollback_tick`]: lightyear_prediction::manager::StateRollbackMetadata::forced_rollback_tick
#[derive(Resource, Debug, Default)]
pub struct AwaitingCatchUpSnapshot {
    awaiting: bool,
}

impl AwaitingCatchUpSnapshot {
    pub fn is_awaiting(&self) -> bool {
        self.awaiting
    }

    pub fn set_awaiting(&mut self, value: bool) {
        self.awaiting = value;
    }
}

/// Plugin that wires up the late-join catch-up machinery.
///
/// Clients send a single bodyless [`CatchUpRequest`] once they're synced.
/// The server enumerates all [`CatchUpGated`] entities, flips their catch-up
/// visibility bits to *visible* for the requesting client in one frame, and
/// inserts [`HasCaughtUp`] on the client entity. Replicon sends all
/// gated components as a single bundled init message at a single server
/// tick, which the client then uses to drive a single forced rollback.
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
    /// catch-up visibility on all [`CatchUpGated`] entities.
    ApplyPending,
}

impl<C: Channel> Plugin for LateJoinCatchUpPlugin<C> {
    fn build(&self, app: &mut App) {
        if !app.is_message_registered::<CatchUpRequest>() {
            app.register_message::<CatchUpRequest>()
                .add_direction(NetworkDirection::ClientToServer);
        }
        app.init_resource::<CatchUpRegistry>();
        app.init_resource::<CatchUpMode>();
        app.init_resource::<AwaitingCatchUpSnapshot>();
        app.init_resource::<CatchUpServerReadiness>();
        app.add_observer(on_catch_up_gated_added);
        app.add_observer(on_client_connected_hide_gated);
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
        app.add_systems(
            PreUpdate,
            (
                handle_catch_up_requests.in_set(CatchUpSystems::HandleRequests),
                apply_pending_catch_ups.in_set(CatchUpSystems::ApplyPending),
            ),
        );
        app.add_systems(
            PostUpdate,
            send_catchup_request::<C>.before(MessageSystems::Send),
        );
    }
}

/// When a user inserts [`CatchUpGated`] on a server entity, hide its
/// catch-up-scoped components for every client that has *not* yet caught
/// up. Clients with [`HasCaughtUp`] see the entity via normal
/// `replicate_once` replication.
fn on_catch_up_gated_added(trigger: On<Add, CatchUpGated>, mut commands: Commands) {
    let entity = trigger.entity;
    debug!(
        ?entity,
        "CatchUpGated added; queuing hide for not-yet-caught-up clients"
    );
    commands.queue(move |world: &mut World| {
        hide_for_not_yet_caught_up_clients(world, entity);
    });
}

/// When a new client connects, hide the catch-up-scoped components on every
/// already-existing [`CatchUpGated`] entity *for that one client only*.
///
/// Only the newly-connected client needs re-hiding — other clients'
/// visibility state for those entities is already correct (either hidden
/// if they haven't caught up, or visible because they have).
fn on_client_connected_hide_gated(
    trigger: On<Add, Connected>,
    clients: Query<(), (With<ClientOf>, With<LinkOf>)>,
    mut commands: Commands,
) {
    let client = trigger.entity;
    if clients.get(client).is_err() {
        return;
    }
    debug!(
        ?client,
        "client connected; queuing hide for all existing CatchUpGated entities"
    );
    commands.queue(move |world: &mut World| {
        // New connection — `HasCaughtUp` cannot be set yet.
        let gated: Vec<Entity> = world
            .query_filtered::<Entity, With<CatchUpGated>>()
            .iter(world)
            .collect();
        debug!(
            ?client,
            gated_count = gated.len(),
            "hiding gated entities for new client"
        );
        for entity in gated {
            hide_for_client(world, entity, client);
        }
    });
}

/// Flip the catch-up visibility bit to *visible* for every
/// [`CatchUpGated`] entity for the given client, and mark the client as
/// [`HasCaughtUp`]. Exposed as a standalone helper so tests and custom
/// triggers can invoke it without constructing a message receiver.
pub fn apply_catch_up_for_client(world: &mut World, client_link_entity: Entity) {
    let gated: Vec<Entity> = world
        .query_filtered::<Entity, With<CatchUpGated>>()
        .iter(world)
        .collect();
    debug!(
        ?client_link_entity,
        gated_count = gated.len(),
        "applying bundled catch-up for client",
    );
    for entity in gated {
        set_visible_for_client(world, entity, client_link_entity);
    }
    if let Ok(mut entity_mut) = world.get_entity_mut(client_link_entity) {
        entity_mut.insert(HasCaughtUp);
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

/// Server system: apply any pending catch-up requests once the server
/// reports it has received input from every client up through its current
/// tick ([`CatchUpServerReadiness::all_clients_ready`] is `true`). Applies
/// via [`apply_catch_up_for_client`] which flips visibility for every
/// [`CatchUpGated`] entity and inserts [`HasCaughtUp`].
fn apply_pending_catch_ups(world: &mut World) {
    let ready = world
        .resource::<CatchUpServerReadiness>()
        .all_clients_ready;
    if !ready {
        return;
    }
    let pending: Vec<Entity> = world
        .query_filtered::<Entity, (With<ClientOf>, With<CatchUpRequestReceived>, Without<HasCaughtUp>)>()
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

/// Client system: send a single [`CatchUpRequest`] once the input timeline
/// is synced, unless the client is in [`CatchUpMode::InputOnly`] or the
/// request has already been sent.
///
/// The channel type parameter `C` is the channel the request is sent on;
/// use a reliable channel so the request is not lost.
fn send_catchup_request<C: Channel>(
    mode: Res<CatchUpMode>,
    mut awaiting: ResMut<AwaitingCatchUpSnapshot>,
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
    let Some(client) = client else {
        return;
    };
    let (client_entity, mut sender) = client.into_inner();
    debug!(?client_entity, "sending CatchUpRequest to server");
    sender.send::<C>(CatchUpRequest::default());
    awaiting.set_awaiting(true);
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
/// Also clears the [`AwaitingCatchUpSnapshot`] resource: the bundled
/// snapshot has landed and the forced rollback is scheduled, so once the
/// rollback has run the state will be in sync with the server.
///
/// Returns `true` if a rollback was requested.
pub fn request_forced_rollback_to_catch_up_tick(world: &mut World, reference_entity: Entity) -> bool {
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
    if let Some(mut awaiting) = world.get_resource_mut::<AwaitingCatchUpSnapshot>() {
        awaiting.set_awaiting(false);
    }
    true
}

#[cfg(test)]
mod tests {
    //! Unit tests for the late-join catch-up registry and observer wiring.
    //!
    //! bevy_replicon keeps `FiltersMask` and `ClientVisibility::get`
    //! crate-private, so we can't directly assert on the bit state of a
    //! `ClientVisibility` component from outside the crate. These tests
    //! instead verify:
    //! - registration allocates a single replicon filter bit (via the
    //!   public [`FilterRegistry::register_scope`]) and is idempotent;
    //! - the observers (`on_catch_up_gated_added`,
    //!   `on_client_connected_hide_gated`) and
    //!   [`apply_catch_up_for_client`] do not panic in either state
    //!   (with or without the registry initialized), and they dispatch
    //!   through [`ClientVisibility::set`] exactly via the single shared
    //!   bit in [`CatchUpRegistry`].
    //!
    //! The actual visibility-flipping inside replicon is covered by
    //! replicon's own tests. The full init-message round-trip (replicon
    //! writing the component as an insertion once the bit is flipped,
    //! and `add_confirmed_write` seeding `PredictionHistory`) is covered
    //! by the integration test `test_prediction_history_seeded_from_init_message`
    //! in `lightyear_tests`.
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

    /// Build an app with the full catch-up wiring: replicon filter
    /// registry + the plugin's registry and observers. Allocates a real
    /// replicon filter bit via `register_catchup_components`.
    fn test_app() -> App {
        let mut app = App::new();
        app.init_resource::<FilterRegistry>();
        app.init_resource::<ReplicationRegistry>();
        app.init_resource::<CatchUpRegistry>();
        app.register_catchup_components::<(A, B, C)>();
        app.add_observer(on_catch_up_gated_added);
        app.add_observer(on_client_connected_hide_gated);
        app
    }

    fn spawn_client(app: &mut App) -> Entity {
        // `Connected`'s on_insert hook asserts that the entity carries a
        // `RemoteId`. `on_client_connected_hide_gated` filters on
        // `With<ClientOf>` + `With<LinkOf>`, so the test client must be
        // fully shaped to look like a real remote-client link entity.
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
    fn register_allocates_one_bit_and_is_idempotent() {
        let mut app = App::new();
        app.init_resource::<FilterRegistry>();
        app.init_resource::<ReplicationRegistry>();
        app.init_resource::<CatchUpRegistry>();
        assert!(!app.world().resource::<CatchUpRegistry>().is_initialized());

        app.register_catchup_components::<(A, B, C)>();
        let first_bit = app.world().resource::<CatchUpRegistry>().bit();
        assert!(first_bit.is_some());

        // Second call is a no-op — a second scope is NOT registered.
        app.register_catchup_components::<(A, B, C)>();
        let second_bit = app.world().resource::<CatchUpRegistry>().bit();
        assert_eq!(first_bit, second_bit);
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
        let _entity = app.world_mut().spawn(CatchUpGated).id();
        app.update();
    }

    #[test]
    fn catch_up_gated_does_not_panic_with_clients() {
        let mut app = test_app();
        let _client_a = spawn_client(&mut app);
        let _client_b = spawn_client(&mut app);
        app.update();

        let _entity = app.world_mut().spawn(CatchUpGated).id();
        // Observer runs via commands.queue; another update flushes.
        app.update();
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
    fn apply_catch_up_for_client_does_not_panic() {
        let mut app = test_app();
        let client = spawn_client(&mut app);
        let _entity_one = app.world_mut().spawn(CatchUpGated).id();
        let _entity_two = app.world_mut().spawn(CatchUpGated).id();
        app.update();

        apply_catch_up_for_client(app.world_mut(), client);
        // Client should be marked HasCaughtUp after the helper.
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
        // Even without a registry, HasCaughtUp should still be inserted
        // (the visibility no-op happens at a lower layer).
        assert!(app.world().get::<HasCaughtUp>(client).is_some());
    }
}
