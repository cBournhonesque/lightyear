use avian3d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::input::keyboard::Key;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::connection::host::HostServer;
use lightyear::prelude::client::*;
use lightyear::prelude::input::InputBuffer;
use lightyear::prelude::Controlled;
use lightyear::prediction::correction::CorrectionEase;
use lightyear::prelude::*;

use crate::automation::AutomationClientPlugin;
use crate::protocol::*;
use crate::shared::*;

/// Switch blocks to predicted within this distance of the local player.
///
/// Kept small on purpose so walking toward or away from a block visibly switches it.
/// Drawn as a ground ring around the local player (see renderer).
pub(crate) const SWITCH_RADIUS: f32 = 3.0;
/// Release back to interpolated past this distance (hysteresis against
/// switching back and forth at the boundary).
///
/// Deliberately much wider than one update's worth of motion: at 5 m/s with
/// ~100 ms of latency a single switch to the prediction timeline can displace
/// the live pose by ~0.5 m, which would immediately trigger a switch back to
/// interpolation with a narrow band and oscillate (each applied switch
/// re-teleports and re-blends).
/// Drawn as a second, fainter ground ring.
pub(crate) const RELEASE_RADIUS: f32 = 5.0;
/// Default blend length for every timeline switch: 1.0 s.
///
/// Switch blends converge over this window (not the global rollback decay),
/// so a longer value directly softens the visible pop at the cost of
/// a longer re-switch lockout. Lives on [`TimelineSwitchSettings`] so the
/// policy below just sends bare [`TimelineSwitch`] requests; the debug
/// panel can retune it (and the ease curve) live.
const SWITCH_BLEND_SECS: f32 = 1.0;
/// Suppresses timeline switches for freshly replicated entities.
///
/// New arrivals are still falling and landing while their first snapshots are
/// in flight. Switching one inside that window corrupts its physics: with this
/// window set to zero, the scripted two-client run below fails every time with
/// `NaN or infinity found in Avian component: type=LinearVelocity` and then
/// `assertion failed: b.min.cmple(b.max).all()` in the collider tree, while the
/// same run is clean at two seconds (3/3 each way). A switch to interpolation
/// before any snapshot arrives also leaves interpolation with nothing to
/// present yet.
///
/// This is not something a live [`SwitchBlend`] can cover: that marker only
/// exists while a blend is running, which is after a switch, and this window is
/// about the arrival that precedes the first one. Two seconds comfortably covers
/// landing (~0.7 s) plus the first snapshots (~0.2 s at 10 Hz + 100 ms).
const SWITCH_SETTLE_SECS: f32 = 2.0;

/// Marker armed once when a character/block first replicates locally; removed
/// by [`tick_switch_settle`] after [`SWITCH_SETTLE_SECS`]. While present, the
/// timeline policy leaves the entity on its arrival timeline.
#[derive(Component, Debug)]
struct SwitchSettle(Timer);

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(TimelineSwitchSettings {
            default_transition_secs: SWITCH_BLEND_SECS,
            default_ease: CorrectionEase::EaseOutCubic,
        });
        app.add_plugins(AutomationClientPlugin);
        app.add_systems(
            FixedUpdate,
            (
                handle_character_actions,
                follow_carried_cubes,
                freeze_carried_block,
                spring_carried_spheres,
                restore_dropped_block_body,
            ),
        );
        app.add_observer(swap_sphere_collider);
        app.add_observer(hand_over_to_interpolation);
        app.add_observer(restore_predicted_simulation);
        app.add_observer(restore_predicted_sphere_body);
        app.add_observer(arm_switch_settle_on_character);
        app.add_observer(arm_switch_settle_on_block);
        app.add_systems(
            Update,
            (
                handle_new_block,
                handle_new_character,
                tick_switch_settle,
                timeline_policy,
                glue_interpolated_cubes.after(InterpolationSystems::All),
            ),
        );
        app.add_observer(handle_controlled_character);
    }
}

