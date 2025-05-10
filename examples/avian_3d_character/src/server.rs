use std::f32::consts::TAU;

use avian3d::prelude::*;
use bevy::color::palettes::css;
use bevy::math::VectorSpace;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::prelude::client::{Confirmed, Predicted}; // Keep client components for queries
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear::connection::client::Connected; // Import Connected
use lightyear_examples_common::shared::SEND_INTERVAL; // Import SEND_INTERVAL

use crate::protocol::*;
use crate::shared; // Keep shared import
use crate::shared::apply_character_action;
use crate::shared::BlockPhysicsBundle;
use crate::shared::CharacterPhysicsBundle;
use crate::shared::CharacterQuery;
use crate::shared::FloorPhysicsBundle;
use crate::shared::CHARACTER_CAPSULE_HEIGHT;
use crate::shared::CHARACTER_CAPSULE_RADIUS;
use crate::shared::FLOOR_HEIGHT;
use crate::shared::FLOOR_WIDTH;

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
        app.add_systems(Startup, setup); // Use setup instead of init
        app.add_systems(
            FixedUpdate,
            (handle_character_actions, player_shoot, despawn_system),
        );
        // Use observers for connection events
        app.add_observer(handle_new_client);
        app.add_observer(handle_connected);
        // app.add_systems(Update, handle_connections); // Removed old handler
    }
}

fn handle_character_actions(
    time: Res<Time>,
    spatial_query: SpatialQuery,
    mut query: Query<(&ActionState<CharacterAction>, CharacterQuery)>,
) {
    for (action_state, mut character) in &mut query {
        apply_character_action(&time, &spatial_query, action_state, &mut character);
    }
}

#[derive(Component)]
pub struct DespawnAfter {
    spawned_at: f32,
    lifetime: Duration,
}

fn despawn_system(
    mut commands: Commands,
    query: Query<(Entity, &DespawnAfter)>,
    time: Res<Time<Fixed>>,
) {
    for (entity, despawn) in &query {
        if time.elapsed_secs() - despawn.spawned_at >= despawn.lifetime.as_secs_f32() {
            commands.entity(entity).despawn();
        }
    }
}

fn player_shoot(
    mut commands: Commands, // Added back mut commands
    query: Query<(&ActionState<CharacterAction>, &Position, Has<Controlled>), Without<Predicted>>, // Query server-auth entities
    tick_manager: Res<TickManager>,
    time: Res<Time<Fixed>>,
) {
    for (action_state, position, is_controlled) in &query {
        // Find the controlling player_id if this entity is controlled
        // TODO: This is inefficient. Ideally, the Controlled component stores the PeerId.
        //       Or we query for the ClientOf entity associated with this controlled entity.
        //       For now, we assume the ActionState is only present on controlled entities.
        //       A better approach might be needed.
        let maybe_player_id = if is_controlled {
             // This is a placeholder - need a reliable way to get PeerId from ActionState owner
             None // FIXME: How to get PeerId here?
        } else { None };


        if action_state.just_pressed(&CharacterAction::Shoot) {
             if let Some(player_id) = maybe_player_id { // Check if we found a player_id
                commands.spawn((
                    Name::new("Projectile"),
                    ProjectileMarker,
                    DespawnAfter {
                        spawned_at: time.elapsed_secs(),
                        lifetime: Duration::from_millis(10000),
                    },
                    RigidBody::Dynamic,
                    position.clone(), // Use current position
                    Rotation::default(),
                    LinearVelocity(Vec3::Z * 10.), // arbitrary direction
                    // Use new replication components
                    Replicate::to_clients(NetworkTarget::All) // Replicate projectile to all
                        .set_group(REPLICATION_GROUP),
                    PredictionTarget::to_clients(NetworkTarget::All), // Predict projectile for all
                    InterpolationTarget::to_clients(NetworkTarget::None), // No interpolation for projectile
                    // ControlledBy is implicitly handled by PredictionTarget/InterpolationTarget
                    // we don't want clients to receive any replication updates after the initial spawn
                    ReplicateOnceComponent::<Position>::default(), // Keep ReplicateOnce
                    ReplicateOnceComponent::<Rotation>::default(), // Keep ReplicateOnce
                    ReplicateOnceComponent::<LinearVelocity>::default(),
                    ReplicateOnceComponent::<AngularVelocity>::default(),
                    ReplicateOnceComponent::<ComputedMass>::default(),
                    ReplicateOnceComponent::<ExternalForce>::default(),
                    ReplicateOnceComponent::<ExternalImpulse>::default(),
                ));
            }
        }
    }
}

