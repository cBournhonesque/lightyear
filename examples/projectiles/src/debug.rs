use avian2d::prelude::*;
use bevy::prelude::*;
use lightyear::prelude::*;

use crate::protocol::{
    Bot, BulletMarker, ClientContext, GameReplicationMode, HitscanVisual, PlayerId, PlayerMarker,
    ProjectileReplicationMode, Score,
};

pub(crate) fn register_debug_systems(app: &mut App) {
    app.add_systems(FixedLast, emit_fixed_last_players);
    app.add_systems(Last, emit_last_entities);
}

fn emit_fixed_last_players(
    timeline: Res<LocalTimeline>,
    player: Query<(Entity, &Position), (With<PlayerMarker>, With<PlayerId>)>,
) {
    let tick = timeline.tick();
    for (entity, pos) in player.iter() {
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::FixedLast,
            "FixedLast",
            "player_after_fixed_update",
            tick = ?tick,
            entity = ?entity,
            position = ?pos,
            "Player after fixed update"
        );
    }
}

fn emit_last_entities(
    timeline: Res<LocalTimeline>,
    interpolation_timeline: Query<&InterpolationTimeline>,
    player: Query<
        (
            Entity,
            Option<&Position>,
            Option<&Rotation>,
            Option<&Transform>,
        ),
        (With<PlayerMarker>, With<PlayerId>, With<Bot>),
    >,
    interpolated_bullet: Query<
        (
            Entity,
            Option<&Position>,
            &ConfirmedHistory<Position>,
            Option<&Transform>,
        ),
        (With<BulletMarker>, With<Interpolated>),
    >,
) {
    let tick = timeline.tick();
    let interpolate_tick = interpolation_timeline.iter().next().map(|t| t.tick());
    for (entity, pos, rotation, transform) in player.iter() {
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::Last,
            "Last",
            "player_after_last",
            tick = ?tick,
            entity = ?entity,
            position = ?pos,
            rotation = ?rotation,
            transform = ?transform.map(|t| t.translation.truncate()),
            "Player after last"
        );
    }
    for (entity, position, history, transform) in interpolated_bullet.iter() {
        lightyear_debug_event!(
            DebugCategory::Interpolation,
            DebugSamplePoint::Last,
            "Last",
            "interpolated_bullet_after_last",
            tick = ?tick,
            interpolation_tick = ?interpolate_tick,
            entity = ?entity,
            position = ?position,
            history = ?history,
            transform = ?transform.map(|t| t.translation.truncate()),
            "Bullet after fixed update"
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
            commands.entity(entity).try_insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::Update])
                    .with_component_at::<PlayerId>([DebugSamplePoint::Update])
                    .with_component_at::<Score>([DebugSamplePoint::Update]),
            );
        }
    }

    pub(crate) fn mark_debug_bullets(
        mut commands: Commands,
        bullets: Query<Entity, Added<BulletMarker>>,
    ) {
        for entity in &bullets {
            commands.entity(entity).try_insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::Update])
                    .with_component_at::<BulletMarker>([DebugSamplePoint::Update])
                    .with_component_at::<HitscanVisual>([DebugSamplePoint::Update]),
            );
        }
    }

    pub(crate) fn mark_debug_modes(
        mut commands: Commands,
        modes: Query<Entity, Added<ClientContext>>,
    ) {
        for entity in &modes {
            commands.entity(entity).try_insert(
                LightyearDebug::component_at::<GameReplicationMode>([DebugSamplePoint::Update])
                    .with_component_at::<ProjectileReplicationMode>([DebugSamplePoint::Update]),
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
            commands.entity(entity).try_insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::Update])
                    .with_component_at::<PlayerId>([DebugSamplePoint::Update])
                    .with_component_at::<Score>([DebugSamplePoint::Update]),
            );
        }
    }

    pub(crate) fn mark_debug_bullets(
        mut commands: Commands,
        bullets: Query<Entity, Added<BulletMarker>>,
    ) {
        for entity in &bullets {
            commands.entity(entity).try_insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::Update])
                    .with_component_at::<BulletMarker>([DebugSamplePoint::Update])
                    .with_component_at::<HitscanVisual>([DebugSamplePoint::Update]),
            );
        }
    }

    pub(crate) fn mark_debug_modes(
        mut commands: Commands,
        modes: Query<Entity, Added<ClientContext>>,
    ) {
        for entity in &modes {
            commands.entity(entity).try_insert(
                LightyearDebug::component_at::<GameReplicationMode>([DebugSamplePoint::Update])
                    .with_component_at::<ProjectileReplicationMode>([DebugSamplePoint::Update]),
            );
        }
    }
}