/// Process character actions and apply them to their associated character
/// entity.
fn handle_character_actions(
    time: Res<Time>,
    spatial_query: SpatialQuery,
    mut query: Query<
        (Entity, &ComputedMass, &ActionState<CharacterAction>, Forces),
        Or<(With<Predicted>, With<DeterministicPredicted>)>,
    >,
    // In host-server mode the server portion already applies the character
    // actions, so we must not apply them a second time. The
    // `Predicted`/`DeterministicPredicted` filter excludes host-server mode,
    // which runs multiple timelines.
    timeline: Res<LocalTimeline>,
) {
    let tick = timeline.tick();
    for (entity, computed_mass, action_state, forces) in &mut query {
        // Lightyear restores the correct local or rebroadcast ActionState for every predicted
        // character, both during ordinary prediction and rollback replay.
        apply_character_action(
            entity,
            computed_mass,
            &time,
            &spatial_query,
            action_state,
            forces,
        );
    }
}

/// Decide per entity whether this client should predict or interpolate it.
///
/// The rule is proximity: near → predicted, far → interpolated. Blocks arrive
/// interpolated and switch to the prediction timeline when approached;
/// characters arrive predicted and only switch to interpolation when distant,
/// so every switch happens outside contact range. Entities still inside their
/// [`SwitchSettle`] window are skipped entirely: no switches while arrivals
/// land and their first snapshots are in flight. Either way, nearby bodies
/// share my timeline instead of clipping through interpolated copies. A cube
/// carried by someone else follows its holder's timeline instead of its own
/// proximity verdict, so the pair switches in the same frame and never splits
/// across the hysteresis boundary (the cube sits 1.6 m above the holder's
/// head, so bare 3D distances would disagree at the edge). Tint still shows
/// the timeline: green is me, purple predicted, blue interpolated, orange
/// mid-switch.
///
/// Arm the settle-in window for timeline switches.
///
/// Marker adds fire exactly once per entity (timeline switches only swap the
/// Predicted/Interpolated markers), so unlike the physics setup — which
/// re-runs on every switch — this never re-arms.
fn arm_switch_settle(entity: Entity, commands: &mut Commands) {
    commands.entity(entity).insert(SwitchSettle(Timer::new(
        Duration::from_secs_f32(SWITCH_SETTLE_SECS),
        TimerMode::Once,
    )));
}

fn arm_switch_settle_on_character(trigger: On<Add, CharacterMarker>, mut commands: Commands) {
    arm_switch_settle(trigger.entity, &mut commands);
}

fn arm_switch_settle_on_block(trigger: On<Add, BlockMarker>, mut commands: Commands) {
    arm_switch_settle(trigger.entity, &mut commands);
}

/// Count the settle-in window down; expiry lifts the policy suppression.
fn tick_switch_settle(
    time: Res<Time>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut SwitchSettle)>,
) {
    for (entity, mut settle) in &mut query {
        settle.0.tick(time.delta());
        if settle.0.is_finished() {
            commands.entity(entity).remove::<SwitchSettle>();
        }
    }
}

