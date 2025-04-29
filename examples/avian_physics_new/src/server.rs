use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_movement_behaviour};
use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::prelude::client::{Confirmed, Predicted};
use lightyear::prelude::server::*;
use lightyear::prelude::*;
// Removed InitialReplicated, InputSystemSet
// use lightyear::server::input::InputSystemSet;
// use lightyear::shared::replication::components::InitialReplicated;
use lightyear_examples_common_new::shared::SEND_INTERVAL; // Import SEND_INTERVAL

// Plugin for server-specific logic
#[derive(Clone)] // Added Clone
pub struct ExampleServerPlugin;
// { // Removed predict_all field
//     pub(crate) predict_all: bool,
// }

// Removed Global resource
// #[derive(Resource)]
// pub struct Global {
//     predict_all: bool,
// }

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        // Removed Global resource insertion
        // app.insert_resource(Global {
        //     predict_all: self.predict_all,
        // });

        // Removed start_server system, renamed init to setup
        app.add_systems(Startup, setup);
        // app.add_systems(Startup, (start_server, init));

        // Use observer for adding replication components
        app.add_observer(replicate_players);
        // app.add_systems(
        //     PreUpdate,
        //     replicate_players.in_set(ServerReplicationSet::ClientReplication),
        // );
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_systems(FixedUpdate, movement);
    }
}

// Removed start_server system
// fn start_server(mut commands: Commands) {
//     commands.start_server();
// }

// Renamed from init, removed Global resource, assume ball is always predicted
fn setup(mut commands: Commands) {
    // Spawn server-authoritative entities (ball and walls)
    commands.spawn(BallBundle::new(
        Vec2::new(0.0, 0.0),
        css::AZURE.into(),
        // Assume ball is always predicted for simplicity in refactor
        true,
        // global.predict_all,
    ));
    // Spawn walls (moved from shared init)
    const WALL_SIZE: f32 = 350.0; // Define WALL_SIZE locally
    commands.spawn(WallBundle::new(
        Vec2::new(-WALL_SIZE, -WALL_SIZE),
        Vec2::new(-WALL_SIZE, WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(-WALL_SIZE, WALL_SIZE),
        Vec2::new(WALL_SIZE, WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(WALL_SIZE, WALL_SIZE),
        Vec2::new(WALL_SIZE, -WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(WALL_SIZE, -WALL_SIZE),
        Vec2::new(-WALL_SIZE, -WALL_SIZE),
        Color::WHITE,
    ));
}

/// Read client inputs and move players
/// NOTE: this system can now be run in both client/server!
pub(crate) fn movement(
    tick_manager: Res<TickManager>,
    mut action_query: Query<
        (
            Entity,
            &Position,
            &mut LinearVelocity,
            &ActionState<PlayerActions>,
        ),
        // if we run in host-server mode, we don't want to apply this system to the local client's entities
        // because they are already moved by the client plugin
        (Without<Confirmed>, Without<Predicted>),
    >,
) {
    for (entity, position, velocity, action) in action_query.iter_mut() {
        if !action.get_pressed().is_empty() {
            // NOTE: be careful to directly pass Mut<PlayerPosition>
            // getting a mutable reference triggers change detection, unless you use `as_deref_mut()`
            shared_movement_behaviour(velocity, action);
            trace!(?entity, tick = ?tick_manager.tick(), ?position, actions = ?action.get_pressed(), "applying movement to player");
        }
    }
}

// Replicate the client-replicated entities back to clients
// This system is triggered when the server receives an entity from a client (ClientOf component is added)
pub(crate) fn replicate_players(
    trigger: Trigger<OnAdd, ClientOf>, // Trigger on ClientOf addition
    mut commands: Commands,
    client_query: Query<&ClientOf>, // Query the ClientOf component
) {
    let entity = trigger.target();
    let Ok(client_of) = client_query.get(entity) else {
        error!("ClientOf component not found on entity {entity:?} triggered by OnAdd<ClientOf>");
        return;
    };
    let client_id = client_of.peer_id; // Get PeerId from ClientOf

    info!(
        "Received player entity {entity:?} from client {client_id:?}. Adding replication components."
    );

    // Add the necessary replication components to the entity received from the client
    if let Some(mut e) = commands.get_entity(entity) {
        // Standard prediction: predict owner, interpolate others
        let prediction_target = PredictionTarget::to_clients(NetworkTarget::Single(client_id));
        let interpolation_target = InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id));

        e.insert((
            // Replicate to all clients, including the owner
            Replicate::to_clients(NetworkTarget::All)
                .set_group(REPLICATION_GROUP), // Use prediction group
            prediction_target,
            interpolation_target,
            // Add physics bundle on the server side
            PhysicsBundle::player(),
            // Add ReplicationSender to send updates back to clients
            ReplicationSender::new(
                SEND_INTERVAL,
                SendUpdatesMode::SinceLastAck,
                false,
            ),
        ));
    }
}
