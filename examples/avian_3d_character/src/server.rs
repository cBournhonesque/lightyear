use std::f32::consts::TAU;

use avian3d::prelude::*;
use bevy::color::palettes::css;
use bevy::math::VectorSpace;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::connection::client::Connected;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::SEND_INTERVAL;

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

#[derive(Clone)]
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup);
        app.add_systems(
            FixedUpdate,
            (handle_character_actions, player_shoot, despawn_system),
        );
        app.add_observer(handle_new_client);
        app.add_observer(handle_connected);
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
    timeline: Single<&LocalTimeline, With<Server>>,
    query: Query<(&ActionState<CharacterAction>, &Position, &ControlledBy), Without<Predicted>>,
    time: Res<Time<Fixed>>,
) {
    for (action_state, position, controlled_by) in &query {
        let mut position_override = ComponentReplicationOverrides::<Position>::default();
        position_override.global_override(ComponentReplicationOverride {
            replicate_once: true,
            ..default()
        });
        let mut rotation_override = ComponentReplicationOverrides::<Rotation>::default();
        rotation_override.global_override(ComponentReplicationOverride {
            replicate_once: true,
            ..default()
        });
        let mut linear_velocity_override =
            ComponentReplicationOverrides::<LinearVelocity>::default();
        linear_velocity_override.global_override(ComponentReplicationOverride {
            replicate_once: true,
            ..default()
        });
        let mut angular_velocity_override =
            ComponentReplicationOverrides::<AngularVelocity>::default();
        angular_velocity_override.global_override(ComponentReplicationOverride {
            replicate_once: true,
            ..default()
        });
        let mut computed_mass_override = ComponentReplicationOverrides::<ComputedMass>::default();
        computed_mass_override.global_override(ComponentReplicationOverride {
            replicate_once: true,
            ..default()
        });
        let mut external_force_override = ComponentReplicationOverrides::<ExternalForce>::default();
        external_force_override.global_override(ComponentReplicationOverride {
            replicate_once: true,
            ..default()
        });
        let mut external_impulse_override =
            ComponentReplicationOverrides::<ExternalImpulse>::default();
        external_impulse_override.global_override(ComponentReplicationOverride {
            replicate_once: true,
            ..default()
        });

        if action_state.just_pressed(&CharacterAction::Shoot) {
            commands.spawn((
                Name::new("Projectile"),
                ProjectileMarker,
                DespawnAfter {
                    spawned_at: time.elapsed_secs(),
                    lifetime: Duration::from_millis(5000),
                },
                RigidBody::Dynamic,
                position.clone(), // Use current position
                Rotation::default(),
                LinearVelocity(Vec3::Z * 10.),
                Replicate::to_clients(NetworkTarget::All),
                PredictionTarget::to_clients(NetworkTarget::All),
                ControlledBy {
                    owner: controlled_by.owner,
                    lifetime: Default::default(),
                },
                // we don't want clients to receive any replication updates after the initial spawn
                (
                    position_override,
                    rotation_override,
                    linear_velocity_override,
                    angular_velocity_override,
                    computed_mass_override,
                    external_force_override,
                    external_impulse_override,
                ),
            ));
        }
    }
}

// Renamed from init, removed start_server
fn setup(mut commands: Commands) {
    commands.spawn((
        Name::new("Floor"),
        FloorPhysicsBundle::default(),
        FloorMarker,
        Position::new(Vec3::ZERO),
        Replicate::to_clients(NetworkTarget::All),
    ));

    commands.spawn((
        Name::new("Block"),
        BlockPhysicsBundle::default(),
        BlockMarker,
        Position::new(Vec3::new(1.0, 1.0, 0.0)),
        Replicate::to_clients(NetworkTarget::All),
        PredictionTarget::to_clients(NetworkTarget::All),
    ));
}

/// Add the ReplicationSender component to new clients
pub(crate) fn handle_new_client(trigger: Trigger<OnAdd, LinkOf>, mut commands: Commands) {
    commands
        .entity(trigger.target())
        .insert(ReplicationSender::new(
            SEND_INTERVAL,
            SendUpdatesMode::SinceLastAck,
            false,
        ));
}

/// Spawn the player entity when a client connects
pub(crate) fn handle_connected(
    trigger: Trigger<OnAdd, Connected>,
    query: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
    character_query: Query<Entity, With<CharacterMarker>>,
) {
    let Ok(client_id) = query.get(trigger.target()) else {
        return;
    };
    let client_id = client_id.0;
    info!("Client connected with client-id {client_id:?}. Spawning character entity.");

    // Track the number of characters to pick colors and starting positions.
    let mut num_characters = character_query.iter().count();

    // Default prediction/interpolation: predict owner, interpolate others
    let prediction_target = PredictionTarget::to_clients(NetworkTarget::Single(client_id));
    let interpolation_target =
        InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id));

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

    // Spawn the character with ActionState. The client will add their own InputMap.
    let character = commands
        .spawn((
            Name::new("Character"),
            ActionState::<CharacterAction>::default(),
            Position(Vec3::new(x, 3.0, z)),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
            ControlledBy {
                owner: trigger.target(),
                lifetime: Default::default(),
            },
            CharacterPhysicsBundle::default(),
            ColorComponent(color.into()),
            CharacterMarker,
        ))
        .id();

    info!("Created entity {character:?} for client {client_id:?}");
}
