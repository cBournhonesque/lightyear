use avian2d::prelude::{
    Collider, CollisionLayers, LinearVelocity, PhysicsSchedule, Position, RigidBody,
    SpatialQueryFilter,
};
use bevy::prelude::*;
use bevy::utils::Duration;
use leafwing_input_manager::prelude::*;

use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_player_movement, BOT_RADIUS};
use lightyear::client::prediction::Predicted;
use lightyear::prelude::client::InterpolationDelay;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear::shared::replication::components::InitialReplicated;
use lightyear_avian::prelude::{
    LagCompensationHistory, LagCompensationPlugin, LagCompensationSet, LagCompensationSpatialQuery,
    DEFAULT_AABB_ENVELOPE_LAYER_BIT,
};

// Plugin for server-specific logic
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LagCompensationPlugin);
        app.add_systems(Startup, (init, spawn_bot));
        app.add_systems(Update, spawn_player);
        // the lag compensation systems need to run after LagCompensationSet::UpdateHistory
        app.add_systems(
            PhysicsSchedule,
            compute_hit.in_set(LagCompensationSet::Collisions),
        );
    }
}

pub(crate) fn init(mut commands: Commands) {
    commands.start_server();
}

// Replicate the pre-spawned entities back to the client
// We have to use `InitialReplicated` instead of `Replicated`, because
// the server has already assumed authority over the entity so the `Replicated` component
// has been removed
pub(crate) fn spawn_player(
    mut connections: EventReader<ConnectEvent>,
    mut commands: Commands,
    replicated_players: Query<
        (Entity, &InitialReplicated),
        (Added<InitialReplicated>, With<PlayerId>),
    >,
) {
    connections.read().for_each(|event| {
        let client_id = event.client_id;
        let y = (client_id.to_bits() as f32 * 50.0) % 500.0 - 250.0;
        let color = color_from_id(client_id);
        info!("Spawning player with id: {}", client_id);
        commands.spawn((
            Replicate {
                sync: SyncTarget {
                    prediction: NetworkTarget::Single(client_id),
                    interpolation: NetworkTarget::AllExceptSingle(client_id),
                },
                controlled_by: ControlledBy {
                    target: NetworkTarget::Single(client_id),
                    ..default()
                },
                // make sure that all predicted entities (i.e. all entities for a given client) are part of the same replication group
                group: ReplicationGroup::new_id(client_id.to_bits()),
                ..default()
            },
            PlayerId(client_id),
            Transform::from_xyz(0.0, y, 0.0),
            ColorComponent(color),
            ActionState::<PlayerActions>::default(),
            Name::new("Player"),
        ));
    });
}

/// Spawn a bot
pub(crate) fn spawn_bot(mut commands: Commands) {
    commands.spawn((
        BotMarker,
        Name::new("Bot"),
        Replicate {
            sync: SyncTarget {
                interpolation: NetworkTarget::All,
                ..default()
            },
            // in case the renderer is enabled on the server, we don't want the visuals to be replicated!
            hierarchy: ReplicateHierarchy {
                enabled: false,
                recursive: false,
            },
            // NOTE: all predicted entities must be part of the same replication group!
            ..default()
        },
        Transform::from_xyz(0.0, 10.0, 0.0),
<<<<<<< HEAD
=======
        // WHY IS IT NECESSARY TO ADD POSITION/ROTATION HERE?
>>>>>>> b1643586d7880bb7ea41b3ec09523ad3b9479b5e
        RigidBody::Kinematic,
        Collider::circle(BOT_RADIUS),
        // add the component to make lag-compensation possible!
        LagCompensationHistory::default(),
    ));
}

/// Compute hits if the bullet hits the bot, and increment the score on the player
pub(crate) fn compute_hit(
    // instead of directly using avian's SpatialQuery, we want to use the LagCompensationSpatialQuery
    // to apply lag-compensation (i.e. compute the collision between the bullet and the collider as it
    // was seen by the client when they fired the shot)
    query: LagCompensationSpatialQuery,
    bullets: Query<(&PlayerId, &Position, &LinearVelocity), With<BulletMarker>>,
    manager: Res<ConnectionManager>,
    // the InterpolationDelay component is stored directly on the client entity
    // (the server creates one entity for each client to store client-specific
    // metadata)
    client_query: Query<&InterpolationDelay>,
    mut player_query: Query<(&mut Score, &PlayerId)>,
) {
    bullets.iter().for_each(|(id, position, velocity)| {
        let Ok(delay) = manager
            .client_entity(id.0)
            .map(|client_entity| client_query.get(client_entity).unwrap())
        else {
            error!("Could not retrieve InterpolationDelay for client {id:?}");
            return;
        };
        if query
            .cast_ray(
                // the delay is sent in every input message; the latest InterpolationDelay received
                // is stored on the client entity
                *delay,
                position.0,
                Dir2::new_unchecked(velocity.0.normalize()),
                velocity.length(),
                false,
                &mut SpatialQueryFilter::default(),
            )
            .is_some()
        {
            // if there is a hit, increment the score
            player_query
                .iter_mut()
                .find(|(_, player_id)| player_id.0 == id.0)
                .map(|(mut score, _)| {
                    score.0 += 1;
                });
        }
    })
}
