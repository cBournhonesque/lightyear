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
pub const SPHERE_RADIUS: f32 = 0.5;

pub const CHARACTER_CAPSULE_RADIUS: f32 = 0.5;
pub const CHARACTER_CAPSULE_HEIGHT: f32 = 0.5;

/// Where a carried block rides, relative to the holder.
///
/// Shared by the server (authoritative kinematic follow) and the carrier's
/// client (local predicted follow) so both timelines agree on the carry pose.
pub(crate) const CARRY_OFFSET: Vec3 = Vec3::new(0.0, 1.6, 0.0);
pub const CHARACTER_CHILD_SIZE: f32 = 0.5;
pub const CHARACTER_CHILD_OFFSET: Vec3 = Vec3::new(
    CHARACTER_CAPSULE_RADIUS + CHARACTER_CHILD_SIZE / 2.0,
    0.0,
    0.0,
);

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

/// Spawn the floor locally on every peer.
///
/// It never moves and every peer needs it from its first physics tick, so it is
/// a shared entity rather than a replicated one: no round-trip, and no window
/// where a peer simulates without ground.
fn spawn_floor(mut commands: Commands) {
    commands.spawn((
        Name::new("Floor"),
        FloorPhysicsBundle::default(),
        FloorMarker,
        Position::new(Vec3::ZERO),
    ));
}

/// A character holding a block, and the timeline it is on.
///
/// Carried blocks are posed from their holder, so every peer that simulates a
/// pose needs to know where the holder is and whether that holder is locally
/// predicted (ahead of the snapshots) or interpolated.
#[derive(Clone, Copy)]
pub(crate) struct HolderInfo {
    pub(crate) entity: Entity,
    pub(crate) pos: Vec3,
    /// Timeline the holder is on, or is switching to this frame.
    pub(crate) predicted: bool,
}

/// A carried cube rides exactly at its holder's head; accept a small slop for
/// snapshot lag on interpolated pairs.
const HEAD_MATCH_RADIUS: f32 = 1.5;

/// Resolve the visible holder of the block at `block_pos`, if any.
///
/// 1. Exact [`CarriedBy`] match: the visible character whose entity is the
///    block's recorded holder. The holder is a character entity, so the
///    reference is the same on every peer.
/// 2. Head proximity: the character whose carry pose (`pos + CARRY_OFFSET`)
///    is nearest in 3D within [`HEAD_MATCH_RADIUS`] — covers the frames
///    before the holder reference arrives.
pub(crate) fn resolve_holder(
    holders: &[HolderInfo],
    holder: Option<Entity>,
    block_pos: Vec3,
) -> Option<&HolderInfo> {
    if let Some(holder) = holder {
        if let Some(resolved) = holders.iter().find(|info| info.entity == holder) {
            return Some(resolved);
        }
    }
    let mut best: Option<&HolderInfo> = None;
    let mut best_dist = HEAD_MATCH_RADIUS;
    for info in holders {
        let dist = (info.pos + CARRY_OFFSET).distance(block_pos);
        if dist < best_dist {
            best_dist = dist;
            best = Some(info);
        }
    }
    best
}

/// Pose of the character holding the block, if visible.
///
/// Uses [`resolve_holder`], falling back to the nearest visible character
/// measured horizontally (see [`nearest_holder`]) so a carried cube still
/// tracks a character even while its holder is not (yet) visible.
fn holder_pose(holders: &[HolderInfo], holder: Option<Entity>, fallback: Vec3) -> Option<Vec3> {
    if let Some(resolved) = resolve_holder(holders, holder, fallback) {
        return Some(resolved.pos);
    }
    nearest_holder(
        &holders.iter().map(|holder| holder.pos).collect::<Vec<_>>(),
        fallback,
    )
}

