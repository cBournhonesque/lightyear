use bevy::ecs::query::QueryData;
use bevy::math::VectorSpace;
use bevy::prelude::*;
use core::time::Duration;
use lightyear::inputs::leafwing::input_buffer::InputBuffer;
use server::ControlledEntities;
use std::hash::{Hash, Hasher};

use avian3d::prelude::*;
use avian3d::sync::{position_to_transform, SyncSet};
use bevy::prelude::TransformSystem::TransformPropagate;
use leafwing_input_manager::prelude::ActionState;
use lightyear::shared::replication::components::Controlled;
use tracing::Level;

use lightyear::prelude::client::*;
use lightyear::prelude::TickManager;
use lightyear::prelude::*;
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;

use crate::protocol::*;

pub const FLOOR_WIDTH: f32 = 100.0;
pub const FLOOR_HEIGHT: f32 = 1.0;

pub const BLOCK_WIDTH: f32 = 1.0;
pub const BLOCK_HEIGHT: f32 = 1.0;

pub const CHARACTER_CAPSULE_RADIUS: f32 = 0.5;
pub const CHARACTER_CAPSULE_HEIGHT: f32 = 0.5;

#[derive(Bundle)]
pub(crate) struct CharacterPhysicsBundle {
    collider: Collider,
    rigid_body: RigidBody,
    external_force: ExternalForce,
    external_impulse: ExternalImpulse,
    lock_axes: LockedAxes,
    friction: Friction,
}

impl Default for CharacterPhysicsBundle {
    fn default() -> Self {
        Self {
            collider: Collider::capsule(CHARACTER_CAPSULE_RADIUS, CHARACTER_CAPSULE_HEIGHT),
            rigid_body: RigidBody::Dynamic,
            external_force: ExternalForce::ZERO.with_persistence(false),
            external_impulse: ExternalImpulse::ZERO.with_persistence(false),
            lock_axes: LockedAxes::default()
                .lock_rotation_x()
                .lock_rotation_y()
                .lock_rotation_z(),
            friction: Friction::new(0.0).with_combine_rule(CoefficientCombine::Min),
        }
    }
}

#[derive(Bundle)]
pub(crate) struct FloorPhysicsBundle {
    collider: Collider,
    rigid_body: RigidBody,
}

impl Default for FloorPhysicsBundle {
    fn default() -> Self {
        Self {
            collider: Collider::cuboid(FLOOR_WIDTH, FLOOR_HEIGHT, FLOOR_WIDTH),
            rigid_body: RigidBody::Static,
        }
    }
}

#[derive(Bundle)]
pub(crate) struct BlockPhysicsBundle {
    collider: Collider,
    rigid_body: RigidBody,
}

impl Default for BlockPhysicsBundle {
    fn default() -> Self {
        Self {
            collider: Collider::cuboid(BLOCK_WIDTH, BLOCK_HEIGHT, BLOCK_WIDTH),
            rigid_body: RigidBody::Dynamic,
        }
    }
}

#[derive(Clone)]
pub struct SharedPlugin {
    pub predict_all: bool,
}

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin {
            predict_all: self.predict_all,
        });

        // Physics

        // Position and Rotation are the primary source of truth so no need to
        // sync changes from Transform to Position.
        // (we are not applying manual updates to Transform)
        app.insert_resource(avian3d::sync::SyncConfig {
            transform_to_position: false,
            position_to_transform: true,
            ..default()
        });
        // disable sleeping
        app.insert_resource(SleepingThreshold {
            linear: -0.01,
            angular: -0.01,
        });

        app.add_systems(
            FixedPostUpdate,
            after_physics_log.after(PhysicsSet::StepSimulation),
        );

        app.add_plugins(
            PhysicsPlugins::default()
                .build()
                .disable::<PhysicsInterpolationPlugin>(),
            // disable Sleeping plugin as it can mess up physics rollbacks
            // TODO: disabling sleeping plugin causes the player to fall through the floor
            // .disable::<SleepingPlugin>(),
        );

        // add an extra sync for cases where:
        // - we receive a Position, do a rollback and set C=Correct, apply sync
        // - in RunFixedMainLoop, we set C=Original
        // - FixedUpdate doesn't run because frame rate is too high!
        // - then the Transform that we show is C=Correct instead of C=Original!
        app.add_systems(
            PostUpdate,
            position_to_transform
                .in_set(PhysicsSet::Sync)
                .run_if(|config: Res<avian3d::sync::SyncConfig>| config.position_to_transform),
        );
        // app.add_systems(FixedLast, fixed_last_log
        //     .before(PredictionSet::IncrementRollbackTick)
        //     .after(InterpolationSet::UpdateVisualInterpolationState));
        // app.add_systems(Last, last_log);
    }
}

pub(crate) fn after_physics_log(
    tick_manager: Res<TickManager>,
    rollback: Option<Res<Rollback>>,
    // collisions: Option<Res<Collisions>>,
    query: Query<(Entity, &Position, &Rotation), (Without<Confirmed>, With<CharacterMarker>)>,
) {
    let tick = rollback.as_ref().map_or(tick_manager.tick(), |r| {
        tick_manager.tick_or_rollback_tick(r.as_ref())
    });
    // info!(?tick, ?collisions, "collisions");
    let is_rollback = rollback.map_or(false, |r| r.is_rollback());

    // if is_rollback {
    //     println!("rollback tick {}", tick.0);
    // }
    // for (entity, position, rotation) in query.iter() {
    //     warn!(
    //         ?is_rollback,
    //         ?tick,
    //         ?entity,
    //         ?position,
    //         ?rotation,
    //         "Block after physics update"
    //     );
    // }
}