/// Every switch to predicted first rewinds the world to the latest
/// rollback-scanned tick via a forced rollback, so the same frame replays
/// under the new marker and the blend meets fresh simulation. Switches to
/// interpolated are plain marker swaps with a visual blend: delayed
/// interpolation presents the pose from the next frame on, and the blend
/// smooths the jump to it.
fn timeline_policy(
    host_server: Query<(), With<HostServer>>,
    // Controlled regardless of timeline: my character arrives interpolated
    // and this same policy is what switches it to the predicted timeline.
    my_character: Query<&Position, (With<CharacterMarker>, With<Controlled>)>,
    blocks: Query<
        (
            Entity,
            &Position,
            Has<Predicted>,
            Has<Interpolated>,
            Has<Controlled>,
            Option<&CarriedBy>,
        ),
        (
            With<BlockMarker>,
            Without<SwitchSettle>,
            Without<SwitchBlend>,
        ),
    >,
    characters: Query<
        (
            Entity,
            &Position,
            Has<Predicted>,
            Has<Interpolated>,
            Has<Controlled>,
        ),
        (
            With<CharacterMarker>,
            Without<SwitchSettle>,
            Without<SwitchBlend>,
        ),
    >,
    // Holder bodies WITHOUT the blend filter: a mid-blend holder already
    // shows its post-switch markers, so blocks keep following it through the
    // window instead of falling back to proximity and splitting. Inserts still
    // withhold mid-blend entities (they re-insert after expiry), but following
    // never goes blind.
    holder_bodies: Query<
        (Entity, &Position, Has<Predicted>),
        (With<CharacterMarker>, Without<SwitchSettle>),
    >,
    commands: Commands,
    mut switch_events: MessageWriter<TimelineSwitch>,
) {
    // Timeline switching is client-local; the host-server world stays authoritative.
    if !host_server.is_empty() {
        return;
    }
    let Ok(my_pos) = my_character.single() else {
        return;
    };
    // All visible holders with the timeline they are on. The push loop below
    // overwrites entries for characters switching this frame (their markers
    // still show the old timeline until the switch handlers run) for
    // `resolve_holder`.
    let mut holders: Vec<HolderInfo> = holder_bodies
        .iter()
        .map(|(entity, pos, predicted)| HolderInfo {
            entity,
            pos: pos.0,
            predicted,
        })
        .collect();
    for (entity, pos, is_predicted, is_interpolated, controlled) in &characters {
        let dist = pos.distance(my_pos.0);
        // Hysteresis shared with blocks; my own character counts as always near.
        let near = controlled
            || (is_interpolated && dist < SWITCH_RADIUS)
            || (is_predicted && dist < RELEASE_RADIUS);
        let reason = if controlled {
            "my character"
        } else if near {
            "nearby player"
        } else {
            "distant player"
        };
        // This frame's switch target overwrites the collected entry: markers
        // still show the old timeline until the switch is applied on the next
        // frame, but followers must use the timeline the holder is moving to.
        if near && is_interpolated {
            info!(?entity, "switching character to predicted ({reason})");
            switch_events.write(TimelineSwitch::to_predicted(entity));
            if let Some(holder) = holders.iter_mut().find(|holder| holder.entity == entity) {
                holder.predicted = true;
            }
        } else if !near && is_predicted {
            info!(?entity, "switching character to interpolated ({reason})");
            switch_events.write(TimelineSwitch::to_interpolated(entity));
            if let Some(holder) = holders.iter_mut().find(|holder| holder.entity == entity) {
                holder.predicted = false;
            }
        }
    }
    for (entity, pos, is_predicted, is_interpolated, controlled, carried_by) in &blocks {
        let dist = pos.distance(my_pos.0);
        // Hysteresis: switch to the prediction timeline when close, release only
        // when farther.
        let near =
            (is_interpolated && dist < SWITCH_RADIUS) || (is_predicted && dist < RELEASE_RADIUS);
        let (to_predicted, reason) = if controlled {
            (true, "carried by me")
        } else if let Some(carried_by) = carried_by {
            // Someone else's block: follow the resolved holder when visible
            // (same-frame switches, no hysteresis splits), otherwise fall back
            // to proximity.
            match resolve_holder(&holders, Some(carried_by.holder), pos.0) {
                Some(holder) => (holder.predicted, "with holder"),
                None => (near, "carried by another player"),
            }
        } else if near {
            (true, "nearby")
        } else {
            (false, "far")
        };
        if to_predicted && is_interpolated {
            info!(?entity, "switching block to predicted ({reason})");
            switch_events.write(TimelineSwitch::to_predicted(entity));
        } else if !to_predicted && is_predicted {
            info!(?entity, "switching block to interpolated ({reason})");
            switch_events.write(TimelineSwitch::to_interpolated(entity));
        }
    }
}

/// One visible character that could hold a block.
struct HolderInfo {
    entity: Entity,
    pos: Vec3,
    /// Timeline the holder is on, or is switching to this frame.
    predicted: bool,
}

/// A carried cube rides exactly at its holder's head; accept a small slop for
/// snapshot lag on interpolated pairs.
const HEAD_MATCH_RADIUS: f32 = 1.5;

