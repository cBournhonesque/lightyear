use bevy::ecs::query::QueryData;
use bevy::math::VectorSpace;
use bevy::prelude::*;
use core::hash::Hash;

use crate::protocol::*;
use avian3d::prelude::forces::ForcesItem;
use avian3d::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::avian3d::plugin::AvianReplicationMode;
use lightyear::connection::client_of::ClientOf;
use lightyear::prelude::*;

pub const FLOOR_WIDTH: f32 = 100.0;
pub const FLOOR_HEIGHT: f32 = 1.0;

pub const BLOCK_WIDTH: f32 = 1.0;
pub const BLOCK_HEIGHT: f32 = 1.0;

pub const CHARACTER_CAPSULE_RADIUS: f32 = 0.5;
pub const CHARACTER_CAPSULE_HEIGHT: f32 = 0.5;
pub const CHARACTER_CHILD_SIZE: f32 = 0.5;
pub const CHARACTER_CHILD_OFFSET: Vec3 = Vec3::new(
    CHARACTER_CAPSULE_RADIUS + CHARACTER_CHILD_SIZE / 2.0,
    0.0,
    0.0,
);

pub const PROJECTILE_RADIUS: f32 = 0.25;
const PROJECTILE_DENSITY: f32 = 2.0;

/// Local-only marker for the fixed-offset cube collider in the `CharacterMarker` template.
#[derive(Component)]
pub(crate) struct CharacterChildCollider;

impl CharacterChildCollider {
    pub(crate) fn local_transform() -> Transform {
        Transform::from_translation(CHARACTER_CHILD_OFFSET)
    }

    pub(crate) fn collider() -> Collider {
        Collider::cuboid(
            CHARACTER_CHILD_SIZE,
            CHARACTER_CHILD_SIZE,
            CHARACTER_CHILD_SIZE,
        )
    }
}

/// Reconstruct the character's touching child cube independently on every peer.
fn spawn_character_child_collider(trigger: On<Add, CharacterMarker>, mut commands: Commands) {
    let character = trigger.entity;
    commands.spawn((
        ChildOf(character),
        CharacterChildCollider,
        CharacterChildCollider::local_transform(),
        CharacterChildCollider::collider(),
        ColliderOf { body: character },
        ColliderDensity(0.1),
        Restitution::new(0.3),
        CollisionLayers::default(),
        Name::new("CharacterOffsetCubeCollider"),
    ));
}

#[derive(Bundle)]
pub(crate) struct CharacterPhysicsBundle {
    collider: Collider,
    rigid_body: RigidBody,
    lock_axes: LockedAxes,
    friction: Friction,
}

impl Default for CharacterPhysicsBundle {
    fn default() -> Self {
        Self {
            collider: Collider::capsule(CHARACTER_CAPSULE_RADIUS, CHARACTER_CAPSULE_HEIGHT),
            rigid_body: RigidBody::Dynamic,
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

#[derive(Bundle)]
pub(crate) struct ProjectilePhysicsBundle {
    collider: Collider,
    rigid_body: RigidBody,
    mass_properties: MassPropertiesBundle,
    restitution: Restitution,
    friction: Friction,
    swept_ccd: SweptCcd,
}

impl Default for ProjectilePhysicsBundle {
    fn default() -> Self {
        let collider = Collider::sphere(PROJECTILE_RADIUS);
        Self {
            mass_properties: MassPropertiesBundle::from_shape(&collider, PROJECTILE_DENSITY),
            collider,
            rigid_body: RigidBody::Dynamic,
            restitution: Restitution::new(0.8).with_combine_rule(CoefficientCombine::Max),
            friction: Friction::new(0.0).with_combine_rule(CoefficientCombine::Min),
            swept_ccd: SweptCcd::default(),
        }
    }
}

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        app.add_observer(spawn_character_child_collider);

        // Physics
        app.add_plugins(lightyear::avian3d::plugin::LightyearAvianPlugin {
            replication_mode: AvianReplicationMode::Position {
                sync_to_transform: false,
            },
            ..default()
        });
        app.add_plugins(
            PhysicsPlugins::default()
                .build()
                // disable the position<>transform sync plugins as it is handled by lightyear_avian
                .disable::<PhysicsTransformPlugin>()
                .disable::<PhysicsInterpolationPlugin>()
                // disable Sleeping plugin as it can mess up physics rollbacks
                .disable::<IslandPlugin>()
                .disable::<IslandSleepingPlugin>(),
        );

        crate::debug::register_debug_systems(app);
    }
}

/// Generates a pseudo-random color from the peer id.
pub(crate) fn color_from_id(client_id: PeerId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

/// Apply the character actions `action_state` to the character entity `character`.
pub fn apply_character_action(
    entity: Entity,
    mass: &ComputedMass,
    time: &Res<Time>,
    spatial_query: &SpatialQuery,
    action_state: &ActionState<CharacterAction>,
    mut forces: ForcesItem,
) {
    const MAX_SPEED: f32 = 5.0;
    const MAX_ACCELERATION: f32 = 20.0;

    // How much velocity can change in a single tick given the max acceleration.
    let max_velocity_delta_per_tick = MAX_ACCELERATION * time.delta_secs();

    // Handle jumping.
    if action_state.just_pressed(&CharacterAction::Jump) {
        let ray_cast_origin = forces.position().0
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
                &SpatialQueryFilter::from_excluded_entities([entity]),
            )
            .is_some()
        {
            forces.apply_linear_impulse(Vec3::new(0.0, 5.0, 0.0));
        }
    }

    // Handle moving.
    let move_dir = action_state
        .axis_pair(&CharacterAction::Move)
        .clamp_length_max(1.0);
    let move_dir = Vec3::new(-move_dir.x, 0.0, move_dir.y);

    // Linear velocity of the character ignoring vertical speed.
    let linear_velocity = forces.linear_velocity();
    let ground_linear_velocity = Vec3::new(linear_velocity.x, 0.0, linear_velocity.z);

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

    forces.apply_force(required_acceleration * mass.value());
}
