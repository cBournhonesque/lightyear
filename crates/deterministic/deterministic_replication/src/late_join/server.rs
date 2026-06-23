use bevy_app::{App, PostUpdate};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::ApplyDeferred;
use bevy_ecs::system::Commands;
use bevy_replicon::prelude::{AppVisibilityExt, FilterScope, RepliconTick, VisibilityFilter};
use bevy_replicon::server::server_tick::ServerTick;
use bevy_replicon::server::visibility::registry::FilterRegistry;
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
use core::marker::PhantomData;
use lightyear_connection::client::Connected;
use lightyear_connection::client_of::ClientOf;
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::tick::Tick;
use lightyear_link::server::LinkOf;
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::EventSender;
use lightyear_messages::receive::MessageReceiver;
use lightyear_prediction::rollback::CatchUpGated;
use lightyear_replication::metadata::MetadataChannel;
use lightyear_replication::prelude::{PreSpawned, ReplicationSystems};
use tracing::debug;

use super::{CatchUpRegistry, CatchUpRequest, CatchUpSnapshotReady, CatchUpSystems, HasCaughtUp};

#[derive(Component)]
#[component(immutable)]
pub(crate) struct CatchUpVisibility<T: FilterScope + Send + Sync + 'static>(PhantomData<fn() -> T>);

impl<T: FilterScope + Send + Sync + 'static> Default for CatchUpVisibility<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T: FilterScope + Send + Sync + 'static> VisibilityFilter for CatchUpVisibility<T> {
    type ClientComponent = HasCaughtUp;
    type Scope = T;

    fn is_visible(&self, _client: Entity, has_caught_up: Option<&HasCaughtUp>) -> bool {
        has_caught_up.is_some()
    }
}

pub(crate) fn register_catchup<T: FilterScope + Send + Sync + 'static>(app: &mut App) {
    app.init_resource::<FilterRegistry>();
    app.init_resource::<ReplicationRegistry>();
    app.init_resource::<CatchUpRegistry>();
    if !app.world().resource::<CatchUpRegistry>().is_initialized() {
        app.add_visibility_filter::<CatchUpVisibility<T>>();
        app.register_required_components::<CatchUpGated, CatchUpVisibility<T>>();
        app.world_mut()
            .resource_mut::<CatchUpRegistry>()
            .initialized = true;
    }
}

pub(crate) fn build(app: &mut App) {
    app.init_resource::<CatchUpServerState>();
    app.add_observer(mark_client_caught_up_if_no_gated_on_connect);
    app.add_observer(mark_server_has_revealed_catchup_state);
    app.add_systems(
        PostUpdate,
        (
            buffer_catch_up_requests,
            accept_buffered_catch_up_requests,
            ApplyDeferred,
        )
            .chain()
            .in_set(CatchUpSystems::HandleRequests)
            .before(ReplicationSystems::Send)
            .before(MessageSystems::Send),
    );
    app.add_systems(
        PostUpdate,
        emit_catch_up_snapshot_ready
            .after(ReplicationSystems::Send)
            .before(MessageSystems::Send),
    );
}

#[derive(Component, Debug, Clone)]
struct ServerCatchUpMetadata {
    input_safe_tick: Tick,
    snapshot_ready: Option<CatchUpSnapshotReady>,
}

impl ServerCatchUpMetadata {
    fn new(input_safe_tick: Tick) -> Self {
        Self {
            input_safe_tick,
            snapshot_ready: None,
        }
    }

    fn not_required() -> Self {
        Self {
            input_safe_tick: Tick(u32::MAX),
            snapshot_ready: Some(CatchUpSnapshotReady::not_required()),
        }
    }
}

#[derive(Resource, Default)]
struct CatchUpServerState {
    has_revealed_catchup_state: bool,
}

/// If a client is the only connected client, or joins before any server-owned
/// catch-up-gated entities exist, it is already part of the deterministic
/// simulation and does not need the late-join snapshot flow. Mark it caught up
/// so gated components replicate normally to it.
fn mark_client_caught_up_if_no_gated_on_connect(
    trigger: On<Add, Connected>,
    clients: Query<Entity, (With<ClientOf>, With<LinkOf>, With<Connected>)>,
    caught_up_clients: Query<
        (),
        (
            With<ClientOf>,
            With<LinkOf>,
            With<Connected>,
            With<HasCaughtUp>,
        ),
    >,
    catchup_gated: Query<(), With<CatchUpGated>>,
    gated_requiring_catchup: Query<(), (With<CatchUpGated>, Without<PreSpawned>)>,
    server_state: Res<CatchUpServerState>,
    mut commands: Commands,
) {
    let client = trigger.entity;
    if clients.get(client).is_err() {
        return;
    }
    let no_caught_up_clients = caught_up_clients.is_empty();
    let has_any_gated = !catchup_gated.is_empty();
    let has_non_prespawn_gated = !gated_requiring_catchup.is_empty();
    if server_state.has_revealed_catchup_state && no_caught_up_clients && has_any_gated {
        debug!(
            ?client,
            "client is first reconnect after catch-up state was revealed; buffering immediate catch-up snapshot"
        );
        commands
            .entity(client)
            .insert(ServerCatchUpMetadata::new(Tick(0)));
        return;
    }
    let needs_catchup = if server_state.has_revealed_catchup_state {
        has_any_gated
    } else {
        !no_caught_up_clients && has_non_prespawn_gated
    };
    if needs_catchup {
        return;
    }
    debug!(
        ?client,
        no_caught_up_clients,
        has_any_gated,
        has_non_prespawn_gated,
        has_revealed_catchup_state = server_state.has_revealed_catchup_state,
        "client does not need initial catch-up; marking caught up"
    );
    commands
        .entity(client)
        .insert((HasCaughtUp, ServerCatchUpMetadata::not_required()));
}

