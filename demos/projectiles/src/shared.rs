//! Small pieces of simulation that are genuinely shared by client and server.
//!
//! Axis-specific behavior belongs in the four axis directories. This module is
//! intentionally limited to player movement, firing cadence, dispatching a
//! shot to the selected representation, and common physics setup.

use avian2d::prelude::*;
use bevy::ecs::relationship::Relationship;
use bevy::prelude::*;
use bevy_enhanced_input::action::Action;
use bevy_enhanced_input::prelude::*;
use core::time::Duration;
use lightyear::core::tick::TickDuration;
use lightyear::prelude::*;
use lightyear_avian2d::plugin::AvianReplicationMode;
use serde::{Deserialize, Serialize};

use crate::protocol::*;
use crate::representation::{
    RepresentationKind, fire_data_entity, prespawn_hash, shot_buffer, state_entity,
};
use crate::timeline::TimelinePolicy;
use crate::trajectory::{TrajectoryKind, hitscan, linear};

const ROTATION_EPSILON: f32 = 0.0001;
/// Player speed in world units per second.
///
/// Keeping movement in `LinearVelocity` gives Avian's velocity-aware remote
/// interpolation the same motion information that produced each position.
/// The old code moved 1.5 units per 64 Hz tick, which is 96 units per second.
const PLAYER_MOVE_SPEED: f32 = 96.0;

pub(crate) const PLAYER_SIZE: f32 = 40.0;

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);

        app.add_observer(apply_player_movement);
        app.add_observer(stop_player_movement);
        app.add_observer(shoot_weapon);

        // Aim is continuous state, so read it after BEI applies the buffered
        // input for this tick. Movement uses BEI's Fire/Complete events above;
        // this is the same path on a predicting client and the server.
        app.add_systems(
            FixedPreUpdate,
            apply_player_aim.after(EnhancedInputSystems::Apply),
        );

        // Fire-data entities are network facts. Every peer turns those facts
        // into local projectile entities in one easy-to-find system.
        app.add_systems(PreUpdate, fire_data_entity::materialize);
        app.add_observer(fire_data_entity::despawn_parent);

        // A shot buffer is a networked stream on the player. Consumers create
        // ordinary local projectile entities from newly visible records.
        app.add_systems(
            FixedUpdate,
            (
                shot_buffer::reconcile_materialized,
                shot_buffer::materialize,
                shot_buffer::apply_authoritative_finishes,
            )
                .chain(),
        );

        // Capture the start of the actual movement segment before Avian moves
        // linear projectiles this tick.
        app.add_systems(FixedPreUpdate, linear::capture_projectile_sweep_start);
        app.add_systems(FixedUpdate, hitscan::update_visuals);
        app.add_systems(PreUpdate, despawn_after);
        app.add_systems(FixedLast, despawn_at_tick);

        crate::debug::register_debug_systems(app);

        // Both client and server simulate local projectile presentation. The
        // server additionally uses Avian for authoritative collision queries.
        app.add_plugins(lightyear::avian2d::plugin::LightyearAvianPlugin {
            replication_mode: AvianReplicationMode::Position {
                sync_to_transform: false,
            },
            ..default()
        });
        app.add_plugins(
            PhysicsPlugins::default()
                .build()
                .disable::<PhysicsTransformPlugin>()
                .disable::<PhysicsInterpolationPlugin>()
                .disable::<IslandPlugin>()
                .disable::<IslandSleepingPlugin>(),
        )
        .insert_resource(Gravity(Vec2::ZERO));
    }
}

pub(crate) fn color_from_id(client_id: PeerId) -> Color {
    let hue = (((client_id.to_bits().wrapping_mul(90)) % 360) as f32) / 360.0;
    Color::hsl(hue, 1.0, 0.5)
}

fn rotation_towards(position: Vec2, target: Vec2) -> Option<Rotation> {
    let aim_direction = (target - position).try_normalize()?;
    let angle = Vec2::Y.angle_to(aim_direction);
    angle.is_finite().then(|| Rotation::from(angle))
}

// In host-client mode an authoritative player can also be the host's
// interpolated presentation entity. `ControlledBy` keeps that shared server
// entity in the simulation while remote clients still ignore their
// presentation-only replicas.
type SimulatedPlayer = (
    With<PlayerMarker>,
    Or<(With<ControlledBy>, Without<Interpolated>)>,
);

