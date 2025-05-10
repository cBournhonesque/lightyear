use std::f32::consts::TAU;

use avian3d::prelude::*;
use bevy::color::palettes::css;
use bevy::math::VectorSpace;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use client::Rollback;
use core::time::Duration;
use leafwing_input_manager::action_diff::ActionDiff;
use leafwing_input_manager::prelude::*;
use lightyear::client::connection;
use lightyear::prelude::client::{Confirmed, Predicted};
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear::server::input::InputSystemSet;
use lightyear::shared::tick_manager;
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;

use crate::protocol::*;
use crate::shared;
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
pub struct ExampleServerPlugin {
    pub(crate) predict_all: bool,
}

#[derive(Resource)]
pub struct Global {
    predict_all: bool,
}

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Global {
            predict_all: self.predict_all,
        });
        app.add_systems(Startup, init);
        app.add_systems(
            FixedUpdate,
            (handle_character_actions, player_shoot, despawn_system),
        );
        app.add_systems(Update, handle_connections);
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
    mut commands: Commands,
    query: Query<(&ActionState<CharacterAction>, &ControlledBy, &Position)>,
    tick_manager: Res<TickManager>,
    time: Res<Time<Fixed>>,
) {
    for (action_state, controlled_by, position) in &query {
        if action_state.just_pressed(&CharacterAction::Shoot) {
            if let NetworkTarget::Single(player_id) = controlled_by.target {
                let mut replicate_once = ReplicateOnce::default();
                let replicate_once = replicate_once
                    .add::<Position>()
                    .add::<Rotation>()
                    .add::<LinearVelocity>()
                    .add::<AngularVelocity>()
                    .add::<ComputedMass>()
                    .add::<ExternalForce>()
                    .add::<ExternalImpulse>();

                commands.spawn((
                    Name::new("Projectile"),
                    ProjectileMarker,
                    DespawnAfter {
                        spawned_at: time.elapsed_secs(),
                        lifetime: Duration::from_millis(10000),
                    },
                    RigidBody::Dynamic,
                    position.clone(),
                    Rotation::default(),
                    LinearVelocity(Vec3::Z * 10.), // arbitrary direction since we are just testing rollbacks
                    Replicate {
                        group: REPLICATION_GROUP,
                        controlled_by: ControlledBy {
                            target: NetworkTarget::Single(player_id),
                            lifetime: Lifetime::SessionBased,
                        },
                        sync: SyncTarget {
                            // even remote interpolated players shoot predicted projectiles, so that they can visually
                            // hit our (local player) own predicted entity
                            prediction: NetworkTarget::All,
                            interpolation: NetworkTarget::None,
                        },
                        ..default()
                    },
                    // we don't want clients to receive any replication updates after the initial spawn
                    replicate_once
                ));
            }
        }
    }
}

fn init(mut commands: Commands) {
    commands.start_server();

    commands.spawn((
        Name::new("Floor"),
        FloorPhysicsBundle::default(),
        FloorMarker,
        Position::new(Vec3::ZERO),
        // Floors don't need to be predicted since they will never move.
        // We put it in the same replication group to avoid having the players be replicated before the floor
        // and falling infinitely
        ReplicateToClient::default(),
        REPLICATION_GROUP,
    ));

    // Blocks need to be predicted because their position, rotation, velocity
    // may change.
    let block_replicate_component = (
        ReplicateToClient::default(),
        SyncTarget {
            prediction: NetworkTarget::All,
            ..default()
        },
        // Make sure that all entities that are predicted are part of the
        // same replication group
        REPLICATION_GROUP,
    );
    // commands.spawn((
    //     Name::new("Block"),
    //     BlockPhysicsBundle::default(),
    //     BlockMarker,
    //     Position::new(Vec3::new(1.0, 1.0, 0.0)),
    //     block_replicate_component.clone(),
    // ));
    // commands.spawn((
    //     Name::new("Block"),
    //     BlockPhysicsBundle::default(),
    //     BlockMarker,
    //     Position::new(Vec3::new(-1.0, 1.0, 0.0)),
    //     block_replicate_component.clone(),
    // ));
}

/// Spawn a character whenever a new client has connected.
pub(crate) fn handle_connections(
    global: Res<Global>,
    mut connections: EventReader<ConnectEvent>,
    mut commands: Commands,
    character_query: Query<Entity, With<CharacterMarker>>,
) {
    // Track the number of characters in order to pick colors and starting
    // positions.
    let mut num_characters = character_query.iter().count();
    for connection in connections.read() {
        let client_id = connection.client_id;
        info!("Client connected with client-id {client_id:?}. Spawning character entity.");

        let sync = if global.predict_all {
            SyncTarget {
                prediction: NetworkTarget::All,
                ..default()
            }
        } else {
            SyncTarget {
                prediction: NetworkTarget::Single(connection.client_id),
                interpolation: NetworkTarget::AllExceptSingle(client_id),
            }
        };

        // Pick color and position for player.
        let available_colors = [
            css::LIMEGREEN,
            css::PINK,
            css::YELLOW,
            css::AQUA,
            css::CRIMSON,
            css::GOLD,
            css::ORANGE_RED,
            css::SILVER,
            css::SALMON,
            css::YELLOW_GREEN,
            css::WHITE,
            css::RED,
        ];
        let color = available_colors[num_characters % available_colors.len()];
        let angle: f32 = num_characters as f32 * 5.0;
        let x = 2.0 * angle.cos();
        let z = 2.0 * angle.sin();

        // Spawn the character with ActionState. The client will add their own
        // InputMap.
        let character = commands
            .spawn((
                Name::new("Character"),
                ActionState::<CharacterAction>::default(),
                Position(Vec3::new(x, 3.0, z)),
                // replicate newly connected clients
                ReplicateToClient::default(),
                sync,
                // Make sure that all entities that are predicted are part of the
                // same replication group
                REPLICATION_GROUP,
                ControlledBy {
                    target: NetworkTarget::Single(client_id),
                    ..default()
                },
                CharacterPhysicsBundle::default(),
                ColorComponent(color.into()),
                CharacterMarker,
            ))
            .id();

        info!("Created entity {character:?} for client {client_id:?}");
        num_characters += 1;
    }
}