/// Resolve the visible holder of the block at `block_pos`, if any.
///
/// 1. Exact [`CarriedBy`] match: the visible character whose entity is the
///    block's recorded holder. The holder is a replicated character entity,
///    so the reference maps consistently on every client.
/// 2. Head proximity: the character whose carry pose (`pos + CARRY_OFFSET`)
///    is nearest in 3D within [`HEAD_MATCH_RADIUS`] — covers the frames
///    before the holder reference arrives.
///
/// Deliberately strict (no unbounded fallback): timeline decisions must only
/// follow a positively identified holder, falling back to proximity. Posing
/// (`holder_pose`) may additionally attach to the nearest visible character so
/// a block never renders detached — a transient visual, unlike a timeline.
fn resolve_holder(
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
/// tracks a character even while its holder is not (yet) visible. The fallback
/// only poses — never decides timelines — so at worst a block renders at a
/// stranger for a frame, while snapshots stay authoritative.
fn holder_pose(holders: &[HolderInfo], holder: Option<Entity>, fallback: Vec3) -> Option<Vec3> {
    if let Some(resolved) = resolve_holder(holders, holder, fallback) {
        return Some(resolved.pos);
    }
    nearest_holder(
        &holders.iter().map(|holder| holder.pos).collect::<Vec<_>>(),
        fallback,
    )
}

/// Keep carried cubes at their holder's head: whoever carries one, every
/// predicting client teleports it there each tick.
///
/// A predicted cube must be posed locally: without this, physics (gravity)
/// drags it down between the 10Hz server corrections and it rubber-bands.
/// Teleporting to the same [`CARRY_OFFSET`](crate::shared::CARRY_OFFSET) pose
/// the server uses keeps prediction aligned with the confirmed state, so
/// rollbacks find (almost) no error.
///
/// This is what keeps another player's cube at their head on my screen during
/// their jumps: the teleport — not the delayed snapshots — poses it.
/// Interpolated cubes get the same treatment in [`glue_interpolated_cubes`].
///
/// # Why this repeats the server's rule
///
/// The server has the same rule in
/// `server::update_carried_blocks`, and it has to be applied on both timelines:
/// the server poses its authoritative block, and each client poses the copy it
/// simulates. They cannot share one system because they act on different poses
/// for the holder — the server reads its own character, while a client must read
/// the *local* holder pose, which for my own character is predicted (ahead of the
/// server) and for a remote is whatever timeline that remote is on. Running the
/// rule locally is what keeps the block glued to the holder on screen instead of
/// trailing by the replication delay. Only the carry law itself is shared; see
/// [`carry_spring_velocity`](crate::shared::carry_spring_velocity).
fn follow_carried_cubes(
    mut commands: Commands,
    host_server: Query<(), With<HostServer>>,
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
            ),
            (
                With<BlockMarker>,
                With<CarriedBy>,
                With<Predicted>,
                Without<SphereMarker>,
            ),
        >,
    )>,
) {
    // Same host-server guard as the timeline policy: the shared world stays
    // authoritative and the server plugin already moves carried blocks.
    if !host_server.is_empty() {
        return;
    }
    // Holder poses first; the two queries both touch Position, so they only
    // run one at a time through the ParamSet.
    let holders: Vec<HolderInfo> = sets
        .p0()
        .iter()
        .map(|(entity, pos, predicted)| HolderInfo {
            entity,
            pos: pos.0,
            predicted,
        })
        .collect();
    for (entity, body, mut pos, mut lin_vel, mut ang_vel, carried_by) in sets.p1().iter_mut() {
        let holder = carried_by.map(|carried_by| carried_by.holder);
        let Some(holder_pos) = holder_pose(&holders, holder, pos.0) else {
            continue;
        };
        // RigidBody is immutable: swap it via insert instead of mutation, but
        // only on an actual transition. Re-inserting every tick destroys and
        // recreates the Avian body (and its contacts) 60 times a second —
        // during rollback replay that churn panics the solver.
        if body != Some(&RigidBody::Kinematic) {
            commands.entity(entity).insert(RigidBody::Kinematic);
        }
        pos.0 = holder_pos + CARRY_OFFSET;
        lin_vel.0 = Vec3::ZERO;
        ang_vel.0 = Vec3::ZERO;
    }
}

