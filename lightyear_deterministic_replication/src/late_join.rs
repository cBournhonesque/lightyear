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
//! # Approach (client-driven)
//!
//! Information flows from the client — which actually knows what it has —
//! back to the server:
//!
//! 1. At join, the server replicates "structural" data for existing entities
//!    (markers like `PlayerId`, `DeterministicPredicted`) and starts
//!    rebroadcasting inputs for those entities. Physics components
//!    registered via [`AppCatchUpExt::register_catchup_component`] are
//!    **hidden by default** via a replicon per-component visibility filter.
//!
//! 2. On the client, the example code inserts [`PendingCatchUp`] on remote
//!    entities that need catch-up (typically entities with
//!    `DeterministicPredicted` that are not the local player and have no
//!    physics yet). A client-side system waits until the `LeafwingBuffer`
//!    (or any `InputBuffer`) for that entity has rebroadcast inputs up
//!    to roughly the current remote tick, and then sends a
//!    [`CatchUpForEntity`] message to the server identifying the entity.
//!
//! 3. On the server, the [`CatchUpForEntity`] handler flips the
//!    visibility bit to *visible* for that `(entity, client)` pair. On the
//!    next send, replicon sends the physics components as an init insertion
//!    (since they are registered as `replicate_once` — they have never been
//!    replicated to this client before).
//!
//! 4. The client receives the physics components at some server tick `S`.
//!    The existing `add_confirmed_write` machinery routes these writes into
//!    `PredictionHistory<C>` as confirmed values at `S`. User code then
//!    calls [`request_forced_rollback_from_confirm_history`] once the
//!    snapshot has landed, which schedules a one-shot rollback from `S`
//!    via `StateRollbackMetadata::request_forced_rollback`. Because by
//!    construction the client now has rebroadcast inputs covering
//!    `[S, current_tick]`, the rollback re-simulates deterministically
//!    to the current tick with bit-perfect state.
//!
//!    The rollback is explicitly user-triggered rather than
//!    auto-detected because [`ConfirmHistory::last_tick`] advances on
//!    *any* replication update for the entity — including pre-catch-up
//!    structural updates — so auto-detection would fire a rollback at
//!    the wrong tick in many scenarios.
//!
//! # Why per-component visibility, not entity-level
//!
//! If we hid the whole entity, the client would not know the entity
//! exists and could not send the catch-up request. By keeping the entity
//! visible but hiding only the physics components, the client sees the
//! entity + marker components + rebroadcast inputs and can request the
//! snapshot only when it's useful.
//!
//! # Why not server-driven re-replication
//!
//! The server does not reliably know which ticks of input the client has
//! received (acks cover replication, not rebroadcast inputs). Letting the
//! client drive avoids that guessing game and minimizes bandwidth to one
//! snapshot per catch-up.

use alloc::vec::Vec;
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::component::Component;
use bevy_ecs::entity::{Entity, EntityMapper, MapEntities};
use bevy_ecs::prelude::*;
use bevy_ecs::system::Commands;
use bevy_replicon::prelude::FilterScope;
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use bevy_replicon::server::visibility::filters_mask::FilterBit;
use bevy_replicon::server::visibility::registry::FilterRegistry;
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
use lightyear_connection::client::Connected;
use lightyear_connection::client_of::ClientOf;
use lightyear_connection::direction::NetworkDirection;
use lightyear_link::server::LinkOf;
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::{AppMessageExt, MessageSender};
use lightyear_messages::receive::MessageReceiver;
use lightyear_prediction::prelude::StateRollbackMetadata;
use lightyear_replication::checkpoint::ReplicationCheckpointMap;
use lightyear_replication::prelude::ConfirmHistory;
use lightyear_transport::prelude::Channel;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// Message sent from client to server requesting the one-time snapshot of
/// catch-up-gated components for the given entity.
///
/// The `Entity` inside the message is the **server-side** entity (the remote
/// entity from the client's point of view). Registered with `add_map_entities`
/// so that the client's local entity id is mapped to the server entity on
/// deserialization.
#[derive(Event, Serialize, Deserialize, Clone, Debug)]
pub struct CatchUpForEntity(pub Entity);