/// Holder pose nearest to `pos`, if any character is visible.
///
/// Compared horizontally: a cube rides 1.6m above its holder's head, so a
/// full 3D distance would let a bystander standing next to the holder (closer
/// than 1.6m) steal the cube. The holder is always directly below it.
fn nearest_holder(holders: &[Vec3], pos: Vec3) -> Option<Vec3> {
    holders.iter().copied().min_by(|a, b| {
        horizontal_distance_squared(*a, pos)
            .partial_cmp(&horizontal_distance_squared(*b, pos))
            .unwrap_or(core::cmp::Ordering::Equal)
    })
}

fn horizontal_distance_squared(a: Vec3, b: Vec3) -> f32 {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    dx * dx + dz * dz
}

/// Pose one carried block at its holder, returning whether its body belongs
/// kinematic.
///
/// Cubes ride frozen: teleported to the carry pose with zero velocity, and the
/// caller pins the body kinematic so the step does not drag it off. Spheres
/// stay dynamic and are steered towards the pose on the leash
/// ([`carry_spring_velocity`]), which is the one carry law every peer shares —
/// the server sims the swing authoritatively, the carrier predicts it, and
/// remotes run it against the interpolated holder so their local simulation
/// agrees with the snapshots.
fn pose_carried(
    holder_pos: Vec3,
    is_sphere: bool,
    pos: &mut Position,
    lin_vel: &mut LinearVelocity,
    ang_vel: &mut AngularVelocity,
) -> bool {
    if is_sphere {
        lin_vel.0 = carry_spring_velocity(holder_pos, pos.0);
        return false;
    }
    pos.0 = holder_pos + CARRY_OFFSET;
    lin_vel.0 = Vec3::ZERO;
    ang_vel.0 = Vec3::ZERO;
    true
}

/// Apply each character's inputs to its body.
///
/// The same rule runs on both peers, because both simulate the same kind of
/// entity — they just mark it differently:
///
/// * a character this peer simulates (`Replicate` on a server, `Predicted` or
///   `DeterministicPredicted` on a client) is the one whose inputs act on the
///   body;
/// * an interpolated remote is skipped: it is only presented, and its motion
///   comes from delayed interpolation, so a force applied here would fight the
///   pose the server sent.
///
/// One pass over those markers also settles host-client mode, where a character
/// can be authoritative and predicted at once: it is still visited once, so its
/// inputs are applied once.
fn apply_character_inputs(
    time: Res<Time>,
    spatial_query: SpatialQuery,
    mut query: Query<
        (Entity, &ComputedMass, &ActionState<CharacterAction>, Forces),
        Or<(
            With<Replicate>,
            With<Predicted>,
            With<DeterministicPredicted>,
        )>,
    >,
) {
    for (entity, mass, action_state, forces) in &mut query {
        apply_character_action(entity, mass, &time, &spatial_query, action_state, forces);
    }
}

