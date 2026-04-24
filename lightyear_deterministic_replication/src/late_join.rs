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
//!    `PredictionHistory<C>` as confirmed values at `S`, and triggers a
//!    state rollback from `S` forward. Because by construction the client
//!    now has rebroadcast inputs covering `[S, current_tick]`, the
//!    rollback re-simulates deterministically to the current tick with
//!    bit-perfect state.
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
use bevy_ecs::component::{Component, Mutable};
use bevy_ecs::entity::{Entity, EntityMapper, MapEntities};
use bevy_ecs::prelude::*;
use bevy_ecs::system::Commands;
use bevy_replicon::prelude::SingleComponent;
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
use lightyear_transport::prelude::Channel;
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use tracing::{debug, info, trace, warn};

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

/// Component-level visibility filter bit for a catch-up-gated component `C`.
///
/// Registered via [`AppCatchUpExt::register_catchup_component::<C>`]. On the
/// server this bit is set to *hidden* for every client on every replicated
/// entity by default, which prevents replicon from ever sending `C` for that
/// entity until [`handle_catch_up_requests`] flips the bit.
#[derive(Resource)]
pub struct CatchUpBit<C: Component>(FilterBit, core::marker::PhantomData<fn() -> C>);

impl<C: Component> CatchUpBit<C> {
    fn new(bit: FilterBit) -> Self {
        Self(bit, core::marker::PhantomData)
    }
}

/// List of per-component functions for flipping visibility on a client.
///
/// Populated by [`AppCatchUpExt::register_catchup_component`]. When the server
/// receives a [`CatchUpForEntity`] from a client, it walks this registry and
/// calls each entry with the target entity and the client's link entity so
/// that every registered catch-up-gated component becomes visible for that
/// client at once.
#[derive(Resource, Default)]
pub struct CatchUpRegistry {
    /// Each entry: `(set_visible_for_client, hide_for_all_clients)`.
    ///
    /// `set_visible_for_client(world, entity, client_link_entity)` flips the
    /// component-scoped bit to visible on the given client's `ClientVisibility`
    /// so replicon will send the component on the next update.
    ///
    /// `hide_for_all_clients(world, entity)` is used on entity spawn to hide
    /// the component from every connected client by default.
    entries: Vec<CatchUpEntry>,
}

struct CatchUpEntry {
    set_visible: fn(&mut World, Entity, Entity),
    hide_for_all: fn(&mut World, Entity),
}

fn set_visible_for_client<C: Component>(
    world: &mut World,
    entity: Entity,
    client_link_entity: Entity,
) {
    let Some(&CatchUpBit(bit, _)) = world.get_resource::<CatchUpBit<C>>() else {
        warn!(
            "CatchUpBit for component not registered; cannot set visibility"
        );
        return;
    };
    if let Some(mut vis) = world.get_mut::<ClientVisibility>(client_link_entity) {
        vis.set(entity, bit, true);
    }
}

fn hide_for_all_clients<C: Component>(world: &mut World, entity: Entity) {
    let Some(&CatchUpBit(bit, _)) = world.get_resource::<CatchUpBit<C>>() else {
        return;
    };
    // Gather all link entities with ClientVisibility and flip the bit off.
    let clients: Vec<Entity> = world
        .query_filtered::<Entity, With<ClientVisibility>>()
        .iter(world)
        .collect();
    for client in clients {
        if let Some(mut vis) = world.get_mut::<ClientVisibility>(client) {
            vis.set(entity, bit, false);
        }
    }
}

/// Extension trait for registering components that should be gated behind
/// the late-join catch-up flow.
pub trait AppCatchUpExt {
    /// Register `C` as a catch-up-gated component.
    ///
    /// This:
    /// - allocates a replicon per-component [`FilterBit`] scoped to
    ///   `SingleComponent<C>`, stored in [`CatchUpBit<C>`];
    /// - adds an entry to the [`CatchUpRegistry`] so that `C` participates in
    ///   server-side hiding and visibility flipping automatically.
    ///
    /// The component must be registered for replication separately (typically
    /// via `app.replicate_once::<C>()` and `add_rollback::<C>().add_confirmed_write()`).
    fn register_catchup_component<C>(&mut self) -> &mut Self
    where
        C: Component<Mutability = Mutable>;