fn mark_server_has_revealed_catchup_state(
    trigger: On<Add, HasCaughtUp>,
    clients: Query<(), (With<ClientOf>, With<LinkOf>)>,
    mut server_state: ResMut<CatchUpServerState>,
) {
    if clients.get(trigger.entity).is_ok() {
        server_state.has_revealed_catchup_state = true;
    }
}

/// Server system: buffer catch-up requests until they become safe to accept.
///
/// Requests can arrive while the client's advertised input-safe tick is still
/// ahead of the server. Keep the newest input-safe tick so later, fresher
/// requests replace older pending ones instead of being lost.
fn buffer_catch_up_requests(
    mut query: Query<
        (
            Entity,
            &mut MessageReceiver<CatchUpRequest>,
            Option<&mut ServerCatchUpMetadata>,
        ),
        (With<ClientOf>, Without<HasCaughtUp>),
    >,
    mut commands: Commands,
) {
    for (client_link_entity, mut receiver, pending) in query.iter_mut() {
        let mut newest = pending.as_ref().map(|pending| pending.input_safe_tick);
        for request in receiver.receive() {
            newest = Some(newest.map_or(request.input_safe_tick, |tick| {
                core::cmp::max(tick, request.input_safe_tick)
            }));
        }
        let Some(input_safe_tick) = newest else {
            continue;
        };
        if let Some(mut pending) = pending {
            if pending.input_safe_tick != input_safe_tick {
                debug!(
                    ?client_link_entity,
                    previous_input_safe_tick = ?pending.input_safe_tick,
                    ?input_safe_tick,
                    "updating buffered CatchUpRequest"
                );
                pending.input_safe_tick = input_safe_tick;
            }
        } else {
            debug!(
                ?client_link_entity,
                ?input_safe_tick,
                "buffering CatchUpRequest"
            );
            commands
                .entity(client_link_entity)
                .insert(ServerCatchUpMetadata::new(input_safe_tick));
        }
    }
}

/// Server system: accept buffered catch-up requests and reveal the gated
/// snapshot as soon as the server tick has advanced beyond the buffered
/// input-safe tick.
fn accept_buffered_catch_up_requests(
    timeline: Res<LocalTimeline>,
    server_tick: Option<Res<ServerTick>>,
    mut query: Query<(Entity, &mut ServerCatchUpMetadata), (With<ClientOf>, Without<HasCaughtUp>)>,
    mut commands: Commands,
) {
    let Some(server_replicon_tick) = server_tick else {
        return;
    };
    if !server_replicon_tick.is_changed() {
        return;
    }
    let server_tick = timeline.tick();
    let replicon_tick = RepliconTick::new(server_replicon_tick.get());
    for (client_link_entity, mut metadata) in query.iter_mut() {
        if metadata.snapshot_ready.is_some() {
            continue;
        }
        if server_tick <= metadata.input_safe_tick {
            debug!(
                ?client_link_entity,
                ?server_tick,
                input_safe_tick = ?metadata.input_safe_tick,
                "deferring buffered CatchUpRequest until server tick advances past input-safe tick"
            );
            continue;
        }
        debug!(
            ?client_link_entity,
            ?server_tick,
            ?replicon_tick,
            input_safe_tick = ?metadata.input_safe_tick,
            "accepting buffered CatchUpRequest"
        );
        metadata.snapshot_ready = Some(CatchUpSnapshotReady {
            server_tick,
            replicon_tick,
        });
        commands.entity(client_link_entity).insert(HasCaughtUp);
    }
}

/// Send the metadata event only after the accepted visibility reveal has gone
/// through Replicon's send set. This keeps the event's Replicon checkpoint
/// causally tied to the snapshot data the client waits for.
fn emit_catch_up_snapshot_ready(
    mut query: Query<(
        Entity,
        &ServerCatchUpMetadata,
        &mut EventSender<CatchUpSnapshotReady>,
    )>,
    mut commands: Commands,
) {
    for (client_link_entity, metadata, mut sender) in query.iter_mut() {
        let Some(snapshot_ready) = metadata.snapshot_ready.as_ref() else {
            continue;
        };
        debug!(
            ?client_link_entity,
            snapshot_server_tick = ?snapshot_ready.server_tick,
            snapshot_replicon_tick = ?snapshot_ready.replicon_tick,
            "sending CatchUpSnapshotReady"
        );
        sender.trigger::<MetadataChannel>(snapshot_ready.clone());
        commands
            .entity(client_link_entity)
            .remove::<ServerCatchUpMetadata>();
    }
}
