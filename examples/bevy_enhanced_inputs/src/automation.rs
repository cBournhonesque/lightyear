use bevy::prelude::*;
use lightyear::prelude::*;
use lightyear_examples_common::automation::{env_string, sync_pressed_keys, HeadlessInputPlugin};

use crate::protocol::{PlayerId, PlayerPosition};

#[cfg(feature = "client")]
pub struct AutomationClientPlugin;

#[cfg(feature = "client")]
impl Plugin for AutomationClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(HeadlessInputPlugin);
        app.add_systems(Startup, client::init_settings);
        app.add_systems(First, client::drive_keys);
        app.add_systems(Update, client::mark_debug_players);
    }
}

#[cfg(feature = "server")]
pub struct AutomationServerPlugin;

#[cfg(feature = "server")]
impl Plugin for AutomationServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, server::mark_debug_players);
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
                commands.entity(entity).insert(
                    LightyearDebug::component_at::<PlayerPosition>([DebugSamplePoint::Update])
                        .with_component_at::<PlayerId>([DebugSamplePoint::Update]),
                );
            }
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
            script.push((time, parse_move_keys(Some(dir_str.trim().to_string()))));
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
}

#[cfg(feature = "server")]
mod server {
    use super::*;

    pub(super) fn mark_debug_players(
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
