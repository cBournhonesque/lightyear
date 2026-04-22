use crate::automation::AutomationServerPlugin;
use crate::protocol::*;
use crate::shared;
use crate::shared::color_from_id;
use bevy::prelude::*;
use bevy_replicon::prelude::Remote;
use core::time::Duration;
use lightyear::connection::client::PeerMetadata;
use lightyear::connection::host::{HostClient, HostServer};
use lightyear::input::bei::prelude::Fire;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::SEND_INTERVAL;

pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationServerPlugin);
        app.insert_resource(ReplicationMetadata::new(SEND_INTERVAL));
        app.add_observer(replicate_cursors);
        app.add_observer(replicate_host_cursor);
        app.add_observer(handle_new_client);
        app.add_observer(on_connect);
    }
}

/// When a new client tries to connect to a server, an entity is created for it with the `ClientOf` component.
/// This entity represents the connection between the server and that client.
///
/// You can add additional components to update the connection. In this case we will add a `ReplicationSender` that
/// will enable us to replicate local entities to that client.
pub(crate) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert((
        ReplicationReceiver,
        ReplicationSender,
        Name::from("ClientOf"),
    ));
}

pub(crate) fn on_connect(
    trigger: On<Add, Connected>,
    query: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
) {
    let Ok(client_id) = query.get(trigger.entity) else {
        return;
    };
    let client_id = client_id.0;
    commands.spawn((
        Replicate::manual(vec![trigger.entity]),
        Admin,
        Name::from("Admin"),
        PlayerId(client_id),
    ));
}

/// When we receive a replicated Cursor, replicate it to all other clients
pub(crate) fn replicate_cursors(
    trigger: On<Add, CursorPosition>,
    mut commands: Commands,
    cursor_query: Query<&PlayerId, (With<CursorPosition>, With<Remote>)>,
    peer_metadata: Res<PeerMetadata>,
) {
    let entity = trigger.entity;
    let Ok(player_id) = cursor_query.get(entity) else {
        return;
    };
    let client_id = player_id.0;
    let Some(sender_entity) = peer_metadata.mapping.get(&client_id) else {
        error!("Could not find sender entity for client: {:?}", client_id);
        return;
    };
    info!("received cursor spawn event from client: {client_id:?}");
    configure_cursor_rebroadcast(&mut commands, entity, client_id, *sender_entity);
}

/// In host-server mode, the host cursor already exists in the server world.
/// Reuse that entity directly for rebroadcast instead of routing it back through the
/// client->server replication bridge.
pub(crate) fn replicate_host_cursor(
    trigger: On<Add, CursorPosition>,
    _: Single<(), With<HostServer>>,
    host_client: Single<Entity, With<HostClient>>,
    cursor_query: Query<&PlayerId, (With<CursorPosition>, Without<Remote>)>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok(player_id) = cursor_query.get(entity) else {
        return;
    };

    configure_cursor_rebroadcast(&mut commands, entity, player_id.0, host_client.into_inner());
}

fn configure_cursor_rebroadcast(
    commands: &mut Commands,
    entity: Entity,
    client_id: PeerId,
    sender_entity: Entity,
) {
    if let Ok(mut e) = commands.get_entity(entity) {
        e.insert((
            Replicate::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            ControlledBy {
                owner: sender_entity,
                lifetime: Lifetime::SessionBased,
            },
        ));
    }
}