/// Apply the current continuous aim state once per fixed tick.
///
/// BEI emits `Fire<MoveCursor>` every tick while the action remains fired, so
/// an aim observer would pay synchronous observer-dispatch overhead every tick
/// and could not run in parallel with other systems. A scheduled system is a
/// better fit for this continuously updated state.
///
/// Bevy does not provide relative ordering between observers. In this example,
/// BEI triggers observers such as [`shoot_weapon`] during
/// [`EnhancedInputSystems::Apply`], while this system deliberately runs after
/// that set. Consequently, `shoot_weapon` cannot assume that `Rotation` already
/// contains this tick's aim and explicitly reads `Action<MoveCursor>` itself.
pub(crate) fn apply_player_aim(
    aim_actions: Query<(&ActionOf<PlayerContext>, &Action<MoveCursor>)>,
    mut players: Query<(&mut Rotation, &Position), SimulatedPlayer>,
) {
    for (action_of, aim) in &aim_actions {
        let Ok((mut rotation, position)) = players.get_mut(action_of.get()) else {
            continue;
        };
        let Some(new_rotation) = rotation_towards(position.0, **aim) else {
            continue;
        };
        if (new_rotation.as_radians() - rotation.as_radians()).abs() > ROTATION_EPSILON {
            *rotation = new_rotation;
        }
    }
}

pub(crate) fn apply_player_movement(
    trigger: On<Fire<MovePlayer>>,
    mut players: Query<&mut LinearVelocity, SimulatedPlayer>,
) {
    let Ok(mut velocity) = players.get_mut(trigger.context) else {
        return;
    };
    // Normalizing preserves zero axes and gives cardinal and diagonal
    // movement the same speed. Avian integrates the velocity this tick.
    velocity.0 = trigger.value.normalize_or_zero() * PLAYER_MOVE_SPEED;
}

pub(crate) fn stop_player_movement(
    trigger: On<Complete<MovePlayer>>,
    mut players: Query<&mut LinearVelocity, SimulatedPlayer>,
) {
    if let Ok(mut velocity) = players.get_mut(trigger.context) {
        velocity.0 = Vec2::ZERO;
    }
}

/// Validate firing cadence, then hand the shot to exactly one representation.
/// Trajectory, hit policy, and timeline remain plain inputs; this function does
/// not contain their implementations.
pub(crate) fn shoot_weapon(
    trigger: On<Start<Shoot>>,
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    tick_duration: Res<TickDuration>,
    mut players: Query<
        (
            &PlayerId,
            &Position,
            &Rotation,
            &ColorComponent,
            &mut Weapon,
            &shot_buffer::ShotBuffer,
            Option<&ControlledBy>,
            Has<Controlled>,
        ),
        With<PlayerMarker>,
    >,
    aim_actions: Query<(&ActionOf<PlayerContext>, &Action<MoveCursor>)>,
    config: Single<(&TrajectoryKind, &RepresentationKind, &TimelinePolicy), With<ClientContext>>,
) {
    let Ok((id, position, rotation, color, mut weapon, shot_buffer, controlled_by, controlled)) =
        players.get_mut(trigger.context)
    else {
        return;
    };
    let (trajectory, representation, presentation_timeline) = *config;
    let tick = timeline.tick();
    let is_server = controlled_by.is_some();

    // This observer intentionally also runs while prediction is replaying a
    // rollback. Lightyear removes prespawned entities created at or after the
    // rollback tick; replaying the same input edge must recreate the entity
    // with the same signature. `Weapon` is predicted, so cadence state is
    // restored before this code runs again.

    // A client only predicts its locally controlled player's shot. Remote
    // predicted input may still fire the Shoot action in AllPredicted, but the
    // authoritative replicated projectile will provide that visual.
    if !is_server && !controlled {
        return;
    }
    if let Some(last_fire_tick) = weapon.last_fire_tick {
        let elapsed_ticks = (tick - last_fire_tick).max(0) as f64;
        let elapsed = tick_duration.0.mul_f64(elapsed_ticks);
        let minimum = Duration::from_secs_f32(1.0 / trajectory.fire_rate());
        if elapsed < minimum {
            return;
        }
    }
    weapon.last_fire_tick = Some(tick);
    let prespawn_hash = prespawn_hash(id.0, tick, *trajectory, *representation);
    let aim_value = aim_actions
        .iter()
        .find_map(|(action_of, action)| (action_of.get() == trigger.context).then_some(**action));
    // `Start<Shoot>` is triggered while BEI is still applying action events,
    // before `apply_player_aim` runs. Derive the firing rotation from the same
    // tick's aim value so the first shot and the server-authoritative shot do
    // not use a one-tick-old (or default) facing.
    let shot_rotation = aim_value
        .and_then(|aim| rotation_towards(position.0, aim))
        .unwrap_or(*rotation);
    debug!(
        shooter = ?id.0,
        ?tick,
        origin = ?position.0,
        rotation = shot_rotation.as_radians(),
        aim = ?aim_value,
        trajectory = trajectory.name(),
        representation = representation.name(),
        prespawn_hash,
        is_server,
        "Firing projectile"
    );

    match representation {
        RepresentationKind::StateEntity => state_entity::shoot(
            &mut commands,
            prespawn_hash,
            tick,
            &tick_duration,
            *position,
            shot_rotation,
            *id,
            *color,
            controlled_by,
            *trajectory,
            *presentation_timeline,
        ),
        RepresentationKind::FireDataEntity => fire_data_entity::shoot(
            &mut commands,
            prespawn_hash,
            tick,
            *position,
            shot_rotation,
            *id,
            *color,
            controlled_by,
            *trajectory,
            *presentation_timeline,
        ),
        RepresentationKind::ShotBuffer => {
            // All-interpolated clients wait for the authoritative buffer. In
            // the other timeline modes the owner appends the same predicted
            // record that the server will later confirm.
            if is_server || presentation_timeline.owner_spawns_locally() {
                shot_buffer::shoot(
                    &mut commands,
                    trigger.context,
                    shot_buffer,
                    tick,
                    *position,
                    shot_rotation,
                    *trajectory,
                );
            }
        }
    }
}