impl MapEntities for CatchUpForEntity {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.0 = entity_mapper.get_mapped(self.0);
    }
}

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

fn hide_for_all_clients(world: &mut World, entity: Entity) {
    let Some(bit) = world.resource::<CatchUpRegistry>().bit() else {
        return;
    };
    let clients: Vec<Entity> = world
        .query_filtered::<Entity, With<ClientVisibility>>()
        .iter(world)
        .collect();
    debug!(
        ?entity,
        num_clients = clients.len(),
        "hiding catch-up bit on all connected clients",
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
/// catch-up-gated components should be hidden from all currently-connected
/// clients by default.
///
/// On [`Add`], an observer walks every registered catch-up component and
/// sets its visibility bit to *hidden* for every client. This is the
/// server's way of saying "this entity is catch-up gated; nobody gets its
/// physics until they ask for it."
///
/// In the deterministic_replication example this is inserted on the player
/// entity next to `Replicate::to_clients(NetworkTarget::All)`.
#[derive(Component, Default)]
pub struct CatchUpGated;

/// Marker component added by client-side user code to indicate that an
/// entity is waiting for a catch-up snapshot from the server.
///
/// Once the entity also carries [`CatchUpReady`], [`send_catchup_requests`]
/// sends a [`CatchUpForEntity`] to the server and removes this marker.
///
/// The example code should insert this on remote entities (for instance
/// on `Add<DeterministicPredicted>` where the entity is not the local
/// player) — the plugin intentionally does not add it automatically so
/// that user code stays in control of which entities use catch-up.
///
/// On insertion, [`AwaitingCatchUpSnapshot`] is also inserted via an
/// observer, to gate checksum computation until the catch-up rollback
/// completes.
#[derive(Component, Default)]
pub struct PendingCatchUp;

/// Inserted by user code on an entity with [`PendingCatchUp`] once the
/// readiness condition (e.g. "remote input buffer covers current tick")
/// is satisfied.
///
/// Keeping readiness as a separate component lets the plugin be agnostic
/// to the specific input type used by the user. The example wires up a
/// small system that checks `LeafwingBuffer<PlayerActions>::last_remote_tick`
/// and inserts this marker when it becomes `Some`.
#[derive(Component, Default)]
pub struct CatchUpReady;

/// Marker inserted automatically when [`PendingCatchUp`] is added, and
/// removed by [`request_forced_rollback_from_confirm_history`] once the
/// catch-up snapshot has landed and the forced rollback has been scheduled.
///
/// While this marker is present the entity's state is known to not match
/// the server. [`ChecksumSendPlugin`] skips the whole checksum send if any
/// entity carries this marker — filtering the mismatched entity out of an
/// order-independent XOR checksum would instead leave the server computing
/// the checksum over all entities while the client computes it over a
/// subset, producing a sustained mismatch for every other entity too.
///
/// The complementary gate — covering the window from "rollback scheduled"
/// to "rollback executed" — is handled inside [`ChecksumSendPlugin`] by
/// checking [`StateRollbackMetadata::forced_rollback_tick`], because by
/// then this marker has already been removed.
///
/// [`ChecksumSendPlugin`]: crate::prelude::ChecksumSendPlugin
/// [`StateRollbackMetadata::forced_rollback_tick`]: lightyear_prediction::manager::StateRollbackMetadata::forced_rollback_tick
#[derive(Component, Debug, Default)]
pub struct AwaitingCatchUpSnapshot;

/// Plugin that wires up the late-join catch-up machinery.
///
/// Clients insert [`PendingCatchUp`] on remote entities that need a
/// snapshot; the plugin waits for user code to add [`CatchUpReady`] and
/// then sends [`CatchUpForEntity`] on channel `C`. The server receives
/// the message and flips the catch-up visibility bit to *visible* so
/// that replicon sends the gated components as a one-shot init insertion.
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

impl<C: Channel> Plugin for LateJoinCatchUpPlugin<C> {
    fn build(&self, app: &mut App) {
        if !app.is_message_registered::<CatchUpForEntity>() {
            app.register_message::<CatchUpForEntity>()
                .add_map_entities()
                .add_direction(NetworkDirection::ClientToServer);
        }
        app.init_resource::<CatchUpRegistry>();
        app.add_observer(on_catch_up_gated_added);
        app.add_observer(on_client_connected_hide_gated);
        app.add_observer(on_pending_catch_up_added);
        app.add_systems(
            PreUpdate,
            handle_catch_up_requests.after(MessageSystems::Receive),
        );
        app.add_systems(
            PostUpdate,
            send_catchup_requests::<C>.before(MessageSystems::Send),
        );
    }
}

/// When a user inserts [`CatchUpGated`] on a server entity, immediately
/// hide the catch-up-scoped components for every currently-connected client.
fn on_catch_up_gated_added(trigger: On<Add, CatchUpGated>, mut commands: Commands) {
    let entity = trigger.entity;
    debug!(?entity, "CatchUpGated added; queuing hide_for_all_clients");
    commands.queue(move |world: &mut World| {
        hide_for_all_clients(world, entity);
    });
}

/// When a new client connects, hide the catch-up-scoped components on every
/// already-existing [`CatchUpGated`] entity for that client. Without this,
/// a client that connects *after* an entity was spawned would see the
/// components pushed on the next update because the bit was never set for
/// that client.
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
        "client connected; queuing hide for all CatchUpGated entities"
    );
    commands.queue(move |world: &mut World| {
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
            // `hide_for_all_clients` iterates every `ClientVisibility`
            // entity and setting an already-hidden bit is a no-op, so
            // re-hiding for previously-connected clients is cheap.
            hide_for_all_clients(world, entity);
        }
    });
}