/// Keep interpolated carried cubes at their holder's head, applied after
/// delayed interpolation.
///
/// Snapshots lag the holder by the interpolation delay, so a cube posed purely
/// from snapshots trails a locally predicted holder by ~delay × speed and
/// visibly detaches whenever the holder jumps. Re-posing it from the holder's
/// local pose — the same law [`follow_carried_cubes`] uses — keeps it at the
/// holder's head on every screen; the snapshots only matter again after the
/// next switch to the prediction timeline.
///
/// Runs in `Update` after interpolation has written, so the re-posed location
/// (not a stale snapshot) is what the `PostUpdate` transform sync renders.
/// Physics stays out of the way because [`freeze_carried_block`] pins these
/// cubes kinematic with zero velocity before every step.
fn glue_interpolated_cubes(
    host_server: Query<(), With<HostServer>>,
    mut sets: ParamSet<(
        Query<(Entity, &Position, Has<Predicted>), With<CharacterMarker>>,
        Query<
            (
                &mut Position,
                &mut LinearVelocity,
                &mut AngularVelocity,
                Option<&CarriedBy>,
            ),
            (
                With<BlockMarker>,
                With<CarriedBy>,
                With<Interpolated>,
                Without<SphereMarker>,
            ),
        >,
    )>,
) {
    // Same host-server guard as the timeline policy: the shared world stays
    // authoritative and the server plugin already moves carried blocks.
    if !host_server.is_empty() {
        return;
    }
    // Holder poses first; the two queries both touch Position, so they only
    // run one at a time through the ParamSet.
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
        pos.0 = holder_pos + CARRY_OFFSET;
        lin_vel.0 = Vec3::ZERO;
        ang_vel.0 = Vec3::ZERO;
    }
}

/// Keep an interpolated carried block's pose owned by replication, not physics.
///
/// Interpolated cubes are re-posed from the holder every frame by
/// [`glue_interpolated_cubes`], and interpolated spheres purely from
/// snapshots; either way the block is still a local `Dynamic` body, so
/// gravity would drag it down and contacts would knock it around between
/// writes. Pinning it to `Kinematic` with zero velocity (exactly like the
/// server does for cubes) leaves the posed location alone.
///
/// This runs in `FixedUpdate`, ahead of Avian's step in `FixedPostUpdate`,
/// so the zeroed velocity is what the step integrates: snapshot velocities
/// (a swinging sphere's especially) never get a tick to drag the body off
/// its pose before the next write.
///
/// Predicted blocks never reach this system: [`follow_carried_cubes`]
/// teleports predicted cubes instead, and [`spring_carried_spheres`] steers
/// predicted spheres. Interpolated ones include my own block during the
/// brief blend-lockout window where it is carried but not yet predicted.
fn freeze_carried_block(
    mut commands: Commands,
    host_server: Query<(), With<HostServer>>,
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
    // Same host-server guard as the timeline policy: the shared world stays
    // authoritative and the server plugin already moves carried blocks.
    if !host_server.is_empty() {
        return;
    }
    for (entity, body, mut lin_vel, mut ang_vel) in &mut blocks {
        // RigidBody is immutable: swap it via insert instead of mutation.
        // Stale velocities must go too: a kinematic body still integrates its
        // own velocity, so leftovers from the dynamic era would make it drift
        // between interpolation writes.
        if body != Some(&RigidBody::Kinematic) {
            commands.entity(entity).insert(RigidBody::Kinematic);
        }
        lin_vel.0 = Vec3::ZERO;
        ang_vel.0 = Vec3::ZERO;
    }
}

