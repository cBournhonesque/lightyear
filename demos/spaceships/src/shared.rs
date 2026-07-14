use bevy::diagnostic::LogDiagnosticsPlugin;
use bevy::ecs::query::QueryData;
use bevy::prelude::*;
use core::hash::{Hash, Hasher};
use core::time::Duration;
use std::collections::HashMap;

use crate::protocol::*;
#[cfg(feature = "gui")]
use crate::renderer::ExampleRendererPlugin;
use avian2d::prelude::{forces::ForcesItem, *};
use leafwing_input_manager::prelude::ActionState;
use lightyear::avian2d::plugin::AvianReplicationMode;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prelude::*;
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;
use tracing::Level;

pub(crate) const MAX_VELOCITY: f32 = 200.0;
pub(crate) const WALL_SIZE: f32 = 350.0;

#[derive(Clone)]
pub struct SharedPlugin {
    pub(crate) show_confirmed: bool,
}

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        app.init_resource::<BulletDebugRegistry>();

        // bundles
        app.add_systems(Startup, init);

        // Physics
        app.add_plugins(lightyear::avian2d::plugin::LightyearAvianPlugin {
            replication_mode: AvianReplicationMode::Position {
                sync_to_transform: false,
            },
            ..default()
        });
        app.add_plugins(
            PhysicsPlugins::default()
                .build()
                // disable syncing position<>transform as it is handled by lightyear_avian
                .disable::<PhysicsTransformPlugin>()
                .disable::<PhysicsInterpolationPlugin>()
                // disable island sleeping plugin as it's not compatible with rollbacks
                .disable::<IslandPlugin>()
                .disable::<IslandSleepingPlugin>(),
        );
        app.insert_resource(Gravity(Vec2::ZERO));

        // Movement/firing inputs are applied before Avian's fixed physics step.
        app.add_systems(
            FixedUpdate,
            (player_movement, shared_player_firing, lifetime_despawner),
        );
        app.add_systems(
            PostUpdate,
            (
                emit_bullet_post_update_state.after(TransformSystems::Propagate),
                track_bullet_lifecycle_added,
                track_bullet_lifecycle_removed,
                detect_duplicate_bullets,
            )
                .chain(),
        );
        app.add_systems(
            FixedPostUpdate,
            process_collisions.after(PhysicsSystems::StepSimulation),
        );

        app.add_message::<BulletHitMessage>();
    }
}

#[derive(Resource, Default)]
struct BulletDebugRegistry {
    bullets: HashMap<Entity, (PeerId, Tick)>,
}

fn emit_bullet_post_update_state(
    timeline: Res<LocalTimeline>,
    interpolation_timeline: Query<&InterpolationTimeline>,
    bullets: Query<
        (
            Entity,
            &BulletMarker,
            &BulletLifetime,
            &Position,
            &LinearVelocity,
            &Transform,
            &GlobalTransform,
            Option<&ConfirmedHistory<Position>>,
            Has<Predicted>,
            Has<Interpolated>,
            Has<PreSpawned>,
            Has<Replicate>,
            Has<Replicated>,
        ),
        With<BulletMarker>,
    >,
) {
    let tick = timeline.tick();
    let interpolation_tick = interpolation_timeline
        .iter()
        .next()
        .map(|timeline| timeline.tick().0 as i64);
    for (
        entity,
        marker,
        lifetime,
        position,
        velocity,
        transform,
        global_transform,
        position_history,
        is_predicted,
        is_interpolated,
        is_prespawned,
        is_replicate,
        is_replicated,
    ) in &bullets
    {
        let position_history_start_tick = position_history
            .and_then(|history| history.start_present().map(|(tick, _)| tick.0 as i64));
        let position_history_end_tick = position_history
            .and_then(|history| history.get_nth_present(1).map(|(tick, _)| tick.0 as i64));
        let position_visual_ready = position_history_end_tick.is_some()
            && position_history_start_tick
                .zip(interpolation_tick)
                .is_some_and(|(start_tick, interpolation_tick)| interpolation_tick >= start_tick);
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::PostUpdate,
            "PostUpdate",
            "spaceships_bullet_post_update_state",
            local_tick = tick.0 as i64,
            entity = ?entity,
            owner = ?marker.owner,
            owner_bits = marker.owner.to_bits(),
            origin_tick = lifetime.origin_tick.0 as i64,
            position = ?position,
            velocity = ?velocity,
            transform = ?transform.translation.truncate(),
            global_transform = ?global_transform.translation().truncate(),
            position_history_ready = position_history_end_tick.is_some(),
            position_visual_ready = position_visual_ready,
            position_history_start_tick = ?position_history_start_tick,
            position_history_end_tick = ?position_history_end_tick,
            interpolation_tick = ?interpolation_tick,
            is_predicted = is_predicted,
            is_interpolated = is_interpolated,
            is_prespawned = is_prespawned,
            is_replicate = is_replicate,
            is_replicated = is_replicated,
            "Spaceships bullet state after transform propagation"
        );
    }
}