    /// Convenience helper: register a tuple of components.
    ///
    /// Equivalent to calling [`register_catchup_component`] for each element.
    ///
    /// [`register_catchup_component`]: AppCatchUpExt::register_catchup_component
    fn register_catchup_components<T: CatchUpComponentTuple>(&mut self) -> &mut Self;
}

impl AppCatchUpExt for App {
    fn register_catchup_component<C>(&mut self) -> &mut Self
    where
        C: Component<Mutability = Mutable>,
    {
        if self.world().contains_resource::<CatchUpBit<C>>() {
            return self;
        }
        let bit = self
            .world_mut()
            .resource_scope(|world, mut filter_registry: Mut<FilterRegistry>| {
                world.resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                    filter_registry.register_scope::<SingleComponent<C>>(world, &mut registry)
                })
            });
        self.world_mut().insert_resource(CatchUpBit::<C>::new(bit));

        let mut reg = self.world_mut().resource_mut::<CatchUpRegistry>();
        reg.entries.push(CatchUpEntry {
            set_visible: set_visible_for_client::<C>,
            hide_for_all: hide_for_all_clients::<C>,
        });
        self
    }

    fn register_catchup_components<T: CatchUpComponentTuple>(&mut self) -> &mut Self {
        T::register(self);
        self
    }
}

/// Helper trait implemented for tuples of components to enable bulk
/// registration via [`AppCatchUpExt::register_catchup_components`].
///
/// Implemented for tuples of size 1 through 8 of components. Use
/// [`AppCatchUpExt::register_catchup_component`] directly for a single
/// component; this trait exists to keep bulk-registration one line.
pub trait CatchUpComponentTuple {
    fn register(app: &mut App);
}

macro_rules! impl_catchup_tuple {
    ($($name:ident),*) => {
        impl<$($name: Component<Mutability = Mutable>),*> CatchUpComponentTuple for ($($name,)*) {
            fn register(app: &mut App) {
                $(app.register_catchup_component::<$name>();)*
            }
        }
    };
}

impl_catchup_tuple!(C1);
impl_catchup_tuple!(C1, C2);
impl_catchup_tuple!(C1, C2, C3);
impl_catchup_tuple!(C1, C2, C3, C4);
impl_catchup_tuple!(C1, C2, C3, C4, C5);
impl_catchup_tuple!(C1, C2, C3, C4, C5, C6);
impl_catchup_tuple!(C1, C2, C3, C4, C5, C6, C7);
impl_catchup_tuple!(C1, C2, C3, C4, C5, C6, C7, C8);

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
/// While present, [`send_catchup_requests`] polls the entity. Once the
/// entity's remote input buffer covers roughly the current remote tick
/// (see the doc on [`send_catchup_requests`] for the exact check), it
/// sends a [`CatchUpForEntity`] to the server and removes this marker.
///
/// The example code should insert this on remote entities (for instance
/// on `Add<DeterministicPredicted>` where the entity is not the local
/// player) — the plugin intentionally does not add it automatically so
/// that user code stays in control of which entities use catch-up.
#[derive(Component, Default)]
pub struct PendingCatchUp;

/// Insert `PendingCatchUp` + `CatchUpGated` to bootstrap the plugin on
/// late-joining clients.
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
/// hide every registered catch-up component for every currently-connected
/// client.
fn on_catch_up_gated_added(trigger: On<Add, CatchUpGated>, mut commands: Commands) {
    let entity = trigger.entity;
    commands.queue(move |world: &mut World| {
        // Take the registry by value so we can call its fn pointers while
        // holding a mutable world reference.
        let Some(registry) = world.remove_resource::<CatchUpRegistry>() else {
            return;
        };
        for entry in &registry.entries {
            (entry.hide_for_all)(world, entity);
        }
        world.insert_resource(registry);
    });
}