// Renamed from init, removed start_server
fn setup(mut commands: Commands) {
    // commands.start_server(); // Removed: Handled in main.rs

    commands.spawn((
        Name::new("Floor"),
        FloorPhysicsBundle::default(),
        FloorMarker,
        Position::new(Vec3::ZERO),
        // Use new replication components
        Replicate::to_clients(NetworkTarget::All) // Replicate floor to all clients
            .set_group(REPLICATION_GROUP), // Put in replication group
        // Floor doesn't need prediction/interpolation targets
    ));

    // Blocks spawning logic can be added here if needed, using the new replication components
    // let block_replicate = Replicate::to_clients(NetworkTarget::All)
    //     .set_group(REPLICATION_GROUP);
    // let block_prediction = PredictionTarget::to_clients(NetworkTarget::All);
    // let block_interpolation = InterpolationTarget::to_clients(NetworkTarget::All); // Or None if not needed
    // commands.spawn((
    //     Name::new("Block"),
    //     BlockPhysicsBundle::default(),
    //     BlockMarker,
    //     Position::new(Vec3::new(1.0, 1.0, 0.0)),
    //     block_replicate.clone(),
    //     block_prediction.clone(),
    //     block_interpolation.clone(),
    // ));
}

/// Add the ReplicationSender component to new clients
pub(crate) fn handle_new_client(
    trigger: Trigger<OnAdd, ClientOf>,
    mut commands: Commands,
) {
    commands.entity(trigger.target()).insert(
        ReplicationSender::new(
            SEND_INTERVAL,
            SendUpdatesMode::SinceLastAck,
            false,
        ),
    );
}


/// Spawn the player entity when a client connects
pub(crate) fn handle_connected(
    trigger: Trigger<OnAdd, Connected>,
    mut query: Query<&Connected, With<ClientOf>>,
    mut commands: Commands,
    character_query: Query<Entity, With<CharacterMarker>>, // Query existing characters
) {
    let connected = query.get(trigger.target()).unwrap();
    let client_id = connected.peer_id; // Use PeerId
    info!("Client connected with client-id {client_id:?}. Spawning character entity.");

    // Track the number of characters to pick colors and starting positions.
    let mut num_characters = character_query.iter().count();

    // Default prediction/interpolation: predict owner, interpolate others
    let prediction_target = PredictionTarget::to_clients(NetworkTarget::Single(client_id));
    let interpolation_target = InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id));

    // Pick color and position for player.
    let available_colors = [
        css::LIMEGREEN, css::PINK, css::YELLOW, css::AQUA, css::CRIMSON, css::GOLD,
        css::ORANGE_RED, css::SILVER, css::SALMON, css::YELLOW_GREEN, css::WHITE, css::RED,
    ];
    let color = available_colors[num_characters % available_colors.len()];
    let angle: f32 = num_characters as f32 * 5.0;
    let x = 2.0 * angle.cos();
    let z = 2.0 * angle.sin();

    // Spawn the character with ActionState. The client will add their own InputMap.
    let character = commands
        .spawn((
            Name::new("Character"),
            ActionState::<CharacterAction>::default(),
            Position(Vec3::new(x, 3.0, z)),
            // Use new replication components
            Replicate::to_clients(NetworkTarget::All) // Replicate character to all clients
                .set_group(REPLICATION_GROUP), // Put in replication group
            prediction_target, // Set prediction target
            interpolation_target, // Set interpolation target
            // ControlledBy is implicitly handled by PredictionTarget/InterpolationTarget
            CharacterPhysicsBundle::default(),
            ColorComponent(color.into()),
            CharacterMarker,
        ))
        .id();

    info!("Created entity {character:?} for client {client_id:?}");
    num_characters += 1; // Increment character count
}
