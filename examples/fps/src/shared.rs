use crate::protocol::*;
use avian2d::prelude::*;
use avian2d::PhysicsPlugins;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::ActionState;
use lightyear::connection::client_of::ClientOf;
use lightyear::connection::host::HostServer;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
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
        crate::debug::register_debug_systems(app);

        app.add_plugins(lightyear::avian2d::plugin::LightyearAvianPlugin {
            replication_mode: AvianReplicationMode::PositionButInterpolateTransform,
            ..default()
        });

        app.add_systems(PreUpdate, despawn_after);

        // Physics-based systems that can roll back must run in FixedUpdate.
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

// Generates a pseudo-random color from the peer id.
pub(crate) fn color_from_id(client_id: PeerId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(90)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

// Applies movement input to a player position and rotation.
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
    // Only trigger change detection when the rotation actually changes.
    if (angle - rotation.as_radians()).abs() > EPS {
        *rotation = Rotation::from(angle);
    }
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

fn bullet_prespawn_hash(player_id: PeerId, tick: Tick) -> u64 {
    player_id.to_bits().wrapping_mul(1_000_003) ^ tick.0 as u64
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

// Apply local input only to predicted entities owned by this client.
//
// If this example predicted remote entities, ownership would need to be checked before movement.
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

/// Spawns bullets on both clients and servers.
///
/// Clients spawn predicted bullets locally; the server spawns authoritative bullets from the
/// server-owned player entity. `PreSpawned` lets replicated bullets match the local prediction.
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
        let is_server = cfg!(feature = "server") && controlled_by.is_some();
        let should_shoot = if is_host_server || !is_server {
            shoot_pressed_this_tick(input_buffer, tick)
        } else {
            action.just_pressed(&PlayerActions::Shoot)
        };
        if should_shoot {
            let prespawn_hash = bullet_prespawn_hash(id.0, tick);
            let bullet_position = *position;
            let bullet_rotation = *rotation;
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
                *id,
                *color,
                BulletMarker::new(id.0, tick, prespawn_hash),
                Name::new("Bullet"),
            );

            #[cfg(feature = "server")]
            let bullet_entity = if let Some(controlled_by) = controlled_by.cloned() {
                commands
                    .spawn((
                        bullet_bundle,
                        // PreSpawned lets the server match this authoritative bullet with
                        // the client's predicted bullet that used the same hash.
                        PreSpawned::new(prespawn_hash),
                        DespawnAfter(Timer::new(Duration::from_secs(2), TimerMode::Once)),
                        Replicate::to_clients(NetworkTarget::All),
                        PredictionTarget::to_clients(NetworkTarget::Single(id.0)),
                        InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(id.0)),
                        controlled_by,
                    ))
                    .id()
            } else {
                commands
                    .spawn((bullet_bundle, PreSpawned::new(prespawn_hash)))
                    .id()
            };
            #[cfg(not(feature = "server"))]
            let bullet_entity = {
                commands
                    .spawn((bullet_bundle, PreSpawned::new(prespawn_hash)))
                    .id()
            };
            lightyear_debug_event!(
                DebugCategory::Prediction,
                DebugSamplePoint::FixedUpdate,
                "FixedUpdate",
                "bullet_spawn",
                local_tick = tick.0 as i64,
                entity = ?bullet_entity,
                client_id = ?id.0,
                shooter = ?id.0,
                shooter_bits = id.0.to_bits(),
                fire_tick = tick.0 as i64,
                prespawn_hash = prespawn_hash,
                is_server = is_server,
                is_client_spawn = !is_server,
                position = ?bullet_position,
                rotation = ?bullet_rotation,
                velocity = ?bullet_velocity,
                "Spawn bullet"
            );
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
