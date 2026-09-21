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
/// Drawn as a second, fainter ground ring.
pub(crate) const RELEASE_RADIUS: f32 = 5.0;
/// Blend window for every timeline switch
const SWITCH_BLEND_SECS: f32 = 1.0;
/// Suppresses timeline switches for freshly replicated entities.
///
/// New arrivals are still falling and landing while their first snapshots are
/// in flight. Switching one inside that window corrupts its physics.
const SWITCH_SETTLE_SECS: f32 = 2.0;

/// Marker armed once when a character/block first replicates locally; removed
/// by [`tick_switch_settle`] after [`SWITCH_SETTLE_SECS`]. While present, the
/// timeline policy leaves the entity on its arrival timeline.
#[derive(Component, Debug)]
struct SwitchSettle(Timer);

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        // Only the window is demoed here: the ease comes from the library
        // default, so the example shows what a game gets without tuning. The
        // debug panel can retune both live.
        app.insert_resource(TimelineSwitchSettings {
            default_transition_secs: SWITCH_BLEND_SECS,
            ..Default::default()
        });
        app.add_plugins(AutomationClientPlugin);
        // Character input and the carry rules run on both peers from `shared.rs`.
        app.add_observer(hand_over_to_interpolation);
        app.add_observer(restore_predicted_simulation);
        app.add_systems(
            Update,
            (
                handle_new_block,
                handle_new_character,
                tick_switch_settle,
                timeline_policy,
            ),
        );
        app.add_observer(handle_controlled_character);
    }
}

/// Arm the settle-in window for timeline switches.
/// Entities still inside their [`SwitchSettle`] window are skipped entirely by switches
fn arm_switch_settle(trigger: On<Add<(CharacterMarker, BlockMarker)>>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .insert(SwitchSettle(Timer::new(
            Duration::from_secs_f32(SWITCH_SETTLE_SECS),
            TimerMode::Once,
        )));
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

/// Inserts the [`TimelineSwitch`] component on blocks and characters according to the
/// policy described in the module docs.
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
    mut commands: Commands,
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
            commands
                .entity(entity)
                .insert(TimelineSwitch::to_predicted());
            if let Some(holder) = holders.iter_mut().find(|holder| holder.entity == entity) {
                holder.predicted = true;
            }
        } else if !near && is_predicted {
            info!(?entity, "switching character to interpolated ({reason})");
            commands
                .entity(entity)
                .insert(TimelineSwitch::to_interpolated());
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
            commands
                .entity(entity)
                .insert(TimelineSwitch::to_predicted());
        } else if !to_predicted && is_predicted {
            info!(?entity, "switching block to interpolated ({reason})");
            commands
                .entity(entity)
                .insert(TimelineSwitch::to_interpolated());
        }
    }
}

/// Handle newly replicated character
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
    trigger: On<Add<Interpolated>>,
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

/// Restore avian simulation when a character switches back to the predicted timeline.
fn restore_predicted_simulation(
    trigger: On<Add<Predicted>>,
    bodies: Query<&RigidBody, With<CharacterMarker>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    if bodies.get(entity) != Ok(&RigidBody::Kinematic) {
        return;
    }
    commands.entity(entity).insert(RigidBody::Dynamic);
}

fn handle_controlled_character(
    trigger: On<Add<Controlled>>,
    mut commands: Commands,
    character_query: Query<(), (With<CharacterMarker>, Without<InputMap<CharacterAction>>)>,
) {
    let entity = trigger.entity;
    if character_query.get(entity).is_err() {
        return;
    };
    info!("Adding InputMap to controlled character {entity:?}");
    commands.entity(entity).insert(
        InputMap::new([(CharacterAction::Jump, KeyCode::Space)])
            .with(CharacterAction::Jump, GamepadButton::South)
            .with(CharacterAction::Pickup, KeyCode::KeyE)
            .with_dual_axis(CharacterAction::Move, GamepadStick::LEFT)
            .with_dual_axis(CharacterAction::Move, VirtualDPad::wasd()),
    );
}

/// Add physics to blocks once, whichever timeline they arrive on. The
/// `Without<RigidBody>` filter keeps timeline switches from resetting the
/// body (a carried block is kinematic on purpose).
fn handle_new_block(
    mut commands: Commands,
    block_query: Query<
        (Entity, Has<SphereMarker>),
        (
            Or<(Added<Interpolated>, Added<Predicted>)>,
            With<BlockMarker>,
            Without<RigidBody>,
        ),
    >,
) {
    for (entity, is_sphere) in &block_query {
        info!(?entity, "Adding physics to block");
        commands
            .entity(entity)
            .insert(BlockPhysicsBundle::new(is_sphere));
    }
}
