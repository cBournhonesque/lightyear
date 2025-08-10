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
use lightyear::input::bei::prelude::Fired;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::SEND_INTERVAL;

pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        // the physics/FixedUpdates systems that consume inputs should be run in this set.
        app.add_observer(movement);
        app.add_observer(handle_new_client);
        app.add_observer(handle_connected);
    }
}

/// When a new client tries to connect to a server, an entity is created for it with the `LinkOf` component.
/// This entity represents the link between the server and that client.
///
/// You can add additional components to update the link. In this case we will add a `ReplicationSender` that
/// will enable us to replicate local entities to that client.
pub(crate) fn handle_new_client(trigger: Trigger<OnAdd, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.target()).insert((
        ReplicationSender::new(SEND_INTERVAL, SendUpdatesMode::SinceLastAck, false),
        Name::from("Client"),
    ));
}

/// If the new client connnects to the server, we want to spawn a new player entity for it.
///
/// We have to react specifically on `Connected` because there is no guarantee that the connection request we
/// received was valid. The server could reject the connection attempt for many reasons (server is full, packet is invalid,
/// DDoS attempt, etc.). We want to start the replication only when the client is confirmed as connected.
pub(crate) fn handle_connected(
    trigger: Trigger<OnAdd, Connected>,
    query: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
) {
    let Ok(client_id) = query.get(trigger.target()) else {
        return;
    };
    let client_id = client_id.0;
    // Generate pseudo random color from client id.
    let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
    let s = 0.8;
    let l = 0.5;
    let color = Color::hsl(h, s, l);
    let entity = commands
        .spawn((
            // Add the context component on the server; it will be replicated to the client
            Player,
            PlayerId(client_id),
            PlayerPosition(Vec2::ZERO),
            PlayerColor(color),
            // we replicate the Player entity to all clients that are connected to this server
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            ControlledBy {
                owner: trigger.target(),
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
    trigger: Trigger<Fired<Movement>>,
    mut position_query: Query<
        &mut PlayerPosition,
        // if we run in host-server mode, we don't want to apply this system to the local client's entities
        // because they are already moved by the client plugin
        (Without<Confirmed>, Without<Predicted>),
    >,
) {
    if let Ok(position) = position_query.get_mut(trigger.target()) {
        shared::shared_movement_behaviour(position, trigger.value);
    }
}