/// Selects the clock used by a local projectile presentation.
///
/// Authoritative and predicted projectiles use the local simulation tick.
/// Projectiles reconstructed for an interpolated player use that client's
/// interpolation tick, otherwise they would expire early just because the
/// local prediction timeline is ahead of the rendered remote timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExpiryTimeline {
    Local,
    Interpolation,
}

/// Local fire tick carried by the projectile entity that already exists.
///
/// This is a firing fact on the projectile that already exists, not a separate
/// shot-ID entity. State entities replicate it so all-interpolated clients can
/// identify the shot; the other representations reconstruct it locally. The
/// client-reported policy uses shooter plus fire tick to avoid reporting the
/// same predicted shot again when rollback recreates its projectile.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProjectileFireTick(pub(crate) Tick);

/// Local fixed-tick projectile lifetime.
///
/// This component is not replicated. The server and predicted owner derive it
/// from the same fire tick, while buffer/fire-data consumers derive it from
/// the fire record on whichever presentation timeline they use.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DespawnAtTick {
    pub(crate) tick: Tick,
    pub(crate) timeline: ExpiryTimeline,
}

impl DespawnAtTick {
    pub(crate) fn new(tick: Tick, timeline: ExpiryTimeline) -> Self {
        Self { tick, timeline }
    }
}

/// Convert a duration to a whole number of simulation ticks, rounding up so a
/// projectile is never shortened by a fractional tick.
pub(crate) fn expiry_tick(fire_tick: Tick, lifetime_seconds: f32, tick_duration: Duration) -> Tick {
    let ticks = (lifetime_seconds / tick_duration.as_secs_f32()).ceil() as i32;
    fire_tick + ticks.max(1)
}

#[derive(Component, Clone, PartialEq, Debug)]
pub(crate) struct DespawnAfter(pub(crate) Timer);

fn despawn_after(
    time: Res<Time>,
    mut commands: Commands,
    mut entities: Query<(Entity, &mut DespawnAfter)>,
) {
    for (entity, mut despawn_after) in &mut entities {
        despawn_after.0.tick(time.delta());
        if despawn_after.0.is_finished() {
            commands.entity(entity).try_despawn();
        }
    }
}

fn despawn_at_tick(
    local_timeline: Res<LocalTimeline>,
    interpolation_timeline: Option<Res<InterpolationTimeline>>,
    mut commands: Commands,
    entities: Query<(Entity, &DespawnAtTick)>,
) {
    let interpolation_tick = interpolation_timeline
        .filter(|timeline| timeline.is_synced())
        .map(|timeline| timeline.tick());

    for (entity, expiry) in &entities {
        let current_tick = match expiry.timeline {
            ExpiryTimeline::Local => local_timeline.tick(),
            ExpiryTimeline::Interpolation => {
                let Some(tick) = interpolation_tick else {
                    continue;
                };
                tick
            }
        };
        if current_tick >= expiry.tick {
            commands.entity(entity).try_despawn();
        }
    }
}

pub(crate) fn player_bundle(client_id: PeerId, is_bot: bool) -> impl Bundle {
    let color = color_from_id(client_id);
    // The example's initial forward direction is +Y. Put the human below the
    // bot facing up, and the bot above the human facing down. The bot then
    // strafes horizontally while aiming at the human pose rendered by its client.
    let position = if is_bot {
        Position::from_xy(0.0, 180.0)
    } else {
        // Client 1, used by the README command, is directly below the bot.
        // Nearby lanes keep additional manual clients from spawning on top of
        // it while retaining the same clear top-versus-bottom layout.
        const PLAYER_LANES: [f32; 7] = [-60.0, 0.0, 60.0, -120.0, 120.0, -180.0, 180.0];
        let lane = (client_id.to_bits() % PLAYER_LANES.len() as u64) as usize;
        Position::from_xy(PLAYER_LANES[lane], -180.0)
    };
    let rotation = if is_bot {
        Rotation::radians(core::f32::consts::PI)
    } else {
        Rotation::default()
    };
    (
        PlayerContext,
        Score(0),
        PlayerId(client_id),
        RigidBody::Kinematic,
        position,
        rotation,
        ColorComponent(color),
        PlayerMarker,
        Weapon::default(),
        shot_buffer::ShotBuffer::default(),
        Collider::rectangle(PLAYER_SIZE, PLAYER_SIZE),
        Name::new("Player"),
    )
}
