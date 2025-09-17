use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_player_movement, BOT_RADIUS};
use avian2d::prelude::*;
use bevy::prelude::*;
use bevy::time::Stopwatch;
use core::ops::DerefMut;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::interpolation::plugin::InterpolationDelay;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_avian2d::prelude::{
    LagCompensationHistory, LagCompensationPlugin, LagCompensationSet, LagCompensationSpatialQuery,
};
use lightyear_examples_common::shared::SEND_INTERVAL;

pub struct ExampleServerPlugin;

const BULLET_COLLISION_DISTANCE_CHECK: f32 = 4.0;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LagCompensationPlugin);
        app.add_systems(Startup, spawn_bots);
        app.add_observer(handle_new_client);
        app.add_observer(spawn_player);
        // the lag compensation systems need to run after LagCompensationSet::UpdateHistory
        app.add_systems(FixedUpdate, interpolated_bot_movement);
        app.add_systems(
            PhysicsSchedule,
            // lag compensation collisions must run after the SpatialQuery has been updated
            compute_hit_lag_compensation.in_set(LagCompensationSet::Collisions),
        );
        app.add_systems(
            FixedPostUpdate,
            // check collisions after physics have run
            compute_hit_prediction.after(PhysicsSet::Sync),
        );
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

// Replicate the pre-spawned entities back to the client
// We have to use `InitialReplicated` instead of `Replicated`, because
// the server has already assumed authority over the entity so the `Replicated` component
// has been removed
pub(crate) fn spawn_player(
    trigger: On<Add, Connected>,
    query: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
    replicated_players: Query<
        (Entity, &InitialReplicated),
        (Added<InitialReplicated>, With<PlayerId>),
    >,
) {
    let Ok(client_id) = query.get(trigger.entity) else {
        return;
    };
    let client_id = client_id.0;
    let y = (client_id.to_bits() as f32 * 50.0) % 500.0 - 250.0;
    let color = color_from_id(client_id);
    info!("Spawning player with id: {}", client_id);
    commands.spawn((
        Replicate::to_clients(NetworkTarget::All),
        PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
        InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
        ControlledBy {
            owner: trigger.entity,
            lifetime: Default::default(),
        },
        Score(0),
        PlayerId(client_id),
        RigidBody::Kinematic,
        Transform::from_xyz(0.0, y, 0.0),
        ColorComponent(color),
        ActionState::<PlayerActions>::default(),
        PlayerMarker,
        Name::new("Player"),
    ));
}

/// Spawn bots (one predicted, one interpolated)
pub(crate) fn spawn_bots(mut commands: Commands) {
    commands.spawn((
        InterpolatedBot,
        Name::new("InterpolatedBot"),
        Replicate::to_clients(NetworkTarget::All),
        InterpolationTarget::to_clients(NetworkTarget::All),
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
        Replicate::to_clients(NetworkTarget::All),
        PredictionTarget::to_clients(NetworkTarget::All),
        // NOTE: all predicted entities must be part of the same replication group!
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
    timeline: Single<&LocalTimeline, With<Server>>,
    query: LagCompensationSpatialQuery,
    bullets: Query<
        (Entity, &PlayerId, &Position, &LinearVelocity, &ControlledBy),
        With<BulletMarker>,
    >,
    // the InterpolationDelay component is stored directly on the client entity
    // (the server creates one entity for each client to store client-specific
    // metadata)
    client_query: Query<&InterpolationDelay, With<ClientOf>>,
    mut player_query: Query<(&mut Score, &PlayerId), With<PlayerMarker>>,
) {
    let tick = timeline.tick();
    bullets
        .iter()
        .for_each(|(entity, id, position, velocity, controlled_by)| {
            let Ok(delay) = client_query.get(controlled_by.owner) else {
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
                info!(
                    ?tick,
                    ?hit_data,
                    ?entity,
                    "Collision with interpolated bot! Despawning bullet"
                );
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
    timeline: Single<&LocalTimeline, With<Server>>,
    query: SpatialQuery,
    bullets: Query<(Entity, &PlayerId, &Position, &LinearVelocity), With<BulletMarker>>,
    bot_query: Query<(), (With<PredictedBot>, Without<Confirmed>)>,
    // the InterpolationDelay component is stored directly on the client entity
    // (the server creates one entity for each client to store client-specific
    // metadata)
    mut player_query: Query<(&mut Score, &PlayerId), With<PlayerMarker>>,
) {
    let tick = timeline.tick();
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
            info!(
                ?tick,
                ?hit_data,
                ?entity,
                "Collision with predicted bot! Despawn bullet"
            );
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
    timeline: Single<&LocalTimeline, With<Server>>,
    mut query: Query<&mut Position, With<InterpolatedBot>>,
) {
    let tick = timeline.tick();
    query.iter_mut().for_each(|mut position| {
        // change direction every 200ticks
        let direction = if (tick.0 / 200) % 2 == 0 { 1.0 } else { -1.0 };
        position.x += shared::BOT_MOVE_SPEED * direction;
    });
}
