use bevy::prelude::*;
use lightyear::prelude::*;

use crate::protocol::{PlayerId, PlayerPosition};

pub(crate) fn register_debug_systems(app: &mut App) {
    app.add_systems(FixedPostUpdate, emit_fixed_post_positions);
    app.add_systems(Update, emit_confirmed_positions);
    app.add_systems(PostUpdate, emit_interpolated_positions);
}

fn emit_confirmed_positions(
    timeline: Res<LocalTimeline>,
    players: Query<(Entity, &PlayerPosition), (With<PlayerId>, Changed<PlayerPosition>)>,
) {
    let tick = timeline.tick();
    for (entity, position) in players.iter() {
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::Update,
            "Update",
            "confirmed_position",
            tick = ?tick,
            entity = ?entity,
            position = ?position,
            "confirmed position updated"
        );
    }
}

fn emit_interpolated_positions(
    timeline: Res<LocalTimeline>,
    players: Query<(Entity, &PlayerPosition), With<Interpolated>>,
) {
    let tick = timeline.tick();
    for (entity, position) in players.iter() {
        lightyear_debug_event!(
            DebugCategory::Interpolation,
            DebugSamplePoint::PostUpdate,
            "PostUpdate",
            "interpolated_position",
            tick = ?tick,
            entity = ?entity,
            position = ?position,
            "interpolated position"
        );
    }
}

fn emit_fixed_post_positions(
    timeline: Res<LocalTimeline>,
    players: Query<(Entity, &PlayerPosition), With<PlayerId>>,
) {
    let tick = timeline.tick();
    for (entity, position) in players.iter() {
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::FixedPostUpdate,
            "FixedPostUpdate",
            "fixed_post_position",
            tick = ?tick,
            entity = ?entity,
            position = ?position,
            "Player after movement"
        );
    }
}

#[cfg(feature = "client")]
pub(crate) mod client {
    use super::*;

    pub(crate) fn mark_debug_players(
        mut commands: Commands,
        query: Query<(Entity, Has<Predicted>, Has<Interpolated>), Added<PlayerId>>,
    ) {
        for (entity, predicted, interpolated) in &query {
            if predicted || interpolated {
                commands.entity(entity).insert(
                    LightyearDebug::component_at::<PlayerPosition>([DebugSamplePoint::Update])
                        .with_component_at::<PlayerId>([DebugSamplePoint::Update]),
                );
            }
        }
    }
}

#[cfg(feature = "server")]
pub(crate) mod server {
    use super::*;

    pub(crate) fn mark_debug_players(
        mut commands: Commands,
        query: Query<Entity, Added<PlayerId>>,
    ) {
        for entity in &query {
            commands.entity(entity).insert(
                LightyearDebug::component_at::<PlayerPosition>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<PlayerId>([DebugSamplePoint::FixedUpdate]),
            );
        }
    }
}
