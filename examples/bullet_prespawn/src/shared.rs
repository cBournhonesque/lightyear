use bevy::diagnostic::LogDiagnosticsPlugin;
use bevy::prelude::*;
use bevy::utils::Duration;
use leafwing_input_manager::prelude::ActionState;

use lightyear::client::prediction::plugin::is_in_rollback;
use lightyear::prelude::client::*;
use lightyear::prelude::server::{Replicate, ReplicationTarget, SyncTarget};
use lightyear::prelude::TickManager;
use lightyear::prelude::*;
use lightyear::shared::plugin::Identity;
use lightyear::transport::io::IoDiagnosticsPlugin;

use crate::protocol::*;

const EPS: f32 = 0.0001;

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        // registry types for reflection
        app.register_type::<PlayerId>();
        app.add_systems(FixedPostUpdate, fixed_update_log);
        // every system that is physics-based and can be rolled-back has to be in the `FixedUpdate` schedule
        app.add_systems(
            FixedUpdate,
            // ideally, during rollback, we'd despawn the pre-predicted player objects and then respawn them during shoot_bullet.
            // how? we keep track of their spawn-tick, if it was before the rollback-tick we despawn.
            //  - for every pre-predicted or pre-spawned entity, we keep track of the spawn tick.
            //  - if we rollback to before that, we
            (
                player_movement,
                shoot_bullet,
                // avoid re-shooting bullets during rollbacks
                // shoot_bullet.run_if(not(is_in_rollback)),
                move_bullet,
            )
                .chain(),
        );
        // NOTE: we need to create prespawned entities in FixedUpdate, because only then are inputs correctly associated with a tick
        //  Example:
        //  tick = 0
        //   F1 PreUpdate: press-shoot. F1 FixedUpdate: SKIPPED!!! F1 Update: spawn bullet F1: PostUpdate add hash.
        //   F2 FixedUpdate: tick = 1. Gather inputs for the tick (i.e. F1 preupdate + F2 preupdate)
        //  So now the server will think that the bullet was shot at tick = 1, but on client it was shot on tick = 0 and the hashes won't match.
        //  In general, most input-handling needs to be handled in FixedUpdate to be correct.
        // app.add_systems(Update, shoot_bullet);
    }
}

// Generate pseudo-random color from id
pub(crate) fn color_from_id(client_id: ClientId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(90)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_player_movement(
    mut transform: Mut<Transform>,
    action: &ActionState<PlayerActions>,
) {
    const PLAYER_MOVE_SPEED: f32 = 10.0;
    // warn!(?action, "action state");
    let Some(cursor_data) = action.dual_axis_data(&PlayerActions::MoveCursor) else {
        return;
    };
    // warn!(?mouse_position);
    let angle = Vec2::new(0.0, 1.0).angle_to(cursor_data.pair - transform.translation.truncate());
    // careful to only activate change detection if there was an actual change
    if (angle - transform.rotation.to_euler(EulerRot::XYZ).2).abs() > EPS {
        transform.rotation = Quat::from_rotation_z(angle);
    }
    // TODO: look_at should work
    // transform.look_at(Vec3::new(mouse_position.x, mouse_position.y, 0.0), Vec3::Y);
    if action.pressed(&PlayerActions::Up) {
        transform.translation.y += PLAYER_MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Down) {
        transform.translation.y -= PLAYER_MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Right) {
        transform.translation.x += PLAYER_MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Left) {
        transform.translation.x -= PLAYER_MOVE_SPEED;
    }
}

// The client input only gets applied to predicted entities that we own
// This works because we only predict the user's controlled entity.
// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(
    tick_manager: Res<TickManager>,
    mut player_query: Query<
        (&mut Transform, &ActionState<PlayerActions>, &PlayerId),
        Or<(With<Predicted>, With<ReplicationTarget>)>,
    >,
) {
    for (transform, action_state, player_id) in player_query.iter_mut() {
        // info!(tick = ?tick_manager.tick(), action = ?action_state.dual_axis_data(&PlayerActions::MoveCursor), "Data in Movement (FixedUpdate)");
        shared_player_movement(transform, action_state);
        // info!(tick = ?tick_manager.tick(), ?transform, actions = ?action_state.get_pressed(), "applying movement to predicted player");
    }
}