pub(crate) fn color_from_id(client_id: PeerId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

pub(crate) fn init(mut commands: Commands) {
    commands.spawn(WallBundle::new(
        Vec2::new(-WALL_SIZE, -WALL_SIZE),
        Vec2::new(-WALL_SIZE, WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(-WALL_SIZE, WALL_SIZE),
        Vec2::new(WALL_SIZE, WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(WALL_SIZE, WALL_SIZE),
        Vec2::new(WALL_SIZE, -WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(WALL_SIZE, -WALL_SIZE),
        Vec2::new(-WALL_SIZE, -WALL_SIZE),
        Color::WHITE,
    ));
}

/// applies forces based on action state inputs
pub fn apply_action_state_to_player_movement(
    action: &ActionState<PlayerActions>,
    mut forces: ForcesItem,
    tick: Tick,
) {
    let rot = *forces.rotation();
    const THRUSTER_POWER: f32 = 32000.;
    const ROTATIONAL_SPEED: f32 = 4.0;

    if action.pressed(&PlayerActions::Up) {
        forces.apply_force(rot * (Vec2::Y * THRUSTER_POWER));
    }
    let desired_ang_vel = if action.pressed(&PlayerActions::Left) {
        ROTATIONAL_SPEED
    } else if action.pressed(&PlayerActions::Right) {
        -ROTATIONAL_SPEED
    } else {
        0.0
    };
    let ang_vel = forces.angular_velocity();
    if ang_vel != desired_ang_vel {
        *forces.angular_velocity_mut() = desired_ang_vel;
    }
}

/// Read inputs and move players
///
/// If we didn't receive the input for a given player, we do nothing (which is the default behaviour from lightyear),
/// which means that we will be using the last known input for that player
/// (i.e. we consider that the player kept pressing the same keys).
/// see: https://github.com/cBournhonesque/lightyear/issues/492
pub(crate) fn player_movement(
    mut q: Query<
        (&ActionState<PlayerActions>, &Player, Forces),
        (With<Player>, Without<Interpolated>),
    >,
    timeline: Res<LocalTimeline>,
) {
    let tick = timeline.tick();
    for (action_state, player, forces) in q.iter_mut() {
        if !action_state.get_pressed().is_empty() {
            trace!(
                "🎹 {:?} {tick:?} = {:?}",
                player.client_id,
                action_state.get_pressed(),
            );
        }
        apply_action_state_to_player_movement(action_state, forces, tick);
    }
}

/// Clients prespawn bullets for any predicted player whose rebroadcast input is available. The
/// server replicates those bullets back as predicted entities so the local prespawn can be matched.
///
/// When spawning locally, we add the `PreSpawned` component. When a client receives the replication
/// packet from the server, it matches the hash on its own `PreSpawned` entity and treats that entity
/// as the authoritative predicted bullet.
pub fn shared_player_firing(
    mut q: Query<(
        &Position,
        Option<&Rotation>,
        Option<&LinearVelocity>,
        &ColorComponent,
        &ActionState<PlayerActions>,
        &LeafwingBuffer<PlayerActions>,
        &mut Weapon,
        Option<&ConfirmedHistory<Weapon>>,
        Has<Controlled>,
        Has<Predicted>,
        Has<Interpolated>,
        Option<&ControlledBy>,
        &Player,
    )>,
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    synced_client: Query<(), (With<Client>, With<IsSynced<InputTimeline>>)>,
    server: Query<(), With<Server>>,
) {
    let client_is_synced = !synced_client.is_empty();
    let is_server = !server.is_empty();
    if q.is_empty() {
        return;
    }

    let current_tick = timeline.tick();
    for (
        player_position,
        player_rotation,
        player_velocity,
        color,
        action,
        input_buffer,
        mut weapon,
        weapon_history,
        is_local,
        is_predicted,
        is_interpolated,
        controlled_by,
        player,
    ) in q.iter_mut()
    {
        if is_server {
            if controlled_by.is_none() {
                continue;
            }
        } else if !client_is_synced || !is_predicted || is_interpolated {
            continue;
        }
        // Firing runs in FixedUpdate. Using a level-trigger here is more robust than
        // relying on a frame-edge `just_pressed`, and the weapon cooldown already
        // guarantees we only spawn bullets at the intended rate.
        if !action.pressed(&PlayerActions::Fire) {
            continue;
        }
        if !is_server
            && !client_should_fire(input_buffer, &weapon, current_tick, !is_local, !is_local)
        {
            continue;
        }

        let last_fire_tick = if is_server {
            weapon.last_fire_tick
        } else {
            effective_last_fire_tick(&weapon, weapon_history, current_tick)
        };
        let wrapped_diff = last_fire_tick - current_tick;
        if wrapped_diff.abs() <= weapon.cooldown as i32 {
            // cooldown period - can't fire.
            if last_fire_tick == current_tick {
                // logging because debugging latency edge conditions where
                // inputs arrive on exact frame server replicates to you.
                info!("Can't fire, fired this tick already! {current_tick:?}");
            } else {
                // info!("cooldown. {weapon:?} current_tick = {current_tick:?} wrapped_diff: {wrapped_diff}");
            }
            continue;
        }
        let prev_last_fire_tick = weapon.last_fire_tick;
        weapon.last_fire_tick = current_tick;
        let player_rotation = player_rotation.copied().unwrap_or_default();

        // bullet spawns just in front of the nose of the ship, in the direction the ship is facing,
        // and inherits the speed of the ship.
        let bullet_spawn_offset = Vec2::Y * (2.0 + (SHIP_LENGTH + BULLET_SIZE) / 2.0);

        let bullet_origin = player_position.0 + player_rotation * bullet_spawn_offset;
        let player_velocity = player_velocity.map_or(Vec2::ZERO, |velocity| velocity.0);
        let bullet_linvel = player_rotation * (Vec2::Y * weapon.bullet_speed) + player_velocity;

        // A bullet is uniquely identified by the owner and the simulation tick
        // that fired it. Use an explicit hash instead of the default
        // archetype-based hash so rollback replay/component timing cannot make
        // the local prespawn disagree with the server spawn.
        let prespawn_hash = bullet_prespawn_hash(player.client_id, current_tick);
        let prespawned = PreSpawned::new(prespawn_hash);

        let bullet_entity = commands
            .spawn((
                Position(bullet_origin),
                LinearVelocity(bullet_linvel),
                ColorComponent((color.0.to_linear() * 5.0).into()), // bloom !
                BulletLifetime {
                    origin_tick: current_tick,
                    lifetime: FIXED_TIMESTEP_HZ as i32 * 2,
                },
                BulletMarker::new(player.client_id),
                PhysicsBundle::bullet(),
                bullet_mass_properties(),
                prespawned,
            ))
            .id();
        lightyear_debug_event!(
            DebugCategory::Prediction,
            DebugSamplePoint::FixedUpdate,
            "FixedUpdate",
            "spaceships_bullet_spawn",
            tick = ?current_tick,
            entity = ?bullet_entity,
            owner = ?player.client_id,
            is_server = is_server,
            prespawn_hash = prespawn_hash,
            position = ?bullet_origin,
            linear_velocity = ?bullet_linvel,
            previous_last_fire_tick = ?prev_last_fire_tick,
            is_local = is_local,
            "Spaceships bullet spawned"
        );
        info!(
            pressed=?action.get_pressed(),
            "spawned bullet for ActionState, bullet={bullet_entity:?} ({}, {}). prev last_fire tick: {prev_last_fire_tick:?}",
            weapon.last_fire_tick.0, player.client_id
        );

        if is_server {
            #[cfg(feature = "server")]
            commands.entity(bullet_entity).insert((
                Replicate::to_clients(NetworkTarget::All),
                PredictionTarget::to_clients(NetworkTarget::All),
            ));
        }
    }
}

fn bullet_prespawn_hash(owner: PeerId, tick: Tick) -> u64 {
    let mut x = owner.to_bits() ^ ((tick.0 as u64) << 32) ^ tick.0 as u64;
    // SplitMix64 finalizer: stable, cheap, and sufficient for example-level
    // prespawn identity where the inputs are already unique.
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn client_should_fire(
    input_buffer: &LeafwingBuffer<PlayerActions>,
    weapon: &Weapon,
    tick: Tick,
    allow_initial_fire: bool,
    require_remote_input: bool,
) -> bool {
    // Local players keep the first-shot guard: before the first server Weapon confirmation,
    // a held local Fire input can be older than the synced timeline and create an unmatched
    // prespawn. Remote predicted players can predict their first shot when the rebroadcast
    // buffer has the relevant tick, otherwise Weapon prediction would always lag the server.
    if !allow_initial_fire && weapon.last_fire_tick == Tick(0) {
        return false;
    }
    if require_remote_input
        && input_buffer
            .last_remote_tick
            .is_none_or(|last_tick| last_tick < tick)
    {
        return false;
    }
    let current_pressed = input_buffer
        .get(tick)
        .is_some_and(|snapshot| snapshot.0.pressed(&PlayerActions::Fire));
    let has_previous_sample = input_buffer.get(tick - 1).is_some();

    current_pressed && has_previous_sample
}

fn effective_last_fire_tick(
    weapon: &Weapon,
    weapon_history: Option<&ConfirmedHistory<Weapon>>,
    tick: Tick,
) -> Tick {
    let mut last_fire_tick = weapon.last_fire_tick;
    if let Some(history_weapon) = weapon_history.and_then(|history| history.get_present(tick))
        && history_weapon.last_fire_tick - last_fire_tick > 0
    {
        last_fire_tick = history_weapon.last_fire_tick;
    }
    last_fire_tick
}

fn track_bullet_lifecycle_added(
    timeline: Res<LocalTimeline>,
    mut registry: ResMut<BulletDebugRegistry>,
    bullets: Query<
        (
            Entity,
            &BulletMarker,
            &BulletLifetime,
            &Position,
            Has<Predicted>,
            Has<Interpolated>,
            Has<PreSpawned>,
            Has<Replicate>,
        ),
        Added<BulletMarker>,
    >,
    rollback: Query<(), With<Rollback>>,
) {
    let tick = timeline.tick();
    let in_rollback = !rollback.is_empty();
    for (
        entity,
        marker,
        lifetime,
        position,
        is_predicted,
        is_interpolated,
        is_prespawned,
        is_replicate,
    ) in &bullets
    {
        registry
            .bullets
            .insert(entity, (marker.owner, lifetime.origin_tick));
        lightyear_debug_event!(
            DebugCategory::Prediction,
            DebugSamplePoint::PostUpdate,
            "PostUpdate",
            "spaceships_bullet_lifecycle_added",
            local_tick = tick.0 as i64,
            entity = ?entity,
            owner = ?marker.owner,
            owner_bits = marker.owner.to_bits(),
            origin_tick = lifetime.origin_tick.0 as i64,
            position = ?position,
            is_predicted = is_predicted,
            is_interpolated = is_interpolated,
            is_prespawned = is_prespawned,
            is_replicate = is_replicate,
            in_rollback = in_rollback,
            "Spaceships bullet lifecycle added"
        );
    }
}

fn track_bullet_lifecycle_removed(
    timeline: Res<LocalTimeline>,
    mut registry: ResMut<BulletDebugRegistry>,
    mut removed: RemovedComponents<BulletMarker>,
    rollback: Query<(), With<Rollback>>,
) {
    let tick = timeline.tick();
    let in_rollback = !rollback.is_empty();
    for entity in removed.read() {
        let identity = registry.bullets.remove(&entity);
        let (owner, owner_bits, origin_tick) = match identity {
            Some((owner, origin_tick)) => (
                Some(owner),
                Some(owner.to_bits()),
                Some(origin_tick.0 as i64),
            ),
            None => (None, None, None),
        };
        lightyear_debug_event!(
            DebugCategory::Prediction,
            DebugSamplePoint::PostUpdate,
            "PostUpdate",
            "spaceships_bullet_lifecycle_removed",
            local_tick = tick.0 as i64,
            entity = ?entity,
            owner = ?owner,
            owner_bits = owner_bits,
            origin_tick = origin_tick,
            in_rollback = in_rollback,
            "Spaceships bullet lifecycle removed"
        );
    }
}

fn detect_duplicate_bullets(
    timeline: Res<LocalTimeline>,
    bullets: Query<(
        Entity,
        &BulletMarker,
        &BulletLifetime,
        &Position,
        Has<Predicted>,
        Has<Interpolated>,
        Has<PreSpawned>,
        Has<Replicate>,
    )>,
    rollback: Query<(), With<Rollback>>,
) {
    #[derive(Debug)]
    struct BulletDuplicateState {
        entity: Entity,
        position: Vec2,
        is_predicted: bool,
        is_interpolated: bool,
        is_prespawned: bool,
        is_replicate: bool,
    }

    let tick = timeline.tick();
    let in_rollback = !rollback.is_empty();
    let mut groups: HashMap<(u64, u32), Vec<BulletDuplicateState>> = HashMap::new();
    for (
        entity,
        marker,
        lifetime,
        position,
        is_predicted,
        is_interpolated,
        is_prespawned,
        is_replicate,
    ) in &bullets
    {
        groups
            .entry((marker.owner.to_bits(), lifetime.origin_tick.0))
            .or_default()
            .push(BulletDuplicateState {
                entity,
                position: position.0,
                is_predicted,
                is_interpolated,
                is_prespawned,
                is_replicate,
            });
    }
    for ((owner_bits, origin_tick), entities) in groups {
        if entities.len() <= 1 {
            continue;
        }
        lightyear_debug_event!(
            DebugCategory::Prediction,
            DebugSamplePoint::PostUpdate,
            "PostUpdate",
            "spaceships_bullet_duplicate_active",
            local_tick = tick.0 as i64,
            owner_bits = owner_bits,
            origin_tick = origin_tick as i64,
            total_count = entities.len() as i64,
            in_rollback = in_rollback,
            entities = ?entities,
            "Multiple active spaceships bullets share the same shot identity"
        );
    }
}

// Predicted/prespawned clients can predict TTL expiry. Interpolated observer bullets wait for the
// authoritative server despawn instead.
pub(crate) fn lifetime_despawner(
    q: Query<(Entity, &BulletLifetime, Has<Predicted>, Has<PreSpawned>)>,
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    server: Query<(), With<Server>>,
) {
    let is_server = !server.is_empty();
    for (e, ttl, is_predicted, is_prespawned) in q.iter() {
        if (timeline.tick() - ttl.origin_tick) > ttl.lifetime
            && (is_server || is_predicted || is_prespawned)
        {
            commands.entity(e).prediction_despawn();
        }
    }
}

// Wall
#[derive(Bundle)]
pub(crate) struct WallBundle {
    color: ColorComponent,
    physics: PhysicsBundle,
    wall: Wall,
    name: Name,
}

#[derive(Component)]
pub(crate) struct Wall {
    pub(crate) start: Vec2,
    pub(crate) end: Vec2,
}

impl WallBundle {
    pub(crate) fn new(start: Vec2, end: Vec2, color: Color) -> Self {
        Self {
            color: ColorComponent(color),
            physics: PhysicsBundle {
                collider: Collider::segment(start, end),
                collider_density: ColliderDensity(1.0),
                rigid_body: RigidBody::Static,
            },
            wall: Wall { start, end },
            name: Name::new("Wall"),
        }
    }
}

// Despawn bullets that collide with something.
//
// Generate a BulletHitEvent so we can modify scores, show visual effects, etc.
//
// Players can't collide with their own bullets.
// this is especially helpful if you are accelerating forwards while shooting, as otherwise you
// might overtake / collide on spawn with your own bullets that spawn in front of you.
pub(crate) fn process_collisions(
    collisions: Collisions,
    bullet_q: Query<(
        &BulletMarker,
        &ColorComponent,
        &Position,
        Has<PreSpawned>,
        Has<Predicted>,
    )>,
    player_q: Query<&Player>,
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    server: Query<(), With<Server>>,
    mut hit_ev_writer: MessageWriter<BulletHitMessage>,
) {
    let is_server = !server.is_empty();
    let tick = timeline.tick();
    // when A and B collide, it can be reported as one of:
    // * A collides with B
    // * B collides with A
    // which is why logic is duplicated twice here
    for contacts in collisions.iter() {
        if let Ok((bullet, col, bullet_pos, is_prespawned, is_predicted)) =
            bullet_q.get(contacts.collider1)
        {
            // Keep unconfirmed client prespawns alive until the server entity can match them.
            if !is_server && is_prespawned {
                continue;
            }
            if !is_server && !is_predicted {
                info!(
                    ?tick,
                    bullet = ?contacts.collider1,
                    "Hide interpolated bullet after local collision"
                );
                hide_interpolated_bullet(&mut commands, contacts.collider1);
                continue;
            }
            if let Ok(owner) = player_q.get(contacts.collider2)
                && bullet.owner == owner.client_id
            {
                // this is our own bullet, don't do anything
                continue;
            }
            // despawn the bullet
            info!(?tick, bullet = ?contacts.collider1, "Hit! Prediction disable bullet");
            commands.entity(contacts.collider1).prediction_despawn();
            let victim_client_id = player_q
                .get(contacts.collider2)
                .map_or(None, |victim_player| Some(victim_player.client_id));

            let ev = BulletHitMessage {
                bullet_owner: bullet.owner,
                victim_client_id,
                position: bullet_pos.0,
                bullet_color: col.0,
            };
            hit_ev_writer.write(ev);
        }
        if let Ok((bullet, col, bullet_pos, is_prespawned, is_predicted)) =
            bullet_q.get(contacts.collider2)
        {
            // Keep unconfirmed client prespawns alive until the server entity can match them.
            if !is_server && is_prespawned {
                continue;
            }
            if !is_server && !is_predicted {
                info!(
                    ?tick,
                    bullet = ?contacts.collider2,
                    "Hide interpolated bullet after local collision"
                );
                hide_interpolated_bullet(&mut commands, contacts.collider2);
                continue;
            }
            if let Ok(owner) = player_q.get(contacts.collider1)
                && bullet.owner == owner.client_id
            {
                // this is our own bullet, don't do anything
                continue;
            }
            info!(?tick, bullet = ?contacts.collider2, "Hit! Prediction disable bullet");
            commands.entity(contacts.collider2).prediction_despawn();
            let victim_client_id = player_q
                .get(contacts.collider1)
                .map_or(None, |victim_player| Some(victim_player.client_id));

            let ev = BulletHitMessage {
                bullet_owner: bullet.owner,
                victim_client_id,
                position: bullet_pos.0,
                bullet_color: col.0,
            };
            hit_ev_writer.write(ev);
        }
    }
}

#[cfg(feature = "gui")]
fn hide_interpolated_bullet(commands: &mut Commands, entity: Entity) {
    commands.entity(entity).insert(Visibility::Hidden);
}

#[cfg(not(feature = "gui"))]
fn hide_interpolated_bullet(_commands: &mut Commands, _entity: Entity) {}

#[cfg(test)]
mod tests {
    use super::*;
    use lightyear::input::leafwing::prelude::LeafwingSnapshot;

    fn action_state<const N: usize>(pressed: [PlayerActions; N]) -> ActionState<PlayerActions> {
        let mut action_state = ActionState::default();
        for action in pressed {
            action_state.press(&action);
        }
        action_state
    }

    #[test]
    fn client_should_fire_allows_first_predicted_shot_with_buffered_input() {
        let mut input_buffer = LeafwingBuffer::<PlayerActions>::default();
        let weapon = Weapon::new(12);
        input_buffer.set(Tick(99), LeafwingSnapshot(action_state([])));
        input_buffer.set(
            Tick(100),
            LeafwingSnapshot(action_state([PlayerActions::Fire])),
        );

        input_buffer.last_remote_tick = Some(Tick(100));

        assert!(client_should_fire(
            &input_buffer,
            &weapon,
            Tick(100),
            true,
            true
        ));
    }

    #[test]
    fn client_should_fire_blocks_remote_shot_without_rebroadcasted_tick() {
        let mut input_buffer = LeafwingBuffer::<PlayerActions>::default();
        let weapon = Weapon::new(12);
        input_buffer.set(Tick(99), LeafwingSnapshot(action_state([])));
        input_buffer.set(
            Tick(100),
            LeafwingSnapshot(action_state([PlayerActions::Fire])),
        );
        input_buffer.last_remote_tick = Some(Tick(99));

        assert!(!client_should_fire(
            &input_buffer,
            &weapon,
            Tick(100),
            true,
            true
        ));
    }

    #[test]
    fn client_should_fire_blocks_local_initial_shot_until_weapon_confirmed() {
        let mut input_buffer = LeafwingBuffer::<PlayerActions>::default();
        let weapon = Weapon::new(12);
        input_buffer.set(Tick(99), LeafwingSnapshot(action_state([])));
        input_buffer.set(
            Tick(100),
            LeafwingSnapshot(action_state([PlayerActions::Fire])),
        );

        assert!(!client_should_fire(
            &input_buffer,
            &weapon,
            Tick(100),
            false,
            false
        ));
    }

    #[test]
    fn client_should_fire_waits_for_previous_input_sample() {
        let mut input_buffer = LeafwingBuffer::<PlayerActions>::default();
        let weapon = Weapon {
            last_fire_tick: Tick(90),
            cooldown: 12,
            bullet_speed: 500.0,
        };
        input_buffer.set(
            Tick(100),
            LeafwingSnapshot(action_state([PlayerActions::Fire])),
        );

        input_buffer.last_remote_tick = Some(Tick(100));

        assert!(!client_should_fire(
            &input_buffer,
            &weapon,
            Tick(100),
            true,
            true
        ));
    }

    #[test]
    fn client_should_fire_requires_fire_pressed_this_tick() {
        let mut input_buffer = LeafwingBuffer::<PlayerActions>::default();
        let weapon = Weapon {
            last_fire_tick: Tick(90),
            cooldown: 12,
            bullet_speed: 500.0,
        };
        input_buffer.set(
            Tick(99),
            LeafwingSnapshot(action_state([PlayerActions::Fire])),
        );
        input_buffer.set(Tick(100), LeafwingSnapshot(action_state([])));

        input_buffer.last_remote_tick = Some(Tick(100));

        assert!(!client_should_fire(
            &input_buffer,
            &weapon,
            Tick(100),
            true,
            true
        ));
    }

    #[test]
    fn effective_last_fire_tick_uses_newer_confirmed_history() {
        let weapon = Weapon {
            last_fire_tick: Tick(409),
            cooldown: 12,
            bullet_speed: 500.0,
        };
        let mut history = ConfirmedHistory::<Weapon>::default();
        history.insert(
            Tick(435),
            Some(Weapon {
                last_fire_tick: Tick(435),
                cooldown: 12,
                bullet_speed: 500.0,
            }),
        );

        assert_eq!(
            effective_last_fire_tick(&weapon, Some(&history), Tick(435)),
            Tick(435)
        );
    }

    #[test]
    fn effective_last_fire_tick_keeps_local_value_without_newer_history() {
        let weapon = Weapon {
            last_fire_tick: Tick(409),
            cooldown: 12,
            bullet_speed: 500.0,
        };
        let mut history = ConfirmedHistory::<Weapon>::default();
        history.insert(
            Tick(383),
            Some(Weapon {
                last_fire_tick: Tick(383),
                cooldown: 12,
                bullet_speed: 500.0,
            }),
        );

        assert_eq!(
            effective_last_fire_tick(&weapon, Some(&history), Tick(435)),
            Tick(409)
        );
    }

    #[test]
    fn bullet_prespawn_hash_is_stable_and_distinguishes_owner_and_tick() {
        let owner = PeerId::Netcode(31);

        assert_eq!(
            bullet_prespawn_hash(owner, Tick(435)),
            bullet_prespawn_hash(owner, Tick(435))
        );
        assert_ne!(
            bullet_prespawn_hash(owner, Tick(435)),
            bullet_prespawn_hash(owner, Tick(436))
        );
        assert_ne!(
            bullet_prespawn_hash(owner, Tick(435)),
            bullet_prespawn_hash(PeerId::Netcode(32), Tick(435))
        );
    }
}
