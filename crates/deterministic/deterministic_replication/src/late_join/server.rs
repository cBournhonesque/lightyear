use alloc::vec::Vec;
use bevy_app::{App, PostUpdate};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::ApplyDeferred;
use bevy_ecs::system::Commands;
use bevy_replicon::prelude::{AppVisibilityExt, FilterScope, RepliconTick, VisibilityFilter};
use bevy_replicon::server::server_tick::ServerTick;
use bevy_replicon::shared::{AuthMethod, RepliconSharedPlugin};
use core::marker::PhantomData;
use lightyear_connection::client::Connected;
use lightyear_connection::client_of::ClientOf;
use lightyear_core::prelude::LocalTimeline;
use lightyear_link::server::LinkOf;
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::EventSender;
use lightyear_messages::receive::MessageReceiver;
use lightyear_replication::LightyearRepliconServerBackend;
use lightyear_replication::metadata::MetadataChannel;
use lightyear_replication::prelude::ReplicationSystems;
use tracing::debug;

use super::{
    CatchUpGated, CatchUpRegistry, CatchUpRequest, CatchUpSnapshotReady, CatchUpSystems,
    HasCaughtUp,
};

#[derive(Component)]
#[component(immutable)]
pub(crate) struct CatchUpVisibility<T: FilterScope + Send + Sync + 'static>(PhantomData<fn() -> T>);

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

pub(crate) fn register_catchup<T: FilterScope + Send + Sync + 'static>(app: &mut App) {
    app.init_resource::<CatchUpRegistry>();
    if !app.world().resource::<CatchUpRegistry>().is_initialized() {
        app.add_visibility_filter::<CatchUpVisibility<T>>();
        app.add_observer(on_catch_up_gated_added::<T>);
        app.register_required_components::<HasCaughtUp, CatchUpVisibility<T>>();
        app.world_mut()
            .resource_mut::<CatchUpRegistry>()
            .initialized = true;

        let gated: Vec<Entity> = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<Entity, With<CatchUpGated>>();
            query.iter(world).collect()
        };
        for entity in gated {
            app.world_mut()
                .entity_mut(entity)
                .insert(CatchUpVisibility::<T>::default());
        }
        let caught_up_clients: Vec<Entity> = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<Entity, With<HasCaughtUp>>();
            query.iter(world).collect()
        };
        for entity in caught_up_clients {
            app.world_mut()
                .entity_mut(entity)
                .insert(CatchUpVisibility::<T>::default());
        }
    }
}

pub(crate) fn build(app: &mut App) {
    if !app.is_plugin_added::<RepliconSharedPlugin>() {
        app.add_plugins(RepliconSharedPlugin {
            auth_method: AuthMethod::None,
        });
    }
    if !app.is_plugin_added::<LightyearRepliconServerBackend>() {
        app.add_plugins(LightyearRepliconServerBackend);
    }

    app.add_observer(mark_client_caught_up_if_no_gated_on_connect);
    app.add_systems(
        PostUpdate,
        (handle_catch_up_requests, ApplyDeferred)
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

#[derive(Component, Debug, Clone, Copy)]
struct PendingCatchUpSnapshotReady {
    server_tick: lightyear_core::tick::Tick,
    replicon_tick: RepliconTick,
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

/// Server system: accept catch-up requests and reveal the gated snapshot.
///
/// The request means the client is synced and has enough input history to
/// begin catch-up. The accepted snapshot itself is always taken at the
/// server's current authoritative tick; if that tick is newer than the
/// request's input-safe tick, the client waits until its input buffers cover
/// the accepted tick before replaying.
fn handle_catch_up_requests(
    timeline: Res<LocalTimeline>,
    server_tick: Option<Res<ServerTick>>,
    mut query: Query<
        (
            Entity,
            &mut MessageReceiver<CatchUpRequest>,
            &EventSender<CatchUpSnapshotReady>,
        ),
        (With<ClientOf>, Without<HasCaughtUp>),
    >,
    mut commands: Commands,
) {
    let Some(server_tick) = server_tick else {
        return;
    };
    if !server_tick.is_changed() {
        return;
    }
    let snapshot_server_tick = timeline.tick();
    let snapshot_replicon_tick = RepliconTick::new(server_tick.get());
    for (client_link_entity, mut receiver, _) in query.iter_mut() {
        for request in receiver.receive() {
            let pending = pending_snapshot_for_request(
                snapshot_server_tick,
                snapshot_replicon_tick,
                &request,
            );
            debug!(
                ?client_link_entity,
                ?snapshot_server_tick,
                ?snapshot_replicon_tick,
                input_safe_tick = ?request.input_safe_tick,
                "accepting CatchUpRequest"
            );
            commands
                .entity(client_link_entity)
                .insert((HasCaughtUp, pending));
            break;
        }
    }
}

fn pending_snapshot_for_request(
    snapshot_server_tick: lightyear_core::tick::Tick,
    snapshot_replicon_tick: RepliconTick,
    request: &CatchUpRequest,
) -> PendingCatchUpSnapshotReady {
    if snapshot_server_tick > request.input_safe_tick {
        debug!(
            ?snapshot_server_tick,
            input_safe_tick = ?request.input_safe_tick,
            "CatchUpRequest input coverage is behind the current server tick; accepting current snapshot tick and letting the client wait for input coverage"
        );
    }
    PendingCatchUpSnapshotReady {
        server_tick: snapshot_server_tick,
        replicon_tick: snapshot_replicon_tick,
    }
}

/// Send the metadata event only after the accepted visibility reveal has gone
/// through Replicon's send set. This keeps the event's Replicon checkpoint
/// causally tied to the snapshot data the client waits for.
fn emit_catch_up_snapshot_ready(
    mut query: Query<(
        Entity,
        &PendingCatchUpSnapshotReady,
        &mut EventSender<CatchUpSnapshotReady>,
    )>,
    mut commands: Commands,
) {
    for (client_link_entity, pending, mut sender) in query.iter_mut() {
        debug!(
            ?client_link_entity,
            snapshot_server_tick = ?pending.server_tick,
            snapshot_replicon_tick = ?pending.replicon_tick,
            "sending CatchUpSnapshotReady"
        );
        sender.trigger::<MetadataChannel>(CatchUpSnapshotReady {
            replicon_tick: pending.replicon_tick,
            server_tick: pending.server_tick,
        });
        commands
            .entity(client_link_entity)
            .remove::<PendingCatchUpSnapshotReady>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightyear_core::tick::Tick;

    #[test]
    fn stale_request_uses_current_server_tick_for_snapshot() {
        let pending = pending_snapshot_for_request(
            Tick(1133),
            RepliconTick::new(92),
            &CatchUpRequest {
                input_safe_tick: Tick(1126),
            },
        );

        assert_eq!(pending.server_tick, Tick(1133));
        assert_eq!(pending.replicon_tick, RepliconTick::new(92));
    }
}