/// Leash for predicted carried spheres.
///
/// Unlike cubes, predicted spheres stay dynamic while carried and dangle from
/// the carry pose on [`carry_spring_velocity`](crate::shared::carry_spring_velocity):
/// the server sims the swing authoritatively, I predict it for spheres I
/// carry, and for nearby spheres the same law runs against the local holder
/// pose so prediction agrees with the snapshots. Either way every screen
/// shows the same bounce.
///
/// Predicted only: steering an interpolated sphere's velocity would fight the
/// snapshots — physics would integrate the steered velocity for a tick, then
/// interpolation would snap the pose back, oscillating every frame.
/// Interpolated spheres are posed purely from snapshots while
/// [`freeze_carried_block`] holds them kinematic.
///
/// The holder is my own character for spheres I control, otherwise the exact
/// holder named by [`CarriedBy`] (see [`holder_pose`]).
fn spring_carried_spheres(
    host_server: Query<(), With<HostServer>>,
    my_holder: Query<&Position, (With<CharacterMarker>, With<Controlled>)>,
    holders: Query<(Entity, &Position, Has<Predicted>), With<CharacterMarker>>,
    mut blocks: Query<
        (
            &Position,
            &mut LinearVelocity,
            Has<Controlled>,
            Option<&CarriedBy>,
        ),
        (
            With<BlockMarker>,
            With<SphereMarker>,
            With<CarriedBy>,
            With<Predicted>,
        ),
    >,
) {
    // Same host-server guard as the timeline policy: the shared world stays
    // authoritative and the server plugin already moves carried blocks.
    if !host_server.is_empty() {
        return;
    }
    let my_pos = my_holder.single().map(|pos| pos.0).ok();
    let holders: Vec<HolderInfo> = holders
        .iter()
        .map(|(entity, pos, predicted)| HolderInfo {
            entity,
            pos: pos.0,
            predicted,
        })
        .collect();
    for (pos, mut lin_vel, controlled, carried_by) in &mut blocks {
        let holder = carried_by.map(|carried_by| carried_by.holder);
        let holder_pos = if controlled {
            my_pos.or_else(|| holder_pose(&holders, holder, pos.0))
        } else {
            holder_pose(&holders, holder, pos.0)
        };
        let Some(holder_pos) = holder_pos else {
            continue;
        };
        lin_vel.0 = crate::shared::carry_spring_velocity(holder_pos, pos.0);
    }
}

/// Restore swinging once a carried sphere returns to prediction.
///
/// [`freeze_carried_block`] pins interpolated carried blocks kinematic; cubes
/// stay kinematic when predicted (teleported frozen), but a predicted sphere
/// must simulate freely again on the shared leash.
fn restore_predicted_sphere_body(
    trigger: On<Add, Predicted>,
    mut commands: Commands,
    blocks: Query<&RigidBody, (With<BlockMarker>, With<SphereMarker>, With<CarriedBy>)>,
) {
    let entity = trigger.entity;
    if blocks.get(entity) != Ok(&RigidBody::Kinematic) {
        return;
    }
    commands.entity(entity).insert(RigidBody::Dynamic);
}

/// Holder pose nearest to `pos`, if any character is visible.
///
/// Compared horizontally: the cube rides 1.6m above its holder's head, so a
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

/// Swap the placeholder cuboid collider for a sphere one.
///
/// `handle_new_block` builds cuboid bodies for every block, but the
/// [`SphereMarker`] often arrives in a later replication message; this
/// observer repairs the collider whenever the marker arrives, whichever comes
/// first.
fn swap_sphere_collider(
    trigger: On<Add, SphereMarker>,
    mut commands: Commands,
    blocks: Query<(), With<BlockMarker>>,
) {
    let entity = trigger.entity;
    if blocks.get(entity).is_err() {
        return;
    }
    commands
        .entity(entity)
        .insert(Collider::sphere(crate::shared::SPHERE_RADIUS));
}

/// Restore dynamics once a block is no longer carried.
///
/// The kinematic bodies from [`follow_carried_cubes`] and
/// [`freeze_carried_block`] are client-local (`RigidBody` never replicates),
/// so any kinematic block without [`CarriedBy`] came from those systems and must
/// simulate freely again. Without this a dropped block would hang frozen in
/// mid-air.
fn restore_dropped_block_body(
    mut commands: Commands,
    blocks: Query<(Entity, &RigidBody), (With<BlockMarker>, Without<CarriedBy>)>,
) {
    for (entity, body) in &blocks {
        if *body == RigidBody::Kinematic {
            commands.entity(entity).insert(RigidBody::Dynamic);
        }
    }
}

