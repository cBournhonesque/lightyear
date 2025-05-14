use avian2d::prelude::*;
use avian2d::PhysicsPlugins;
use bevy::diagnostic::LogDiagnosticsPlugin;
use bevy::prelude::*;
use bevy::time::Stopwatch;
use core::ops::DerefMut;
use core::time::Duration;
use leafwing_input_manager::prelude::ActionState;
use lightyear::connection::client_of::ClientOf;
use lightyear::prediction::prespawn::PreSpawned;
use lightyear::prelude::*;

use crate::protocol::*;

const EPS: f32 = 0.0001;
pub const BOT_RADIUS: f32 = 15.0;
pub(crate) const BOT_MOVE_SPEED: f32 = 1.0;
const BULLET_MOVE_SPEED: f32 = 300.0;
const MAP_LIMIT: f32 = 2000.0;

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(lightyear_avian::LightyearAvianPlugin);
        app.add_plugins(ProtocolPlugin);
        // registry types for reflection
        app.register_type::<PlayerId>();
        // debug systems
        // app.add_systems(FixedLast, fixed_update_log);
        // app.add_systems(FixedLast, log_predicted_bot_transform);

        // every system that is physics-based and can be rolled-back has to be in the `FixedUpdate` schedule
        app.add_systems(
            FixedUpdate,
            (predicted_bot_movement, player_movement, shoot_bullet).chain(),
        );
        // both client and server need physics
        // (the client also needs the physics plugin to be able to compute predicted
        //  bullet hits)
        app.add_plugins(
            PhysicsPlugins::default()
                .build()
                .disable::<ColliderHierarchyPlugin>(),
        )
        .insert_resource(Gravity(Vec2::ZERO));
    }
}

// Generate pseudo-random color from id
pub(crate) fn color_from_id(client_id: PeerId) -> Color {
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
    timeline: Single<&LocalTimeline, Without<ClientOf>>,
    mut player_query: Query<
        (&mut Transform, &ActionState<PlayerActions>, &PlayerId),
        Or<(With<Predicted>, With<Replicate>)>,
    >,
) {
    for (transform, action_state, player_id) in player_query.iter_mut() {
        trace!(tick = ?timeline.tick(), action = ?action_state.dual_axis_data(&PlayerActions::MoveCursor), "Data in Movement (FixedUpdate)");
        shared_player_movement(transform, action_state);
        // info!(tick = ?tick_manager.tick(), ?transform, actions = ?action_state.get_pressed(), "applying movement to predicted player");
    }
}

fn predicted_bot_movement(
    timeline: Single<&LocalTimeline, Without<ClientOf>>,
    mut query: Query<&mut Position, (With<PredictedBot>, Or<(With<Predicted>, With<Replicating>)>)>,
) {
    let tick = timeline.tick();
    query.iter_mut().for_each(|mut position| {
        let direction = if (tick.0 / 200) % 2 == 0 { 1.0 } else { -1.0 };
        position.x += BOT_MOVE_SPEED * direction;
    });
}

fn log_predicted_bot_transform(
    timeline: Single<(&LocalTimeline, Has<Rollback>), Without<ClientOf>>,
    query: Query<
        (&Position, &Transform),
        (With<PredictedBot>, Or<(With<Predicted>, With<Replicating>)>),
    >,
) {
    let (timeline, is_rollback) = timeline.into_inner();
    let tick = timeline.tick();
    query.iter().for_each(|(pos, transform)| {
        info!(?tick, ?pos, ?transform, "PredictedBot FixedLast");
    })
}

pub(crate) fn fixed_update_log(
    timeline: Single<(&LocalTimeline, Has<Rollback>), Without<ClientOf>>,
    player: Query<(Entity, &Transform), (With<PlayerId>, Without<Confirmed>)>,
    ball: Query<(Entity, &Transform), (With<BulletMarker>, Without<Confirmed>)>,
    interpolated_ball: Query<(Entity, &Transform), (With<BulletMarker>, With<Interpolated>)>,
) {
    let (timeline, is_rollback) = timeline.into_inner();
    let tick = timeline.tick();
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

/// This system runs on both the client and the server, and is used to shoot a bullet
/// The bullet is shot from the predicted player on the client, and from the server-entity on the server.
/// When the bullet is replicated from server to client, it will use the existing client bullet with the `PreSpawned` component
/// as its `Predicted` entity
pub(crate) fn shoot_bullet(
    mut commands: Commands,
    timeline: Single<&LocalTimeline, Without<ClientOf>>,
    mut query: Query<
        (
            &PlayerId,
            &Transform,
            &ColorComponent,
            &mut ActionState<PlayerActions>,
            Option<&ControlledBy>,
        ),
        Or<(With<Predicted>, With<Replicate>)>,
    >,
) {
    let tick = timeline.tick();
    for (id, transform, color, action, controlled_by) in query.iter_mut() {
        let is_server = controlled_by.is_some();
        // NOTE: pressed lets you shoot many bullets, which can be cool
        if action.just_pressed(&PlayerActions::Shoot) {
            error!(?tick, pos=?transform.translation.truncate(), rot=?transform.rotation.to_euler(EulerRot::XYZ).2, "spawn bullet");
            for delta in [-0.2, 0.2] {
                let salt: u64 = if delta < 0.0 { 0 } else { 1 };
                // shoot from the position of the player, towards the cursor, with an angle of delta
                let mut bullet_transform = transform.clone();
                bullet_transform.rotate_z(delta);
                let bullet_bundle = (
                    bullet_transform,
                    LinearVelocity(bullet_transform.up().as_vec3().truncate() * BULLET_MOVE_SPEED),
                    RigidBody::Kinematic,
                    // store the player who fired the bullet
                    *id,
                    *color,
                    BulletMarker,
                    Name::new("Bullet"),
                );

                // on the server, replicate the bullet
                if is_server {
                    #[cfg(feature = "server")]
                    commands.spawn((
                        bullet_bundle,
                        // NOTE: the PreSpawned component indicates that the entity will be spawned on both client and server
                        //  but the server will take authority as soon as the client receives the entity
                        //  it does this by matching with the client entity that has the same hash
                        //  The hash is computed automatically in PostUpdate from the entity's components + spawn tick
                        //  unless you set the hash manually before PostUpdate to a value of your choice
                        //
                        // the default hashing algorithm uses the tick and component list. in order to disambiguate
                        // between the two bullets, we add additional information to the hash.
                        // NOTE: if you don't add the salt, the 'left' bullet on the server might get matched with the
                        // 'right' bullet on the client, and vice versa. This is not critical, but it will cause a rollback
                        PreSpawned::default_with_salt(salt),
                        Replicate::to_clients(NetworkTarget::All),
                        PredictionTarget::to_clients(NetworkTarget::Single(id.0)),
                        InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                        controlled_by.unwrap().clone(),
                        // NOTE: all predicted entities need to have the same replication group
                        ReplicationGroup::new_id(id.0.to_bits()),
                    ));
                } else {
                    // on the client, just spawn the ball
                    // NOTE: the PreSpawned component indicates that the entity will be spawned on both client and server
                    //  but the server will take authority as soon as the client receives the entity
                    commands.spawn((bullet_bundle, PreSpawned::default_with_salt(salt)));
                }
            }
        }
    }
}
