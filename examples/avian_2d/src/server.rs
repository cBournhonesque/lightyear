use crate::automation::AutomationServerPlugin;
use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_movement_behaviour, SharedPlugin};
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
        app.add_plugins(AutomationServerPlugin);
        app.insert_resource(ReplicationMetadata::new(SEND_INTERVAL));
        app.add_systems(Startup, setup);
        app.add_observer(handle_new_client);
        app.add_observer(replicate_players);
        app.add_systems(FixedUpdate, movement);
    }
}

pub(crate) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert(ReplicationSender);
}

fn setup(mut commands: Commands) {
    // Spawn server-authoritative entities.
    commands.spawn((
        Position::default(),
        ColorComponent(css::AZURE.into()),
        Replicate::to_clients(NetworkTarget::All),
        PredictionTarget::to_clients(NetworkTarget::All),
        PhysicsBundle::ball(),
        BallMarker,
        Name::from("Ball"),
    ));
}

/// Applies player input to replicated physics bodies.
pub(crate) fn movement(
    timeline: Res<LocalTimeline>,
    mut action_query: Query<
        (
            Entity,
            &Position,
            &mut LinearVelocity,
            &ActionState<PlayerActions>,
        ),
        With<PlayerId>,
    >,
) {
    let tick = timeline.tick();
    for (entity, position, velocity, action) in action_query.iter_mut() {
        if !action.get_pressed().is_empty() {
            // Pass Mut<LinearVelocity> directly so change detection only fires when movement changes it.
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
    let position = Vec2::new(-50.0, y);
    let rotation = Rotation::radians(0.15);
    info!("Spawn player for client {client_id:?}");
    commands.spawn((
        PlayerId(client_id),
        Position::from(position),
        rotation,
        AngularVelocity(0.35),
        ColorComponent(color),
        Replicate::to_clients(NetworkTarget::All),
        // The Player template is reconstructed independently on every peer,
        // so its locally spawned child should not inherit ReplicateLike.
        DisableReplicateHierarchy,
        // Every client predicts every dynamically interacting player so
        // player-player and player-ball contacts exist in every physics world.
        PredictionTarget::to_clients(NetworkTarget::All),
        ControlledBy {
            owner: entity,
            lifetime: Default::default(),
        },
        PhysicsBundle::player(),
        Name::from("Player"),
    ));
}
