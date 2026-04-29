use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::*;
use lightyear_examples_common::automation::{env_string, sync_pressed_keys, HeadlessInputPlugin};

use crate::protocol::{
    Direction, Inputs, PlayerId, PlayerParent, PlayerPosition, TailLength, TailPoints,
};

#[cfg(feature = "client")]
pub struct AutomationClientPlugin;

#[cfg(feature = "client")]
impl Plugin for AutomationClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(HeadlessInputPlugin);
        app.add_systems(Startup, client::init_settings);
        app.add_systems(First, client::drive_keys);
        app.add_systems(
            Update,
            (
                client::mark_debug_players,
                client::mark_debug_tails,
                client::emit_interpolated_snake_consistency,
            ),
        );
    }
}

#[cfg(feature = "server")]
pub struct AutomationServerPlugin;

#[cfg(feature = "server")]
impl Plugin for AutomationServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (server::mark_debug_players, server::mark_debug_tails),
        );
    }
}

#[cfg(feature = "client")]
mod client {
    use super::*;

    #[derive(Resource, Clone, Default)]
    pub(super) struct AutomationSettings {
        pressed_keys: Vec<KeyCode>,
        move_script: Option<Vec<(f32, Vec<KeyCode>)>>,
    }

    impl AutomationSettings {
        fn from_env() -> Self {
            Self {
                pressed_keys: parse_move_keys(env_string("LIGHTYEAR_AUTOMOVE")),
                move_script: parse_move_script(env_string("LIGHTYEAR_AUTOMOVE_SCRIPT")),
            }
        }
    }

    pub(super) fn init_settings(mut commands: Commands) {
        commands.insert_resource(AutomationSettings::from_env());
    }

    pub(super) fn drive_keys(
        settings: Res<AutomationSettings>,
        time: Res<Time>,
        mut previous: Local<Vec<KeyCode>>,
        mut buttons: ResMut<ButtonInput<KeyCode>>,
    ) {
        let keys = if let Some(script) = &settings.move_script {
            script_keys(script, time.elapsed_secs())
        } else {
            settings.pressed_keys.clone()
        };
        sync_pressed_keys(&mut buttons, &mut previous, &keys);
    }

    pub(super) fn mark_debug_players(
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

    pub(super) fn mark_debug_tails(
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

    pub(super) fn emit_interpolated_snake_consistency(
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

    fn parse_move_keys(value: Option<String>) -> Vec<KeyCode> {
        let mut keys = Vec::new();
        let Some(value) = value else {
            return keys;
        };
        for token in value.split(',') {
            match token.trim().to_ascii_lowercase().as_str() {
                "up" | "u" => keys.push(KeyCode::KeyW),
                "down" | "d" => keys.push(KeyCode::KeyS),
                "left" | "l" => keys.push(KeyCode::KeyA),
                "right" | "r" => keys.push(KeyCode::KeyD),
                "" | "none" => {}
                other => warn!(token = other, "Ignoring unknown LIGHTYEAR_AUTOMOVE token"),
            }
        }
        keys
    }

    fn parse_move_script(value: Option<String>) -> Option<Vec<(f32, Vec<KeyCode>)>> {
        let value = value?;
        let mut script = Vec::new();
        for chunk in value.split(',') {
            let chunk = chunk.trim();
            if chunk.is_empty() {
                continue;
            }
            let Some((time_str, dir_str)) = chunk.split_once(':') else {
                warn!(?chunk, "Ignoring malformed LIGHTYEAR_AUTOMOVE_SCRIPT chunk");
                continue;
            };
            let Ok(time) = time_str.trim().parse::<f32>() else {
                warn!(
                    ?chunk,
                    "Ignoring LIGHTYEAR_AUTOMOVE_SCRIPT chunk with bad time"
                );
                continue;
            };
            let keys = parse_move_keys(Some(dir_str.trim().to_string()));
            script.push((time, keys));
        }
        script.sort_by(|a, b| a.0.total_cmp(&b.0));
        (!script.is_empty()).then_some(script)
    }

    fn script_keys(script: &[(f32, Vec<KeyCode>)], elapsed: f32) -> Vec<KeyCode> {
        let mut selected = Vec::new();
        for (start, keys) in script {
            if elapsed >= *start {
                selected = keys.clone();
            } else {
                break;
            }
        }
        selected
    }

    fn history_ticks<C>(
        history: Option<&ConfirmedHistory<C>>,
    ) -> Option<(Option<Tick>, Option<Tick>)> {
        history.map(|history| {
            (
                history.start().map(|(tick, _)| tick),
                history.end().map(|(tick, _)| tick),
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
mod server {
    use super::*;

    pub(super) fn mark_debug_players(
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

    pub(super) fn mark_debug_tails(
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
