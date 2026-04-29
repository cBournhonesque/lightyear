use crate::protocol::*;
use avian2d::prelude::*;
use avian2d::PhysicsPlugins;
use bevy::diagnostic::LogDiagnosticsPlugin;
use bevy::prelude::*;
use bevy::time::Stopwatch;
use core::ops::DerefMut;
use core::time::Duration;
use leafwing_input_manager::prelude::ActionState;
use lightyear::connection::client_of::ClientOf;
use lightyear::connection::host::HostServer;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prediction::plugin::PredictionSystems;
use lightyear::prediction::predicted_history::PredictionHistory;
use lightyear::prelude::*;
use lightyear_avian2d::plugin::AvianReplicationMode;

const EPS: f32 = 0.0001;
pub const BOT_RADIUS: f32 = 15.0;
pub(crate) const BOT_MOVE_SPEED: f32 = 1.0;
const BULLET_MOVE_SPEED: f32 = 300.0;
const MAP_LIMIT: f32 = 2000.0;

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);

        app.add_plugins(lightyear::avian2d::plugin::LightyearAvianPlugin {
            replication_mode: AvianReplicationMode::PositionButInterpolateTransform,
            ..default()
        });

        app.add_systems(PreUpdate, despawn_after);

        // debug systems
        app.add_systems(FixedLast, emit_fixed_last_entities);
        app.add_systems(FixedLast, emit_predicted_bot_transform);

        // every system that is physics-based and can be rolled-back has to be in the `FixedUpdate` schedule
        app.add_systems(
            FixedUpdate,
            (predicted_bot_movement, player_movement, shoot_bullet).chain(),
        );
        // both client and server need physics
        // (the client also needs the physics plugin to be able to compute predicted bullet hits)
        app.add_plugins(
            PhysicsPlugins::default()
                .build()
                // disable syncing position<>transform as it is handled by lightyear_avian
                .disable::<PhysicsTransformPlugin>(),
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
    mut position: Mut<Position>,
    mut rotation: Mut<Rotation>,
    action: &ActionState<PlayerActions>,
) {
    const PLAYER_MOVE_SPEED: f32 = 10.0;
    let Some(cursor_data) = action.dual_axis_data(&PlayerActions::MoveCursor) else {
        return;
    };
    let angle = Vec2::new(0.0, 1.0).angle_to(cursor_data.pair - position.0);
    // careful to only activate change detection if there was an actual change
    if (angle - rotation.as_radians()).abs() > EPS {
        *rotation = Rotation::from(angle);
    }
    // TODO: look_at should work
    // transform.look_at(Vec3::new(mouse_position.x, mouse_position.y, 0.0), Vec3::Y);
    if action.pressed(&PlayerActions::Up) {
        position.y += PLAYER_MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Down) {
        position.y -= PLAYER_MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Right) {
        position.x += PLAYER_MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Left) {
        position.x -= PLAYER_MOVE_SPEED;
    }
}

