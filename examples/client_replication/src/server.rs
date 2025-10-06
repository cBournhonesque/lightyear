use crate::protocol::*;
use crate::shared;
use crate::shared::color_from_id;
use bevy::prelude::*;
use core::time::Duration;
use lightyear::connection::client::PeerMetadata;
use lightyear::input::bei::prelude::Fire;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::SEND_INTERVAL;

pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(replicate_cursors);
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
        ReplicationReceiver::default(),
        ReplicationSender::new(SEND_INTERVAL, SendUpdatesMode::SinceLastAck, false),
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
    // We add an observer on both Cursor and Replicated because
    // in host-server mode, Replicated is not present on the entity when
    // CursorPosition is added. (Replicated gets added slightly after by an observer)
    trigger: On<Add, (CursorPosition, Replicated)>,
    mut commands: Commands,
    cursor_query: Query<&Replicated, With<CursorPosition>>,
    client_query: Query<&RemoteId, With<ClientOf>>,
) {
    let entity = trigger.entity;
    let Ok(replicated) = cursor_query.get(entity) else {
        return;
    };
    let client_id = client_query.get(replicated.receiver).unwrap().0;
    info!("received cursor spawn event from client: {client_id:?}");
    if let Ok(mut e) = commands.get_entity(entity) {
        // Cursor: replicate to others, interpolate for others
        e.insert((
            // do not replicate back to the client that owns the cursor!
            Replicate::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            ControlledBy {
                owner: replicated.receiver,
                lifetime: Lifetime::SessionBased,
            },
        ));
    }
}