pub(crate) fn fixed_update_log(
    tick_manager: Res<TickManager>,
    rollback: Option<Res<Rollback>>,
    player: Query<(Entity, &Transform), (With<PlayerId>, Without<Confirmed>)>,
    ball: Query<(Entity, &Transform), (With<BallMarker>, Without<Confirmed>)>,
    interpolated_ball: Query<(Entity, &Transform), (With<BallMarker>, With<Interpolated>)>,
) {
    let tick = rollback.map_or(tick_manager.tick(), |r| {
        tick_manager.tick_or_rollback_tick(r.as_ref())
    });
    for (entity, transform) in player.iter() {
        trace!(
        ?tick,
        ?entity,
        pos = ?transform.translation.truncate(),
        "Player after fixed update"
        );
    }
    for (entity, transform) in ball.iter() {
        trace!(
            ?tick,
            ?entity,
            pos = ?transform.translation.truncate(),
            "Ball after fixed update"
        );
    }
    for (entity, transform) in interpolated_ball.iter() {
        trace!(
            ?tick,
            ?entity,
            pos = ?transform.translation.truncate(),
            "interpolated Ball after fixed update"
        );
    }
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn move_bullet(
    mut commands: Commands,
    mut query: Query<
        (Entity, &mut Transform),
        (
            With<BallMarker>,
            Or<(
                // move predicted bullets
                With<Predicted>,
                // move server entities
                With<ReplicationTarget>,
                // move prespawned bullets
                With<PreSpawnedPlayerObject>,
            )>,
        ),
    >,
) {
    const BALL_MOVE_SPEED: f32 = 3.0;
    const MAP_LIMIT: f32 = 2000.0;
    for (entity, mut transform) in query.iter_mut() {
        let movement_direction = transform.rotation * Vec3::Y;
        transform.translation += movement_direction * BALL_MOVE_SPEED;
        // destroy bullets that are out of the screen
        if transform.translation.x.abs() > MAP_LIMIT || transform.translation.y.abs() > MAP_LIMIT {
            // TODO: use the predicted despawn?
            commands.entity(entity).despawn();
        }
    }
}

/// This system runs on both the client and the server, and is used to shoot a bullet
/// The bullet is shot from the predicted player on the client, and from the server-entity on the server.
/// When the bullet is replicated from server to client, it will use the existing client bullet with the `PreSpawnedPlayerObject` component
/// as its `Predicted` entity
pub(crate) fn shoot_bullet(
    mut commands: Commands,
    tick_manager: Res<TickManager>,
    identity: NetworkIdentity,
    mut query: Query<
        (
            &PlayerId,
            &Transform,
            &ColorComponent,
            &mut ActionState<PlayerActions>,
        ),
        Or<(With<Predicted>, With<ReplicationTarget>)>,
    >,
) {
    let tick = tick_manager.tick();
    const BALL_MOVE_SPEED: f32 = 10.0;
    for (id, transform, color, action) in query.iter_mut() {
        // NOTE: cannot spawn the bullet during FixedUpdate because then during rollback we spawn a new bullet! For now just set the system to
        //  run when not in rollback

        // NOTE: pressed lets you shoot many bullets, which can be cool
        if action.just_pressed(&PlayerActions::Shoot) {
            error!(?tick, pos=?transform.translation.truncate(), rot=?transform.rotation.to_euler(EulerRot::XYZ).2, "spawn bullet");

            for delta in &[-0.2, 0.2] {
                let salt: u64 = if delta < &0.0 { 0 } else { 1 };
                let ball = BallBundle::new(
                    transform.translation.truncate(),
                    transform.rotation.to_euler(EulerRot::XYZ).2 + delta,
                    color.0,
                );
                // on the server, replicate the bullet
                if identity.is_server() {
                    commands.spawn((
                        ball,
                        // NOTE: the PreSpawnedPlayerObject component indicates that the entity will be spawned on both client and server
                        //  but the server will take authority as soon as the client receives the entity
                        //  it does this by matching with the client entity that has the same hash
                        //  The hash is computed automatically in PostUpdate from the entity's components + spawn tick
                        //  unless you set the hash manually before PostUpdate to a value of your choice
                        //
                        // the default hashing algorithm uses the tick and component list. in order to disambiguate
                        // between the two bullets, we add additional information to the hash.
                        // NOTE: if you don't add the salt, the 'left' bullet on the server might get matched with the
                        // 'right' bullet on the client, and vice versa. This is not critical, but it will cause a rollback
                        PreSpawnedPlayerObject::default_with_salt(salt),
                        Replicate {
                            sync: SyncTarget {
                                // the bullet is predicted for the client who shot it
                                prediction: NetworkTarget::Single(id.0),
                                // the bullet is interpolated for other clients
                                interpolation: NetworkTarget::AllExceptSingle(id.0),
                            },
                            // NOTE: all predicted entities need to have the same replication group
                            group: ReplicationGroup::new_id(id.0.to_bits()),
                            ..default()
                        },
                    ));
                } else {
                    // on the client, just spawn the ball
                    // NOTE: the PreSpawnedPlayerObject component indicates that the entity will be spawned on both client and server
                    //  but the server will take authority as soon as the client receives the entity
                    commands.spawn((ball, PreSpawnedPlayerObject::default_with_salt(salt)));
                }
            }
        }
    }
}
