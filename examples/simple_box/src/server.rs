//! The server side of the example.
//! It is possible (and recommended) to run the server in headless mode (without any rendering plugins).
//!
//! The server will:
//! - spawn a new player entity for each client that connects
//! - read inputs from the clients and move the player entities accordingly
//!
//! Lightyear will handle the replication of entities automatically if you add a `Replicate` component to them.
use crate::protocol::*;
use crate::shared;
use bevy::prelude::*;
use lightyear::connection::client::Connected;
use lightyear::connection::host::HostServer;
use lightyear::prelude::input::native::*;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::SEND_INTERVAL;

pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ReplicationMetadata::new(SEND_INTERVAL));
        app.add_systems(Startup, init_server_debug_logging);
        // the physics/FixedUpdates systems that consume inputs should be run in this set.
        app.add_systems(FixedUpdate, (movement, log_server_player_updates).chain());
        app.add_observer(handle_new_client);
        app.add_observer(handle_connected);
        app.add_systems(Update, send_message);
    }
}

#[derive(Resource, Default)]
struct ServerDebugLogging {
    enabled: bool,
}

impl ServerDebugLogging {
    #[cfg(not(target_family = "wasm"))]
    fn from_env() -> Self {
        let enabled = std::env::var("LIGHTYEAR_SIMPLE_BOX_LOG_SERVER")
            .map(|value| value != "0")
            .unwrap_or(false);
        Self { enabled }
    }

    #[cfg(target_family = "wasm")]
    fn from_env() -> Self {
        Self::default()
    }
}

fn init_server_debug_logging(mut commands: Commands) {
    let logging = ServerDebugLogging::from_env();
    if logging.enabled {
        info!("Logging server-side player inputs and position updates");
    }
    commands.insert_resource(logging);
}

/// When a new client tries to connect to a server, an entity is created for it with the `LinkOf` component.
/// This entity represents the link between the server and that client.
///
/// You can add additional components to update the link. In this case we will add a `ReplicationSender` that
/// will enable us to replicate local entities to that client.
pub(crate) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .insert((ReplicationSender::default(), Name::from("Client")));
}

/// If the new client connects to the server, we want to spawn a new player entity for it.
///
/// We have to react specifically on `Connected` because there is no guarantee that the connection request we
/// received was valid. The server could reject the connection attempt for many reasons (server is full, packet is invalid,
/// DDoS attempt, etc.). We want to start the replication only when the client is confirmed as connected.
pub(crate) fn handle_connected(
    trigger: On<Add, Connected>,
    query: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
) {
    let Ok(client_id) = query.get(trigger.entity) else {
        return;
    };
    let client_id = client_id.0;
    let entity = commands
        .spawn((
            PlayerBundle::new(client_id, Vec2::ZERO),
            // we replicate the Player entity to all clients that are connected to this server
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            ControlledBy {
                owner: trigger.entity,
                lifetime: Default::default(),
            },
        ))
        .id();
    info!(
        "Create player entity {:?} for client {:?}",
        entity, client_id
    );
}

/// Read client inputs and move players in server therefore giving a basis for other clients
fn movement(
    timeline: Res<LocalTimeline>,
    host_server: Query<(), With<HostServer>>,
    mut position_query: Query<(&mut PlayerPosition, &ActionState<Inputs>, Has<Predicted>)>,
) {
    let is_host_server = !host_server.is_empty();
    let tick = timeline.tick();
    for (position, inputs, predicted) in position_query.iter_mut() {
        if is_host_server && predicted {
            continue;
        }
        trace!(?tick, ?position, ?inputs, "server");
        shared::shared_movement_behaviour(position, inputs);
    }
}

fn log_server_player_updates(
    logging: Res<ServerDebugLogging>,
    query: Query<
        (
            Entity,
            &PlayerId,
            &PlayerPosition,
            &ActionState<Inputs>,
            Has<Predicted>,
        ),
        Or<(Changed<PlayerPosition>, Changed<ActionState<Inputs>>)>,
    >,
) {
    if !logging.enabled {
        return;
    }
    for (entity, player_id, position, inputs, predicted) in query.iter() {
        info!(
            ?entity,
            ?player_id,
            position = ?position.0,
            ?inputs,
            predicted,
            "Server player update"
        );
    }
}

/// Send messages from server to clients (only in non-headless mode, because otherwise we run with minimal plugins
/// and cannot do input handling)
pub(crate) fn send_message(
    mut sender: ServerMultiMessageSender,
    server: Single<&Server>,
    input: Option<Res<ButtonInput<KeyCode>>>,
) {
    if input.is_some_and(|input| input.just_pressed(KeyCode::KeyM)) {
        let message = Message1(5);
        info!("Sending message: {:?}", message);
        sender
            .send::<_, Channel1>(&message, server.into_inner(), &NetworkTarget::All)
            .unwrap_or_else(|e| {
                error!("Failed to send message: {:?}", e);
            });
    }
}
