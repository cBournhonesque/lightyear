use std::f32::consts::TAU;

use avian3d::prelude::*;
use bevy::{
    color::palettes::css,
    math::VectorSpace,
    prelude::*,
    time::common_conditions::on_timer,
    utils::{Duration, HashMap},
};
use client::Rollback;
use leafwing_input_manager::{action_diff::ActionDiff, prelude::*};
use lightyear::{
    client::connection,
    prelude::{
        client::{Confirmed, Predicted},
        server::*,
        *,
    },
    server::input::InputSystemSet,
    shared::tick_manager,
};
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;

use crate::{
    protocol::*,
    shared,
    shared::{
        apply_character_action, BlockPhysicsBundle, CharacterPhysicsBundle, CharacterQuery,
        FloorPhysicsBundle, CHARACTER_CAPSULE_HEIGHT, CHARACTER_CAPSULE_RADIUS, FLOOR_HEIGHT,
        FLOOR_WIDTH,
    },
};

// Plugin for server-specific logic
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        app.add_systems(FixedUpdate, handle_character_actions);
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
        Replicate {
            group: REPLICATION_GROUP,
            ..default()
        },
    ));

    // Blocks need to be predicted because their position, rotation, velocity
    // may change.
    let block_replicate_component = Replicate {
        sync: SyncTarget {
            prediction: NetworkTarget::All,
            ..default()
        },
        // Make sure that all entities that are predicted are part of the
        // same replication group
        group: REPLICATION_GROUP,
        ..default()
    };
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
        // Replicate newly connected clients to all players
        let replicate = Replicate {
            sync: SyncTarget {
                prediction: NetworkTarget::All,
                ..default()
            },
            controlled_by: ControlledBy {
                target: NetworkTarget::Single(client_id),
                ..default()
            },
            // Make sure that all entities that are predicted are part of the
            // same replication group
            group: REPLICATION_GROUP,
            ..default()
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
                replicate,
                CharacterPhysicsBundle::default(),
                ColorComponent(color.into()),
                CharacterMarker,
            ))
            .id();

        info!("Created entity {character:?} for client {client_id:?}");
        num_characters += 1;
    }
}