fn bullet_prespawn_hash(player_id: PeerId, tick: Tick, salt: u64) -> u64 {
    let mut hash = 0xd1b5_4a32_d192_ed03_u64;
    hash ^= player_id.to_bits().wrapping_mul(0x9e37_79b9_7f4a_7c15);
    hash = hash.rotate_left(27).wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^= (tick.0 as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash = hash.rotate_left(31).wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^ salt.wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

fn should_skip_client_side_entity(
    has_client: bool,
    is_host_server: bool,
    is_predicted: bool,
    client_is_synced: bool,
) -> bool {
    if !has_client {
        return false;
    }
    if is_predicted && !client_is_synced {
        return true;
    }
    !is_host_server && !is_predicted
}

// The client input only gets applied to predicted entities that we own
// This works because we only predict the user's controlled entity.
// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(
    timeline: Res<LocalTimeline>,
    client: Query<(), With<Client>>,
    host_server: Query<(), With<HostServer>>,
    synced_client: Query<(), (With<Client>, With<IsSynced<InputTimeline>>)>,
    mut player_query: Query<
        (
            &mut Position,
            &mut Rotation,
            &ActionState<PlayerActions>,
            &PlayerId,
            Has<Predicted>,
        ),
        (Or<(With<Predicted>, With<Replicate>)>, With<PlayerMarker>),
    >,
) {
    let has_client = !client.is_empty();
    let is_host_server = !host_server.is_empty();
    let client_is_synced = !synced_client.is_empty();
    for (position, rotation, action_state, player_id, is_predicted) in player_query.iter_mut() {
        if should_skip_client_side_entity(
            has_client,
            is_host_server,
            is_predicted,
            client_is_synced,
        ) {
            continue;
        }
        debug!(tick = ?timeline.tick(), action = ?action_state.dual_axis_data(&PlayerActions::MoveCursor), "Data in Movement (FixedUpdate)");
        shared_player_movement(position, rotation, action_state);
    }
}

fn predicted_bot_movement(
    timeline: Res<LocalTimeline>,
    mut query: Query<&mut Position, With<PredictedBot>>,
) {
    let tick = timeline.tick();
    query.iter_mut().for_each(|mut position| {
        let direction = if (tick.0 / 200) % 2 == 0 { 1.0 } else { -1.0 };
        position.x += BOT_MOVE_SPEED * direction;
    });
}

fn emit_predicted_bot_transform(
    timeline: Res<LocalTimeline>,
    query: Query<(&Position, &Transform), With<PredictedBot>>,
) {
    let tick = timeline.tick();
    query.iter().for_each(|(pos, transform)| {
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::FixedLast,
            "FixedLast",
            "predicted_bot_transform",
            tick = ?tick,
            position = ?pos,
            transform = ?transform,
            "PredictedBot FixedLast"
        );
    })
}

pub(crate) fn emit_fixed_last_entities(
    timeline: Res<LocalTimeline>,
    player: Query<(Entity, &Transform), (With<PlayerMarker>, With<PlayerId>)>,
    predicted_bullet: Query<
        (
            Entity,
            &Position,
            &Transform,
            Option<&PredictionHistory<Transform>>,
        ),
        With<BulletMarker>,
    >,
) {
    let tick = timeline.tick();
    for (entity, transform) in player.iter() {
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::FixedLast,
            "FixedLast",
            "player_transform",
            tick = ?tick,
            entity = ?entity,
            pos = ?transform.translation.truncate(),
            "Player after fixed update"
        );
    }
    for (entity, position, transform, history) in predicted_bullet.iter() {
        lightyear_debug_event!(
            DebugCategory::Prediction,
            DebugSamplePoint::FixedLast,
            "FixedLast",
            "bullet_transform_history",
            tick = ?tick,
            entity = ?entity,
            position = ?position,
            transform = ?transform.translation.truncate(),
            history = ?history,
            "Bullet after fixed update"
        );
    }
}