/// Add physics to characters once, whichever timeline they arrive on. The
/// `Without<RigidBody>` filter keeps timeline switches from resetting the
/// body (which would drop jump velocity mid-switch).
fn handle_new_character(
    mut commands: Commands,
    character_query: Query<
        (Entity, Has<Interpolated>),
        (
            Or<(Added<Predicted>, Added<Interpolated>)>,
            With<CharacterMarker>,
            Without<RigidBody>,
        ),
    >,
) {
    for (entity, is_interpolated) in &character_query {
        info!("Character ready on client: {entity:?}");
        info!(?entity, "Adding physics to character");
        commands
            .entity(entity)
            .insert(CharacterPhysicsBundle::default());
        // A switch can be applied before this setup runs (that happens in
        // PreUpdate, this is Update): an already-interpolated character must
        // start kinematic or its dynamic body simulates locally and fights
        // replication.
        if is_interpolated {
            commands.entity(entity).insert((
                RigidBody::Kinematic,
                LinearVelocity(Vec3::ZERO),
                AngularVelocity(Vec3::ZERO),
            ));
        }
    }
}

/// Hand an entity over to delayed interpolation for display.
///
/// Local simulation fights the interpolated pose and is disabled here: a
/// dynamic interpolated remote keeps simulating with no inputs applied, so it
/// rests at its old pose while the server walks away, or drifts off on a
/// stale velocity — and collides where nothing visible is. Kinematic bodies
/// hold the interpolated pose instead. (Frame interpolation is already
/// removed by the switch itself in `apply_saved_switches`; delayed
/// interpolation samples per render frame, so nothing is lost.)
///
/// Swaps only happen on actual transitions: a per-tick re-insert would
/// recreate the Avian body (and its contacts) every frame, which panics the
/// solver during rollback replay.
fn hand_over_to_interpolation(
    trigger: On<Add, Interpolated>,
    bodies: Query<&RigidBody, With<CharacterMarker>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    if bodies.get(entity) != Ok(&RigidBody::Dynamic) {
        return;
    }
    commands.entity(entity).insert((
        RigidBody::Kinematic,
        LinearVelocity(Vec3::ZERO),
        AngularVelocity(Vec3::ZERO),
    ));
}

/// Restore simulation when a character switches back to the predicted timeline.
fn restore_predicted_simulation(
    trigger: On<Add, Predicted>,
    bodies: Query<&RigidBody, With<CharacterMarker>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    if bodies.get(entity) != Ok(&RigidBody::Kinematic) {
        return;
    }
    // Velocities stay as replicated: the pose continues from the delayed
    // pose and the next natural rollback corrects from confirmed data.
    commands.entity(entity).insert(RigidBody::Dynamic);
}

fn handle_controlled_character(
    trigger: On<Add, Controlled>,
    mut commands: Commands,
    character_query: Query<(), (With<CharacterMarker>, Without<InputMap<CharacterAction>>)>,
) {
    let entity = trigger.entity;
    if character_query.get(entity).is_err() {
        return;
    };
    info!("Adding InputMap to controlled character {entity:?}");
    commands.entity(entity).insert(character_input_map());
}

pub(crate) fn character_input_map() -> InputMap<CharacterAction> {
    InputMap::new([(CharacterAction::Jump, KeyCode::Space)])
        .with(CharacterAction::Jump, GamepadButton::South)
        .with(CharacterAction::Pickup, KeyCode::KeyE)
        .with_dual_axis(CharacterAction::Move, GamepadStick::LEFT)
        .with_dual_axis(CharacterAction::Move, VirtualDPad::wasd())
}

/// Add physics to blocks once, whichever timeline they arrive on. The
/// `Without<RigidBody>` filter keeps timeline switches from resetting the
/// body (a carried block is kinematic on purpose).
fn handle_new_block(
    mut commands: Commands,
    block_query: Query<
        Entity,
        (
            Or<(Added<Interpolated>, Added<Predicted>)>,
            With<BlockMarker>,
            Without<RigidBody>,
        ),
    >,
) {
    for entity in &block_query {
        info!(?entity, "Adding physics to block");
        commands
            .entity(entity)
            .insert(BlockPhysicsBundle::default());
    }
}