pub(crate) fn fixed_last_log(
    tick_manager: Res<TickManager>,
    rollback: Option<Res<Rollback>>,
    // collisions: Option<Res<Collisions>>,
    query: Query<
        (Entity, &Position, Option<&Correction<Position>>),
        (Without<Confirmed>, With<ProjectileMarker>),
    >,
) {
    let tick = rollback.as_ref().map_or(tick_manager.tick(), |r| {
        tick_manager.tick_or_rollback_tick(r.as_ref())
    });
    // info!(?tick, ?collisions, "collisions");
    let is_rollback = rollback.map_or(false, |r| r.is_rollback());

    // if is_rollback {
    //     println!("rollback tick {}", tick.0);
    // }
    for (entity, position, correction) in query.iter() {
        warn!(
            ?is_rollback,
            ?tick,
            ?entity,
            ?position,
            ?correction,
            "Bullet in fixed-last"
        );
    }
}
pub(crate) fn last_log(
    time_manager: Res<TimeManager>,
    tick_manager: Res<TickManager>,
    rollback: Option<Res<Rollback>>,
    // collisions: Option<Res<Collisions>>,
    query: Query<
        (
            Entity,
            &Transform,
            Option<&Correction<Position>>,
            Option<&VisualInterpolateStatus<Transform>>,
        ),
        (Without<Confirmed>, With<ProjectileMarker>),
    >,
) {
    let tick = rollback.as_ref().map_or(tick_manager.tick(), |r| {
        tick_manager.tick_or_rollback_tick(r.as_ref())
    });
    let overstep = time_manager.overstep();
    // info!(?tick, ?collisions, "collisions");
    let is_rollback = rollback.map_or(false, |r| r.is_rollback());

    // if is_rollback {
    //     println!("rollback tick {}", tick.0);
    // }
    for (entity, transform, correction, visual_interpolate) in query.iter() {
        warn!(
            ?is_rollback,
            ?tick,
            ?entity,
            ?transform,
            ?correction,
            ?visual_interpolate,
            ?overstep,
            "Bullet in last"
        );
    }
}

/// Generate pseudo-random color based on `client_id`.
pub(crate) fn color_from_id(client_id: ClientId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

#[derive(QueryData)]
#[query_data(mutable, derive(Debug))]
pub struct CharacterQuery {
    pub external_force: &'static mut ExternalForce,
    pub external_impulse: &'static mut ExternalImpulse,
    pub linear_velocity: &'static LinearVelocity,
    pub mass: &'static ComputedMass,
    pub position: &'static Position,
    pub entity: Entity,
}

/// Apply the character actions `action_state` to the character entity `character`.
pub fn apply_character_action(
    time: &Res<Time>,
    spatial_query: &SpatialQuery,
    action_state: &ActionState<CharacterAction>,
    character: &mut CharacterQueryItem,
) {
    const MAX_SPEED: f32 = 5.0;
    const MAX_ACCELERATION: f32 = 20.0;

    // How much velocity can change in a single tick given the max acceleration.
    let max_velocity_delta_per_tick = MAX_ACCELERATION * time.delta_secs();

    // Handle jumping.
    if action_state.pressed(&CharacterAction::Jump) {
        let ray_cast_origin = character.position.0
            + Vec3::new(
                0.0,
                -CHARACTER_CAPSULE_HEIGHT / 2.0 - CHARACTER_CAPSULE_RADIUS,
                0.0,
            );

        // Only jump if the character is on the ground.
        //
        // Check if we are touching the ground by sending a ray from the bottom
        // of the character downwards.
        if spatial_query
            .cast_ray(
                ray_cast_origin,
                Dir3::NEG_Y,
                0.01,
                true,
                &SpatialQueryFilter::from_excluded_entities([character.entity]),
            )
            .is_some()
        {
            character
                .external_impulse
                .apply_impulse(Vec3::new(0.0, 5.0, 0.0));
        }
    }

    // Handle moving.
    let move_dir = action_state
        .axis_pair(&CharacterAction::Move)
        .clamp_length_max(1.0);
    let move_dir = Vec3::new(-move_dir.x, 0.0, move_dir.y);

    // Linear velocity of the character ignoring vertical speed.
    let ground_linear_velocity = Vec3::new(
        character.linear_velocity.x,
        0.0,
        character.linear_velocity.z,
    );

    let desired_ground_linear_velocity = move_dir * MAX_SPEED;

    let new_ground_linear_velocity = ground_linear_velocity
        .move_towards(desired_ground_linear_velocity, max_velocity_delta_per_tick);

    // Acceleration required to change the linear velocity from
    // `ground_linear_velocity` to `new_ground_linear_velocity` in the duration
    // of a single tick.
    //
    // There is no need to clamp the acceleration's length to
    // `MAX_ACCELERATION`. The difference between `ground_linear_velocity` and
    // `new_ground_linear_velocity` is never great enough to require more than
    // `MAX_ACCELERATION` in a single tick, This is because
    // `new_ground_linear_velocity` is calculated using
    // `max_velocity_delta_per_tick` which restricts how much the velocity can
    // change in a single tick based on `MAX_ACCELERATION`.
    let required_acceleration =
        (new_ground_linear_velocity - ground_linear_velocity) / time.delta_secs();

    character
        .external_force
        .apply_force(required_acceleration * character.mass.value());
}
