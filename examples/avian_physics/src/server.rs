use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_movement_behaviour, SharedPlugin, WallBundle};
use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::SEND_INTERVAL;

#[derive(Clone)]
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup);
        app.add_observer(handle_new_client);
        app.add_observer(replicate_players);
        app.add_systems(FixedUpdate, movement);
    }
}

pub(crate) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .insert(ReplicationSender::new(
            SEND_INTERVAL,
            SendUpdatesMode::SinceLastAck,
            false,
        ));
}

// Renamed from init, removed Global resource, assume ball is always predicted
fn setup(mut commands: Commands) {
    // Spawn server-authoritative entities (ball and walls)
    commands.spawn((
        Position::default(),
        ColorComponent(css::AZURE.into()),
        Replicate::to_clients(NetworkTarget::All),
        PredictionTarget::to_clients(NetworkTarget::All),
        PhysicsBundle::ball(),
        BallMarker,
        Name::from("Ball"),
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
    timeline: Single<&LocalTimeline, With<Server>>,
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
    let tick = timeline.tick();
    for (entity, position, velocity, action) in action_query.iter_mut() {
        if !action.get_pressed().is_empty() {
            // NOTE: be careful to directly pass Mut<PlayerPosition>
            // getting a mutable reference triggers change detection, unless you use `as_deref_mut()`
            shared_movement_behaviour(velocity, action);
            trace!(?entity, ?tick, ?position, actions = ?action.get_pressed(), "applying movement to player");
        }
    }
}

// Replicate the client-replicated entities back to clients
// This system is triggered when the server receives an entity from a client (ClientOf component is added)
pub(crate) fn replicate_players(
    trigger: On<Add, Connected>,
    mut commands: Commands,
    client_query: Query<&RemoteId, With<ClientOf>>,
) {
    let Ok(client_id) = client_query.get(trigger.entity) else {
        return;
    };
    let client_id = client_id.0;
    let entity = trigger.entity;

    let color = color_from_id(client_id);
    let y = (client_id.to_bits() as f32 * 50.0) % 500.0 - 250.0;
    info!("Spawn player for client {client_id:?}");
    commands.spawn((
        PlayerId(client_id),
        Position::from(Vec2::new(-50.0, y)),
        ColorComponent(color),
        Replicate::to_clients(NetworkTarget::All),
        // Predict to all players
        PredictionTarget::to_clients(NetworkTarget::All),
        ControlledBy {
            owner: entity,
            lifetime: Default::default(),
        },
        PhysicsBundle::player(),
        Name::from("Player"),
    ));
}