/// Carry pose for the blocks this peer simulates, in the fixed update.
///
/// The same rule runs on the server (its authoritative blocks) and on each
/// client (its local copies), because each peer poses the copy it simulates:
/// the server poses the block it owns, and a client poses the block it
/// predicts. They cannot be one call site with one holder pose, because the
/// pose has to be the local one — for my own character that is predicted, ahead
/// of the snapshots — and running it here keeps the block glued to its holder
/// instead of trailing by the replication delay.
///
/// Runs in `FixedUpdate` so it is part of the simulated tick: a predicted
/// block replays through rollback, and a rollback that re-posed it in `Update`
/// only would diverge from the confirmed state.
fn apply_carry_fixed(
    mut commands: Commands,
    mut sets: ParamSet<(
        Query<(Entity, &Position, Has<Predicted>), With<CharacterMarker>>,
        Query<
            (
                Entity,
                Option<&RigidBody>,
                &mut Position,
                &mut LinearVelocity,
                &mut AngularVelocity,
                Option<&CarriedBy>,
                Has<SphereMarker>,
            ),
            (
                With<BlockMarker>,
                With<CarriedBy>,
                Or<(With<Replicate>, With<Predicted>)>,
            ),
        >,
    )>,
) {
    // Holder poses first; the two queries both touch Position, so they only run
    // one at a time through the ParamSet.
    let holders: Vec<HolderInfo> = sets
        .p0()
        .iter()
        .map(|(entity, pos, predicted)| HolderInfo {
            entity,
            pos: pos.0,
            predicted,
        })
        .collect();
    for (entity, body, mut pos, mut lin_vel, mut ang_vel, carried_by, is_sphere) in
        sets.p1().iter_mut()
    {
        let holder = carried_by.map(|carried_by| carried_by.holder);
        let Some(holder_pos) = holder_pose(&holders, holder, pos.0) else {
            continue;
        };
        let kinematic = pose_carried(holder_pos, is_sphere, &mut pos, &mut lin_vel, &mut ang_vel);
        // The body swap is a command, so only on a transition: re-inserting it
        // every tick destroys and recreates the body (and its contacts), which
        // panics the solver during rollback replay.
        if kinematic && body != Some(&RigidBody::Kinematic) {
            commands.entity(entity).insert(RigidBody::Kinematic);
        }
    }
}

/// Carry pose for blocks whose position comes from delayed interpolation.
///
/// Snapshots lag the holder, so a block posed purely from them trails a locally
/// predicted holder by roughly delay × speed and visibly detaches whenever the
/// holder jumps. Re-posing it from the holder's local pose keeps it at the
/// holder's head on every screen; the snapshots matter again once the block
/// switches back to prediction.
///
/// Runs in `Update` after delayed interpolation has written, so the re-posed
/// location — not a stale snapshot — is what the `PostUpdate` transform sync
/// renders.
fn apply_carry_interpolated(
    mut sets: ParamSet<(
        Query<(Entity, &Position, Has<Predicted>), With<CharacterMarker>>,
        Query<
            (
                &mut Position,
                &mut LinearVelocity,
                &mut AngularVelocity,
                Option<&CarriedBy>,
            ),
            (With<BlockMarker>, With<CarriedBy>, With<Interpolated>),
        >,
    )>,
) {
    let holders: Vec<HolderInfo> = sets
        .p0()
        .iter()
        .map(|(entity, pos, predicted)| HolderInfo {
            entity,
            pos: pos.0,
            predicted,
        })
        .collect();
    for (mut pos, mut lin_vel, mut ang_vel, carried_by) in sets.p1().iter_mut() {
        let holder = carried_by.map(|carried_by| carried_by.holder);
        let Some(holder_pos) = holder_pose(&holders, holder, pos.0) else {
            continue;
        };
        pose_carried(holder_pos, false, &mut pos, &mut lin_vel, &mut ang_vel);
    }
}

/// Keep an interpolated carried block's body out of the physics step.
///
/// Interpolated blocks are posed from their holder (or from snapshots) rather
/// than simulated, but their body is still `Dynamic`, so the step would apply
/// gravity and let contacts knock them around between writes — including
/// shoving *other* bodies from a pose that is only interpolated. Pinning them
/// kinematic with zero velocity leaves the posed location alone.
///
/// Runs in `FixedUpdate`, ahead of Avian's step in `FixedPostUpdate`, so the
/// zeroed velocity is what the step integrates: a swinging sphere's snapshot
/// velocity never gets a tick to drag the body off its pose.
fn freeze_interpolated_carry(
    mut commands: Commands,
    mut blocks: Query<
        (
            Entity,
            Option<&RigidBody>,
            &mut LinearVelocity,
            &mut AngularVelocity,
        ),
        (With<BlockMarker>, With<CarriedBy>, With<Interpolated>),
    >,
) {
    for (entity, body, mut lin_vel, mut ang_vel) in &mut blocks {
        // A kinematic body still integrates its own velocity, so stale
        // velocities from the dynamic era have to go too.
        if body != Some(&RigidBody::Kinematic) {
            commands.entity(entity).insert(RigidBody::Kinematic);
        }
        lin_vel.0 = Vec3::ZERO;
        ang_vel.0 = Vec3::ZERO;
    }
}