/// Flip the catch-up visibility bit to *visible* for the given client on
/// the given entity. Intended to be called by [`handle_catch_up_requests`]
/// but exposed as a standalone helper so unit tests and custom triggers
/// can invoke it without constructing a message receiver.
pub fn apply_catch_up_for_entity(world: &mut World, client_link_entity: Entity, entity: Entity) {
    debug!(
        ?entity,
        ?client_link_entity,
        "flipping catch-up visibility to visible",
    );
    set_visible_for_client(world, entity, client_link_entity);
}

/// Server system: drain incoming [`CatchUpForEntity`] messages and flip
/// the catch-up visibility bit to *visible* for the requesting client on
/// the targeted entity.
///
/// After this, replicon's `collect_changes` will write those components
/// as init insertions for that client on the next send tick.
fn handle_catch_up_requests(world: &mut World) {
    // Collect (client_link_entity, entity_to_show) pairs first so we can
    // drop the query borrow before mutating `ClientVisibility`.
    let mut requests: Vec<(Entity, Entity)> = Vec::new();
    {
        let mut query = world
            .query_filtered::<(Entity, &mut MessageReceiver<CatchUpForEntity>), With<ClientOf>>();
        for (client_link_entity, mut receiver) in query.iter_mut(world) {
            for msg in receiver.receive() {
                requests.push((client_link_entity, msg.0));
            }
        }
    }

    if !requests.is_empty() {
        debug!(count = requests.len(), "handling CatchUpForEntity requests");
    }
    for (client_link_entity, entity) in requests {
        apply_catch_up_for_entity(world, client_link_entity, entity);
    }
}