/// When a new client connects, hide every catch-up-gated component on every
/// already-existing [`CatchUpGated`] entity for that client. Without this,
/// a client that connects *after* an entity was spawned would see the
/// components pushed on the next update because the bit was never set for
/// that client.
///
/// `hide_for_all_clients` iterates every `ClientVisibility` entity and
/// setting an already-hidden bit is a no-op, so re-hiding for the
/// already-connected clients is cheap.
fn on_client_connected_hide_gated(
    trigger: On<Add, Connected>,
    clients: Query<(), (With<ClientOf>, With<LinkOf>)>,
    mut commands: Commands,
) {
    let client = trigger.entity;
    if clients.get(client).is_err() {
        return;
    }
    commands.queue(move |world: &mut World| {
        let Some(registry) = world.remove_resource::<CatchUpRegistry>() else {
            return;
        };
        let gated: Vec<Entity> = world
            .query_filtered::<Entity, With<CatchUpGated>>()
            .iter(world)
            .collect();
        for entity in gated {
            for entry in &registry.entries {
                (entry.hide_for_all)(world, entity);
            }
        }
        world.insert_resource(registry);
    });
}

/// Server system: drain incoming [`CatchUpForEntity`] messages and flip
/// each registered catch-up component's visibility bit to *visible* for
/// the requesting client on the targeted entity.
///
/// After this, replicon's `collect_changes` will write those components
/// as init insertions for that client on the next send tick.
fn handle_catch_up_requests(world: &mut World) {
    // Collect (client_link_entity, entity_to_show) pairs first so we can
    // drop the query borrow before mutating `ClientVisibility`.
    let mut requests: Vec<(Entity, Entity)> = Vec::new();
    {
        let mut query =
            world.query_filtered::<(Entity, &mut MessageReceiver<CatchUpForEntity>), With<ClientOf>>();
        for (client_link_entity, mut receiver) in query.iter_mut(world) {
            for msg in receiver.receive() {
                requests.push((client_link_entity, msg.0));
            }
        }
    }

    if requests.is_empty() {
        return;
    }

    let Some(registry) = world.remove_resource::<CatchUpRegistry>() else {
        return;
    };
    for (client_link_entity, entity) in requests {
        debug!(
            ?entity,
            ?client_link_entity,
            "flipping catch-up visibility to visible for {} components",
            registry.entries.len()
        );
        for entry in &registry.entries {
            (entry.set_visible)(world, entity, client_link_entity);
        }
    }
    world.insert_resource(registry);
}

/// Client system: for every entity with [`PendingCatchUp`], check whether
/// the client has rebroadcast inputs covering roughly the current remote
/// tick; if so, send a [`CatchUpForEntity`] request and remove the marker.
///
/// The condition is deliberately strict: we want to send the request only
/// once we are confident that the resulting snapshot at server tick `S`
/// can be replayed forward using already-buffered inputs. The exact
/// condition is delegated to [`PendingCatchUpCondition`] trait impls on
/// the entity; the plugin ships with a generic wrapper keyed off any
/// [`lightyear_inputs::input_buffer::InputBuffer`] component.
///
/// The channel type parameter `C` is the channel the request is sent on;
/// use a reliable channel so the request is not lost.
fn send_catchup_requests<C: Channel>(
    mut pending: Query<
        (Entity, Option<&CatchUpReady>),
        (With<PendingCatchUp>,),
    >,
    client: Option<
        Single<
            &mut MessageSender<CatchUpForEntity>,
            With<lightyear_connection::client::Client>,
        >,
    >,
    mut commands: Commands,
) {
    let Some(client) = client else {
        return;
    };
    let mut sender = client.into_inner();
    for (entity, ready) in pending.iter_mut() {
        if ready.is_none() {
            continue;
        }
        debug!(?entity, "sending CatchUpForEntity");
        sender.send::<C>(CatchUpForEntity(entity));
        commands
            .entity(entity)
            .remove::<PendingCatchUp>()
            .remove::<CatchUpReady>();
    }
}

/// Inserted by user code on an entity with [`PendingCatchUp`] once the
/// readiness condition (e.g. "remote input buffer covers current tick")
/// is satisfied.
///
/// Keeping readiness as a separate component lets the plugin be agnostic
/// to the specific input type used by the example. The example wires up
/// a small system that checks `LeafwingBuffer<PlayerActions>::end_tick()`
/// against `RemoteTimeline::tick()` and inserts this marker when the gap
/// is small enough.
#[derive(Component, Default)]
pub struct CatchUpReady;
