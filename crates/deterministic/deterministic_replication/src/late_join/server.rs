use bevy_app::{App, PostUpdate};
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::*;
use bevy_ecs::system::Commands;
use bevy_replicon::prelude::{AppVisibilityExt, FilterScope, RepliconTick, VisibilityFilter};
use bevy_replicon::server::server_tick::ServerTick;
use core::marker::PhantomData;
use lightyear_connection::client::Connected;
use lightyear_connection::client_of::ClientOf;
use lightyear_core::prelude::LocalTimeline;
use lightyear_link::server::LinkOf;
use lightyear_messages::prelude::EventSender;
use lightyear_messages::receive::MessageReceiver;
use lightyear_replication::metadata::MetadataChannel;
use lightyear_replication::prelude::ReplicationSystems;
use tracing::debug;
use lightyear_prediction::rollback::CatchUpGated;
use super::{
    CatchUpRegistry, CatchUpRequest, CatchUpSnapshotReady, CatchUpSystems,
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
    type ClientComponent = HasCaughtUp;
    type Scope = T;

    fn is_visible(&self, _client: Entity, has_caught_up: Option<&HasCaughtUp>) -> bool {
        has_caught_up.is_some()
    }
}

pub(crate) fn register_catchup<T: FilterScope + Send + Sync + 'static>(app: &mut App) {
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
    app.add_observer(mark_client_caught_up_if_no_gated_on_connect);
    app.add_systems(
        PostUpdate,
        handle_catch_up_requests
            .chain()
            .in_set(CatchUpSystems::HandleRequests)
            .before(ReplicationSystems::Send)
    );
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
            &mut EventSender<CatchUpSnapshotReady>,
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
    let server_tick = timeline.tick();
    let replicon_tick = RepliconTick::new(server_tick.0);
    for (client_link_entity, mut receiver, mut sender) in query.iter_mut() {
        for request in receiver.receive() {
            if server_tick < request.input_safe_tick {
                debug!(
                    ?client_link_entity,
                    ?server_tick,
                    ?request.input_safe_tick,
                    "deferring CatchUpRequest since server tick is not input-safe yet"
                );
                continue;
            }
            debug!(
                ?client_link_entity,
                ?server_tick,
                ?replicon_tick,
                input_safe_tick = ?request.input_safe_tick,
                "accepting CatchUpRequest"
            );
        sender.trigger::<MetadataChannel>(CatchUpSnapshotReady {
                replicon_tick,
                server_tick,
            });
            commands
                .entity(client_link_entity)
                .insert(HasCaughtUp);
            break;
        }
    }
}