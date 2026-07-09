use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::*;
use lightyear_frame_interpolation::FrameInterpolationHistory;

use crate::protocol::{
    Direction, Inputs, PlayerId, PlayerParent, PlayerPosition, TailLength, TailPoints,
};

#[cfg(feature = "client")]
pub(crate) mod client {
    use super::*;

    pub(crate) fn mark_debug_players(
        mut commands: Commands,
        query: Query<(Entity, Has<Predicted>, Has<Interpolated>), Added<PlayerId>>,
    ) {
        for (entity, predicted, interpolated) in &query {
            if predicted || interpolated {
                commands
                    .entity(entity)
                    .insert(LightyearDebug::component_at::<PlayerPosition>([
                        DebugSamplePoint::Update,
                    ]));
            }
        }
    }

    pub(crate) fn mark_debug_tails(
        mut commands: Commands,
        tails: Query<Entity, Added<TailPoints>>,
    ) {
        for entity in &tails {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<TailPoints>([
                    DebugSamplePoint::Update,
                ]));
        }
    }

    pub(crate) fn emit_interpolated_snake_consistency(
        players: Query<
            (
                Entity,
                &PlayerId,
                &PlayerPosition,
                Option<&ConfirmedHistory<PlayerPosition>>,
            ),
            With<Interpolated>,
        >,
        tails: Query<
            (
                Entity,
                &PlayerParent,
                &TailPoints,
                &TailLength,
                Option<&ConfirmedHistory<TailPoints>>,
            ),
            With<Interpolated>,
        >,
    ) {
        for (tail_entity, parent, tail, tail_length, tail_history) in &tails {
            let Ok((head_entity, player_id, head, head_history)) = players.get(parent.0) else {
                lightyear_debug_event!(
                    DebugCategory::Component,
                    DebugSamplePoint::Update,
                    "Update",
                    "replication_groups_missing_head",
                    tail_entity = ?tail_entity,
                    parent = ?parent.0,
                    "interpolated tail missing head"
                );
                continue;
            };

            let invalid = snake_has_diagonal(head.0, tail)
                || !front_matches_head_direction(head.0, tail)
                || history_ticks_mismatch(head_history, tail_history);
            if !invalid {
                continue;
            }

            lightyear_debug_event!(
                DebugCategory::Component,
                DebugSamplePoint::Update,
                "Update",
                "replication_groups_snake_inconsistency",
                ?player_id,
                ?head_entity,
                ?tail_entity,
                head = ?head.0,
                first_point = ?tail.0.front().map(|(point, _)| *point),
                head_history = ?history_ticks(head_history),
                tail_history = ?history_ticks(tail_history),
                tail_len = tail_total_length(head.0, tail),
                expected_len = tail_length.0,
                ?tail,
                "replication_groups client interpolated snake inconsistency"
            );
        }
    }

    pub(crate) fn debug_pre_visual_interpolation(
        timeline: Res<LocalTimeline>,
        query: Query<(&PlayerPosition, &FrameInterpolationHistory<PlayerPosition>)>,
    ) {
        let tick = timeline.tick();
        for (position, interpolate_status) in query.iter() {
            trace!(
                ?tick,
                ?position,
                ?interpolate_status,
                "pre visual interpolation"
            );
        }
    }

    pub(crate) fn debug_post_visual_interpolation(
        timeline: Res<LocalTimeline>,
        query: Query<(&PlayerPosition, &FrameInterpolationHistory<PlayerPosition>)>,
    ) {
        let tick = timeline.tick();
        for (position, interpolate_status) in query.iter() {
            trace!(
                ?tick,
                ?position,
                ?interpolate_status,
                "post visual interpolation"
            );
        }
    }

    pub(crate) fn debug_interpolate(
        timeline: Res<LocalTimeline>,
        parent_query: Query<(&ConfirmedHistory<PlayerPosition>,)>,
        tail_query: Query<(&PlayerParent, &ConfirmedHistory<TailPoints>)>,
    ) {
        debug!(tick = ?timeline.tick(), "interpolation debug");
        for (parent, tail_history) in tail_query.iter() {
            let parent_history = parent_query
                .get(parent.0)
                .expect("Tail entity has no parent entity!");
            debug!(?parent_history, "parent");
            debug!(?tail_history, "tail");
        }
    }

    fn history_ticks<C>(
        history: Option<&ConfirmedHistory<C>>,
    ) -> Option<(Option<Tick>, Option<Tick>)> {
        history.map(|history| {
            (
                history.start_present().map(|(tick, _)| tick),
                history.newest_present().map(|(tick, _)| tick),
            )
        })
    }

    fn history_ticks_mismatch(
        head_history: Option<&ConfirmedHistory<PlayerPosition>>,
        tail_history: Option<&ConfirmedHistory<TailPoints>>,
    ) -> bool {
        history_ticks(head_history) != history_ticks(tail_history)
    }

    fn front_matches_head_direction(head: Vec2, tail: &TailPoints) -> bool {
        let Some((front_point, front_dir)) = tail.0.front() else {
            return true;
        };
        Direction::from_points(*front_point, head).is_none_or(|dir| dir == *front_dir)
    }

    fn snake_has_diagonal(head: Vec2, tail: &TailPoints) -> bool {
        let Some((front_point, _)) = tail.0.front() else {
            return false;
        };
        is_diagonal(head, *front_point)
            || tail
                .0
                .iter()
                .zip(tail.0.iter().skip(1))
                .any(|(start, end)| is_diagonal(start.0, end.0))
    }

    fn tail_total_length(head: Vec2, tail: &TailPoints) -> f32 {
        let Some((front_point, _)) = tail.0.front() else {
            return 0.0;
        };
        let mut length = (head - *front_point).length();
        for (start, end) in tail.0.iter().zip(tail.0.iter().skip(1)) {
            length += (start.0 - end.0).length();
        }
        length
    }

    fn is_diagonal(a: Vec2, b: Vec2) -> bool {
        a.x != b.x && a.y != b.y
    }
}

#[cfg(feature = "server")]
pub(crate) mod server {
    use super::*;

    pub(crate) fn mark_debug_players(
        mut commands: Commands,
        players: Query<Entity, Added<PlayerId>>,
    ) {
        for entity in &players {
            commands.entity(entity).insert(
                LightyearDebug::component_at::<PlayerPosition>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<ActionState<Inputs>>([DebugSamplePoint::FixedUpdate]),
            );
        }
    }

    pub(crate) fn mark_debug_tails(
        mut commands: Commands,
        tails: Query<Entity, Added<TailPoints>>,
    ) {
        for entity in &tails {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<TailPoints>([
                    DebugSamplePoint::FixedUpdate,
                ]));
        }
    }
}