/// Give a block back to physics once nothing is carrying it.
///
/// The kinematic bodies above and in [`apply_carry_fixed`] are local, so a
/// kinematic block with no [`CarriedBy`] came from them and must simulate
/// freely again — otherwise a dropped block hangs frozen in mid-air, and a
/// sphere handed to interpolation would never swing back when it returns to
/// prediction.
fn restore_uncarried_body(
    mut commands: Commands,
    blocks: Query<(Entity, &RigidBody), (With<BlockMarker>, Without<CarriedBy>)>,
) {
    for (entity, body) in &blocks {
        if *body == RigidBody::Kinematic {
            commands.entity(entity).insert(RigidBody::Dynamic);
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

impl BlockPhysicsBundle {
    /// The body a block of this shape needs.
    ///
    /// The shape is known when the block is first seen — [`SphereMarker`] is
    /// replicated with the entity, and is present by the time a peer adds the
    /// body — so the right collider can be built straight away rather than
    /// repaired afterwards.
    pub(crate) fn new(is_sphere: bool) -> Self {
        let collider = if is_sphere {
            Collider::sphere(SPHERE_RADIUS)
        } else {
            Collider::cuboid(BLOCK_WIDTH, BLOCK_HEIGHT, BLOCK_WIDTH)
        };
        Self {
            collider,
            rigid_body: RigidBody::Dynamic,
        }
    }
}

impl Default for BlockPhysicsBundle {
    fn default() -> Self {
        Self::new(false)
    }
}

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        app.add_observer(spawn_character_child_collider);
        // The floor is the same static body on every peer, so it is spawned
        // locally rather than replicated.
        app.add_systems(Startup, spawn_floor);
        // The carry rules run on every peer: each one poses the copies it
        // simulates, on the schedule that copy is on.
        app.add_systems(
            FixedUpdate,
            (
                apply_character_inputs,
                apply_carry_fixed,
                freeze_interpolated_carry,
                restore_uncarried_body,
            ),
        );
        app.add_systems(
            Update,
            apply_carry_interpolated.after(InterpolationSystems::All),
        );

        // Physics
        app.add_plugins(lightyear::avian3d::plugin::LightyearAvianPlugin {
            replication_mode: AvianReplicationMode::Position {
                sync_to_transform: false,
            },
            rollback_resources: false,
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
    // `ground_linear_velocity` to the new one in the duration of a single tick.
    let required_acceleration =
        (new_ground_linear_velocity - ground_linear_velocity) / time.delta_secs();

    forces.apply_force(required_acceleration * mass.value());
}

/// Stiffness of the sphere carry leash, in velocity gained per metre of
/// displacement per second.
const CARRY_SPRING_GAIN: f32 = 5.0;
/// Fastest the leash ever pulls a carried sphere.
const CARRY_SPRING_MAX_SPEED: f32 = 12.0;

/// Velocity a carried sphere should have to dangle from its holder.
///
/// This is the one carry law every timeline shares: the server runs it
/// authoritatively, the carrier predicts it, and remotes run it against the
/// interpolated holder so their local simulation agrees with the snapshots.
/// It steers velocity directly (no mass involved), so it cannot blow up and
/// behaves identically everywhere: exponential approach to the carry pose,
/// with lag, gravity droop, and contact bounces providing the leash feel.
pub(crate) fn carry_spring_velocity(holder_pos: Vec3, pos: Vec3) -> Vec3 {
    ((holder_pos + CARRY_OFFSET - pos) * CARRY_SPRING_GAIN).clamp_length_max(CARRY_SPRING_MAX_SPEED)
}
