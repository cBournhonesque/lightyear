use avian2d::prelude::*;
use bevy::prelude::*;
use lightyear::prelude::*;
use std::collections::HashMap;

use crate::protocol::{BulletMarker, InterpolatedBot, PlayerId, PlayerMarker, PredictedBot, Score};

#[derive(Resource, Default)]
struct BulletDebugRegistry {
    bullets: HashMap<Entity, BulletMarker>,
}

pub(crate) fn register_debug_systems(app: &mut App) {
    app.init_resource::<BulletDebugRegistry>();
    app.add_systems(FixedLast, emit_fixed_last_entities);
    app.add_systems(FixedLast, emit_predicted_bot_transform);
    app.add_systems(
        PostUpdate,
        emit_bullet_post_update_state.after(TransformSystems::Propagate),
    );
    app.add_systems(
        PostUpdate,
        (
            track_bullet_lifecycle_added,
            track_bullet_lifecycle_removed,
            detect_duplicate_bullets,
        )
            .chain(),
    );
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

fn emit_fixed_last_entities(
    timeline: Res<LocalTimeline>,
    player: Query<(Entity, &Transform), (With<PlayerMarker>, With<PlayerId>)>,
    bullets: Query<
        (
            Entity,
            &BulletMarker,
            &Position,
            &LinearVelocity,
            &Transform,
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
    for (
        entity,
        marker,
        position,
        velocity,
        transform,
        is_predicted,
        is_interpolated,
        is_prespawned,
        is_replicate,
        is_replicated,
    ) in bullets.iter()
    {
        lightyear_debug_event!(
            DebugCategory::Prediction,
            DebugSamplePoint::FixedLast,
            "FixedLast",
            "bullet_state_fixed_last",
            local_tick = tick.0 as i64,
            entity = ?entity,
            shooter = ?marker.shooter,
            shooter_bits = marker.shooter.to_bits(),
            fire_tick = marker.fire_tick.0 as i64,
            prespawn_hash = marker.prespawn_hash,
            position = ?position,
            velocity = ?velocity,
            transform = ?transform.translation.truncate(),
            is_predicted = is_predicted,
            is_interpolated = is_interpolated,
            is_prespawned = is_prespawned,
            is_replicate = is_replicate,
            is_replicated = is_replicated,
            "Bullet state after fixed update"
        );
    }
}

fn emit_bullet_post_update_state(
    timeline: Res<LocalTimeline>,
    interpolation_timeline: Query<&InterpolationTimeline>,
    bullets: Query<
        (
            Entity,
            &BulletMarker,
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
            .and_then(|history| history.newest_present().map(|(tick, _)| tick.0 as i64));
        let position_visual_ready = position_history_end_tick.is_some()
            && position_history_start_tick
                .zip(interpolation_tick)
                .is_some_and(|(start_tick, interpolation_tick)| interpolation_tick >= start_tick);
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::PostUpdate,
            "PostUpdate",
            "fps_bullet_post_update_state",
            local_tick = tick.0 as i64,
            entity = ?entity,
            shooter = ?marker.shooter,
            shooter_bits = marker.shooter.to_bits(),
            fire_tick = marker.fire_tick.0 as i64,
            prespawn_hash = marker.prespawn_hash,
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
            "FPS bullet state after transform propagation"
        );
    }
}

fn track_bullet_lifecycle_added(
    timeline: Res<LocalTimeline>,
    mut registry: ResMut<BulletDebugRegistry>,
    bullets: Query<
        (
            Entity,
            &BulletMarker,
            &Position,
            &LinearVelocity,
            Has<Predicted>,
            Has<Interpolated>,
            Has<PreSpawned>,
            Has<Replicate>,
            Has<Replicated>,
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
        position,
        velocity,
        is_predicted,
        is_interpolated,
        is_prespawned,
        is_replicate,
        is_replicated,
    ) in &bullets
    {
        registry.bullets.insert(entity, *marker);
        lightyear_debug_event!(
            DebugCategory::Prediction,
            DebugSamplePoint::PostUpdate,
            "PostUpdate",
            "bullet_lifecycle_added",
            local_tick = tick.0 as i64,
            entity = ?entity,
            shooter = ?marker.shooter,
            shooter_bits = marker.shooter.to_bits(),
            fire_tick = marker.fire_tick.0 as i64,
            prespawn_hash = marker.prespawn_hash,
            position = ?position,
            velocity = ?velocity,
            is_predicted = is_predicted,
            is_interpolated = is_interpolated,
            is_prespawned = is_prespawned,
            is_replicate = is_replicate,
            is_replicated = is_replicated,
            in_rollback = in_rollback,
            "Bullet lifecycle added"
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
        let marker = registry.bullets.remove(&entity);
        lightyear_debug_event!(
            DebugCategory::Prediction,
            DebugSamplePoint::PostUpdate,
            "PostUpdate",
            "bullet_lifecycle_removed",
            local_tick = tick.0 as i64,
            entity = ?entity,
            shooter = ?marker.map(|m| m.shooter),
            shooter_bits = marker.map(|m| m.shooter.to_bits()),
            fire_tick = marker.map(|m| m.fire_tick.0 as i64),
            prespawn_hash = marker.map(|m| m.prespawn_hash),
            in_rollback = in_rollback,
            "Bullet lifecycle removed"
        );
    }
}

fn detect_duplicate_bullets(
    timeline: Res<LocalTimeline>,
    bullets: Query<
        (
            Entity,
            &BulletMarker,
            &Position,
            Option<&Visibility>,
            Has<Mesh2d>,
            Has<Predicted>,
            Has<Interpolated>,
            Has<PreSpawned>,
            Has<Replicate>,
            Has<Replicated>,
        ),
        With<BulletMarker>,
    >,
    rollback: Query<(), With<Rollback>>,
) {
    #[derive(Debug)]
    struct BulletDuplicateState {
        entity: Entity,
        position: Vec2,
        is_visible: bool,
        has_visual: bool,
        is_predicted: bool,
        is_interpolated: bool,
        is_prespawned: bool,
        is_replicate: bool,
        is_replicated: bool,
    }

    let tick = timeline.tick();
    let in_rollback = !rollback.is_empty();
    let mut groups: HashMap<u64, Vec<BulletDuplicateState>> = HashMap::new();
    for (
        entity,
        marker,
        position,
        visibility,
        has_visual,
        is_predicted,
        is_interpolated,
        is_prespawned,
        is_replicate,
        is_replicated,
    ) in &bullets
    {
        let is_visible =
            visibility.is_none_or(|visibility| !matches!(visibility, Visibility::Hidden));
        groups
            .entry(marker.prespawn_hash)
            .or_default()
            .push(BulletDuplicateState {
                entity,
                position: position.0,
                is_visible,
                has_visual,
                is_predicted,
                is_interpolated,
                is_prespawned,
                is_replicate,
                is_replicated,
            });
    }
    for (prespawn_hash, entities) in groups {
        if entities.len() <= 1 {
            continue;
        }
        let visible_count = entities.iter().filter(|state| state.is_visible).count();
        let visual_count = entities.iter().filter(|state| state.has_visual).count();
        lightyear_debug_event!(
            DebugCategory::Prediction,
            DebugSamplePoint::PostUpdate,
            "PostUpdate",
            "bullet_duplicate_active",
            local_tick = tick.0 as i64,
            prespawn_hash = prespawn_hash,
            total_count = entities.len() as i64,
            visible_count = visible_count as i64,
            visual_count = visual_count as i64,
            in_rollback = in_rollback,
            entities = ?entities,
            "Multiple active bullets share the same shot identity"
        );
    }
}

#[cfg(feature = "client")]
pub(crate) mod client {
    use super::*;

    pub(crate) fn mark_debug_players(
        mut commands: Commands,
        players: Query<Entity, (With<PlayerMarker>, Added<PlayerId>)>,
    ) {
        for entity in &players {
            commands.entity(entity).insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::Update])
                    .with_component_at::<Score>([DebugSamplePoint::Update]),
            );
        }
    }

    pub(crate) fn mark_debug_bullets(
        mut commands: Commands,
        bullets: Query<Entity, Added<BulletMarker>>,
    ) {
        for entity in &bullets {
            commands.entity(entity).insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::Update])
                    .with_component_at::<PlayerId>([DebugSamplePoint::Update]),
            );
        }
    }
}

#[cfg(feature = "server")]
pub(crate) mod server {
    use super::*;

    pub(crate) fn mark_debug_players(
        mut commands: Commands,
        players: Query<Entity, (With<PlayerMarker>, Added<PlayerId>)>,
    ) {
        for entity in &players {
            commands.entity(entity).insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<Score>([DebugSamplePoint::FixedUpdate]),
            );
        }
    }

    pub(crate) fn mark_debug_bullets(
        mut commands: Commands,
        bullets: Query<Entity, Added<BulletMarker>>,
    ) {
        for entity in &bullets {
            commands.entity(entity).insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<PlayerId>([DebugSamplePoint::FixedUpdate]),
            );
        }
    }
}