/// This system runs on both the client and the server, and is used to shoot a bullet
/// The bullet is shot from the predicted player on the client, and from the server-entity on the server.
/// When the bullet is replicated from server to client, it will use the existing client bullet with the `PreSpawned` component
/// as its `Predicted` entity
pub(crate) fn shoot_bullet(
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    client: Query<(), With<Client>>,
    host_server: Query<(), With<HostServer>>,
    synced_client: Query<(), (With<Client>, With<IsSynced<InputTimeline>>)>,
    mut query: Query<
        (
            &PlayerId,
            &Position,
            &Rotation,
            &ColorComponent,
            &ActionState<PlayerActions>,
            &LeafwingBuffer<PlayerActions>,
            Option<&ControlledBy>,
            Has<Predicted>,
        ),
        (Or<(With<Predicted>, With<Replicate>)>, With<PlayerMarker>),
    >,
) {
    let tick = timeline.tick();
    let has_client = !client.is_empty();
    let is_host_server = !host_server.is_empty();
    let client_is_synced = !synced_client.is_empty();
    for (id, position, rotation, color, action, input_buffer, controlled_by, is_predicted) in
        query.iter_mut()
    {
        if should_skip_client_side_entity(
            has_client,
            is_host_server,
            is_predicted,
            client_is_synced,
        ) {
            continue;
        }
        let is_server = controlled_by.is_some();
        let should_shoot = if is_host_server || !is_server {
            shoot_pressed_this_tick(input_buffer, tick)
        } else {
            action.just_pressed(&PlayerActions::Shoot)
        };
        if should_shoot {
            // for delta in [-0.2, 0.2] {
            for delta in [0.0] {
                let salt: u64 = if delta < 0.0 { 0 } else { 1 };
                let prespawn_hash = bullet_prespawn_hash(id.0, tick, salt);
                // shoot from the position of the player, towards the cursor, with an angle of delta
                let bullet_position = *position;
                let bullet_rotation = Rotation::from(rotation.as_radians() + delta);
                let bullet_transform = Transform::from_translation(bullet_position.0.extend(0.0))
                    .with_rotation(Quat::from_rotation_z(bullet_rotation.as_radians()));
                let bullet_velocity = LinearVelocity(bullet_rotation * Vec2::Y * BULLET_MOVE_SPEED);
                debug!(?tick, pos=?bullet_transform.translation.truncate(), rot=?bullet_transform.rotation.to_euler(EulerRot::XYZ).2, "spawn bullet");
                let bullet_bundle = (
                    bullet_position,
                    bullet_rotation,
                    bullet_transform,
                    bullet_velocity,
                    RigidBody::Kinematic,
                    // store the player who fired the bullet
                    *id,
                    *color,
                    BulletMarker,
                    Name::new("Bullet"),
                );

                // on the server, replicate the bullet
                let bullet_entity = if is_server {
                    #[cfg(feature = "server")]
                    {
                        commands
                            .spawn((
                                bullet_bundle,
                                // NOTE: the PreSpawned component indicates that the entity will be spawned on both client and server
                                //  but the server will take authority as soon as the client receives the entity
                                //  it does this by matching with the client entity that has the same hash
                                //  Use an explicit hash so same-tick bullets from different players cannot collide.
                                PreSpawned::new(prespawn_hash),
                                DespawnAfter(Timer::new(Duration::from_secs(2), TimerMode::Once)),
                                Replicate::to_clients(NetworkTarget::All),
                                PredictionTarget::to_clients(NetworkTarget::Single(id.0)),
                                InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(
                                    id.0,
                                )),
                                controlled_by.unwrap().clone(),
                            ))
                            .id()
                    }
                    #[cfg(not(feature = "server"))]
                    {
                        unreachable!("server bullet spawn requires the server feature")
                    }
                } else {
                    // on the client, just spawn the ball
                    // NOTE: the PreSpawned component indicates that the entity will be spawned on both client and server
                    //  but the server will take authority as soon as the client receives the entity
                    commands
                        .spawn((bullet_bundle, PreSpawned::new(prespawn_hash)))
                        .id()
                };
                lightyear_debug_event!(
                    DebugCategory::Prediction,
                    DebugSamplePoint::FixedUpdate,
                    "FixedUpdate",
                    "bullet_spawn",
                    tick = ?tick,
                    entity = ?bullet_entity,
                    client_id = ?id.0,
                    prespawn_hash = prespawn_hash,
                    is_server = is_server,
                    position = ?bullet_position,
                    rotation = ?bullet_rotation,
                    velocity = ?bullet_velocity,
                    "Spawn bullet"
                );
            }
        }
    }
}

fn shoot_pressed_this_tick(input_buffer: &LeafwingBuffer<PlayerActions>, tick: Tick) -> bool {
    let current_pressed = input_buffer
        .get(tick)
        .is_some_and(|snapshot| snapshot.0.pressed(&PlayerActions::Shoot));
    let previous_pressed = input_buffer
        .get(tick - 1)
        .is_some_and(|snapshot| snapshot.0.pressed(&PlayerActions::Shoot));
    current_pressed && !previous_pressed
}

#[derive(Component)]
struct DespawnAfter(pub Timer);

/// Despawn entities after their timer has finished
fn despawn_after(
    time: Res<Time>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut DespawnAfter)>,
) {
    for (entity, mut despawn_after) in query.iter_mut() {
        despawn_after.0.tick(time.delta());
        if despawn_after.0.is_finished() {
            commands.entity(entity).despawn();
        }
    }
}