/// Client system: for every entity with both [`PendingCatchUp`] and
/// [`CatchUpReady`], send a [`CatchUpForEntity`] request to the server
/// and remove the markers.
///
/// The channel type parameter `C` is the channel the request is sent on;
/// use a reliable channel so the request is not lost.
fn send_catchup_requests<C: Channel>(
    pending: Query<Entity, (With<PendingCatchUp>, With<CatchUpReady>)>,
    client: Option<
        Single<&mut MessageSender<CatchUpForEntity>, With<lightyear_connection::client::Client>>,
    >,
    mut commands: Commands,
) {
    let Some(client) = client else {
        return;
    };
    let mut sender = client.into_inner();
    for entity in pending.iter() {
        debug!(?entity, "sending CatchUpForEntity request to server");
        sender.send::<C>(CatchUpForEntity(entity));
        commands
            .entity(entity)
            .remove::<PendingCatchUp>()
            .remove::<CatchUpReady>();
    }
}

/// Observer: when [`PendingCatchUp`] is added to an entity, also insert
/// [`AwaitingCatchUpSnapshot`] so that the entity is excluded from
/// checksum computation for the whole duration of the catch-up flow.
/// Removal is handled by [`request_forced_rollback_from_confirm_history`].
fn on_pending_catch_up_added(
    trigger: On<Add, PendingCatchUp>,
    already_awaiting: Query<(), With<AwaitingCatchUpSnapshot>>,
    mut commands: Commands,
) {
    if already_awaiting.get(trigger.entity).is_ok() {
        return;
    }
    commands
        .entity(trigger.entity)
        .insert(AwaitingCatchUpSnapshot);
}

/// Resolve the server tick at which `entity`'s most recent replication
/// update was produced (via [`ConfirmHistory`] + [`ReplicationCheckpointMap`])
/// and request a forced rollback to that tick via
/// [`StateRollbackMetadata::request_forced_rollback`].
///
/// Call this once from user code when the catch-up snapshot has been
/// applied to the entity — typically when the first catch-up-gated
/// component becomes present. With `rollback_policy.state = Disabled`,
/// this is the mechanism that turns the newly-arrived confirmed state
/// into an actual simulation re-run from tick `S` forward.
///
/// The plugin does not try to detect snapshot arrival automatically:
/// [`ConfirmHistory::last_tick`] advances on *any* replication update
/// for the entity (including pre-catch-up structural updates), so
/// auto-detection would fire a rollback at the wrong tick when the
/// server entity is still being set up. User code knows which concrete
/// components it's waiting for (e.g. `Position`) and can trigger the
/// rollback at exactly the right moment.
///
/// Returns `true` if a rollback was requested.
pub fn request_forced_rollback_from_confirm_history(world: &mut World, entity: Entity) -> bool {
    let Some(confirm) = world.get::<ConfirmHistory>(entity) else {
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
        ?entity,
        ?server_tick,
        ?replicon_tick,
        "requesting forced rollback from catch-up snapshot"
    );
    state_metadata.request_forced_rollback(server_tick);
    // The snapshot has landed and the rollback is scheduled — the entity's
    // state will be in sync with the server after the rollback runs, so
    // we can resume including it in checksums.
    if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
        entity_mut.remove::<AwaitingCatchUpSnapshot>();
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
    //!   [`apply_catch_up_for_entity`] do not panic in either state
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
    fn apply_catch_up_does_not_panic() {
        let mut app = test_app();
        let client = spawn_client(&mut app);
        let entity = app.world_mut().spawn(CatchUpGated).id();
        app.update();

        apply_catch_up_for_entity(app.world_mut(), client, entity);
    }

    #[test]
    fn apply_catch_up_without_registry_is_noop() {
        // No `set_bit_for_test`, no `register_catchup_components`.
        let mut app = App::new();
        app.init_resource::<FilterRegistry>();
        app.init_resource::<ReplicationRegistry>();
        app.init_resource::<CatchUpRegistry>();
        let client = app
            .world_mut()
            .spawn((ClientOf, ClientVisibility::default()))
            .id();
        let entity = app.world_mut().spawn(CatchUpGated).id();
        apply_catch_up_for_entity(app.world_mut(), client, entity);
    }
}
