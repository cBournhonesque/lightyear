use avian2d::prelude::*;
use bevy::prelude::*;
use bevy::time::Stopwatch;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use core::ops::DerefMut;

use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_player_movement, BOT_RADIUS};
use lightyear::client::prediction::Predicted;
use lightyear::prelude::client::{Confirmed, InterpolationDelay};
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear::shared::replication::components::InitialReplicated;
use lightyear_avian::prelude::{
    LagCompensationHistory, LagCompensationPlugin, LagCompensationSet, LagCompensationSpatialQuery,
};

// Plugin for server-specific logic
pub struct ExampleServerPlugin;

const BULLET_COLLISION_DISTANCE_CHECK: f32 = 4.0;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LagCompensationPlugin);
        app.add_systems(Startup, (init, spawn_bots));
        app.add_systems(Update, spawn_player);
        // the lag compensation systems need to run after LagCompensationSet::UpdateHistory
        app.add_systems(FixedUpdate, interpolated_bot_movement);
        app.add_systems(
            PhysicsSchedule,
            // lag compensation collisions must run after the SpatialQuery has been updated
            compute_hit_lag_compensation.after(PhysicsStepSet::SpatialQuery),
        );
        app.add_systems(
            FixedPostUpdate,
            // check collisions after physics have run
            compute_hit_prediction.after(PhysicsSet::Sync),
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
            Score(0),
            PlayerId(client_id),
            Transform::from_xyz(0.0, y, 0.0),
            ColorComponent(color),
            ActionState::<PlayerActions>::default(),
            Name::new("Player"),
        ));
    });
}

/// Spawn bots (one predicted, one interpolated)
pub(crate) fn spawn_bots(mut commands: Commands) {
    commands.spawn((
        InterpolatedBot,
        Name::new("InterpolatedBot"),
        Replicate {
            sync: SyncTarget {
                interpolation: NetworkTarget::All,
                ..default()
            },
            // NOTE: all predicted entities must be part of the same replication group!
            ..default()
        },
        // in case the renderer is enabled on the server, we don't want the visuals to be replicated!
        DisableReplicateHierarchy,
        Transform::from_xyz(-200.0, 10.0, 0.0),
        RigidBody::Kinematic,
        Collider::circle(BOT_RADIUS),
        // add the component to make lag-compensation possible!
        LagCompensationHistory::default(),
    ));
    commands.spawn((
        PredictedBot,
        Name::new("PredictedBot"),
        Replicate {
            sync: SyncTarget {
                prediction: NetworkTarget::All,
                ..default()
            },
            // NOTE: all predicted entities must be part of the same replication group!
            ..default()
        },
        // in case the renderer is enabled on the server, we don't want the visuals to be replicated!
        DisableReplicateHierarchy,
        Transform::from_xyz(200.0, 10.0, 0.0),
        RigidBody::Kinematic,
        Collider::circle(BOT_RADIUS),
    ));
}

/// Compute hits if the bullet hits the bot, and increment the score on the player
pub(crate) fn compute_hit_lag_compensation(
    // instead of directly using avian's SpatialQuery, we want to use the LagCompensationSpatialQuery
    // to apply lag-compensation (i.e. compute the collision between the bullet and the collider as it
    // was seen by the client when they fired the shot)
    mut commands: Commands,
    tick_manager: Res<TickManager>,
    query: LagCompensationSpatialQuery,
    bullets: Query<(Entity, &PlayerId, &Position, &LinearVelocity), With<BulletMarker>>,
    manager: Res<ConnectionManager>,
    // the InterpolationDelay component is stored directly on the client entity
    // (the server creates one entity for each client to store client-specific
    // metadata)
    client_query: Query<&InterpolationDelay>,
    mut player_query: Query<(&mut Score, &PlayerId)>,
) {
    let tick = tick_manager.tick();
    bullets.iter().for_each(|(entity, id, position, velocity)| {
        let Ok(delay) = manager
            .client_entity(id.0)
            .map(|client_entity| client_query.get(client_entity).unwrap())
        else {
            error!("Could not retrieve InterpolationDelay for client {id:?}");
            return;
        };
        if let Some(hit_data) = query.cast_ray(
            // the delay is sent in every input message; the latest InterpolationDelay received
            // is stored on the client entity
            *delay,
            position.0,
            Dir2::new_unchecked(velocity.0.normalize()),
            // TODO: shouldn't this be based on velocity length?
            BULLET_COLLISION_DISTANCE_CHECK,
            false,
            &mut SpatialQueryFilter::default(),
        ) {
            info!(?tick, ?hit_data, ?entity, "Despawn bullet");
            // if there is a hit, increment the score
            player_query
                .iter_mut()
                .find(|(_, player_id)| player_id.0 == id.0)
                .map(|(mut score, _)| {
                    score.0 += 1;
                });
            commands.entity(entity).despawn();
        }
    })
}

pub(crate) fn compute_hit_prediction(
    mut commands: Commands,
    tick_manager: Res<TickManager>,
    query: SpatialQuery,
    bullets: Query<(Entity, &PlayerId, &Position, &LinearVelocity), With<BulletMarker>>,
    bot_query: Query<(), (With<PredictedBot>, Without<Confirmed>)>,
    manager: Res<ConnectionManager>,
    // the InterpolationDelay component is stored directly on the client entity
    // (the server creates one entity for each client to store client-specific
    // metadata)
    mut player_query: Query<(&mut Score, &PlayerId)>,
) {
    let tick = tick_manager.tick();
    bullets.iter().for_each(|(entity, id, position, velocity)| {
        if let Some(hit_data) = query.cast_ray_predicate(
            position.0,
            Dir2::new_unchecked(velocity.0.normalize()),
            // TODO: shouldn't this be based on velocity length?
            BULLET_COLLISION_DISTANCE_CHECK,
            false,
            &SpatialQueryFilter::default(),
            &|entity| {
                // only confirm the hit on predicted bots
                bot_query.get(entity).is_ok()
            },
        ) {
            info!(?tick, ?hit_data, ?entity, "Despawn bullet");
            // if there is a hit, increment the score
            player_query
                .iter_mut()
                .find(|(_, player_id)| player_id.0 == id.0)
                .map(|(mut score, _)| {
                    score.0 += 1;
                });
            commands.entity(entity).despawn();
        }
    })
}

fn interpolated_bot_movement(
    tick_manager: Res<TickManager>,
    mut query: Query<&mut Position, With<InterpolatedBot>>,
) {
    let tick = tick_manager.tick();
    query.iter_mut().for_each(|mut position| {
        // change direction every 200ticks
        let direction = if (tick.0 / 200) % 2 == 0 { 1.0 } else { -1.0 };
        position.x += shared::BOT_MOVE_SPEED * direction;
    });
}
