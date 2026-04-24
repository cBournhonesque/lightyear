use avian2d::prelude::Position;
use bevy::prelude::*;
use leafwing_input_manager::plugin::InputManagerSystem;
use leafwing_input_manager::prelude::{ActionState, InputMap};
use lightyear::prelude::*;
use lightyear_examples_common::automation::{HeadlessInputPlugin, env_string, sync_pressed_keys};

use crate::protocol::{PlayerActions, PlayerId};

#[cfg(feature = "client")]
pub struct AutomationClientPlugin;

#[cfg(feature = "client")]
impl Plugin for AutomationClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(HeadlessInputPlugin);
        app.add_systems(Startup, client::init_settings);
        app.add_systems(First, client::drive_keys);
        app.add_systems(
            PreUpdate,
            client::drive_action_state.in_set(InputManagerSystem::ManualControl),
        );
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
    }

    impl AutomationSettings {
        fn from_env() -> Self {
            Self {
                pressed_keys: parse_move_keys(env_string("LIGHTYEAR_AUTOMOVE")),
            }
        }
    }

    pub(super) fn init_settings(mut commands: Commands) {
        commands.insert_resource(AutomationSettings::from_env());
    }

    pub(super) fn drive_keys(
        settings: Res<AutomationSettings>,
        mut previous: Local<Vec<KeyCode>>,
        mut buttons: ResMut<ButtonInput<KeyCode>>,
    ) {
        sync_pressed_keys(&mut buttons, &mut previous, &settings.pressed_keys);
    }

    pub(super) fn drive_action_state(
        settings: Res<AutomationSettings>,
        mut query: Query<&mut ActionState<PlayerActions>, With<InputMap<PlayerActions>>>,
    ) {
        for mut action_state in &mut query {
            for action in [
                PlayerActions::Up,
                PlayerActions::Down,
                PlayerActions::Left,
                PlayerActions::Right,
            ] {
                action_state.release(&action);
            }
            for key in &settings.pressed_keys {
                match key {
                    KeyCode::KeyW => action_state.press(&PlayerActions::Up),
                    KeyCode::KeyS => action_state.press(&PlayerActions::Down),
                    KeyCode::KeyA => action_state.press(&PlayerActions::Left),
                    KeyCode::KeyD => action_state.press(&PlayerActions::Right),
                    _ => {}
                }
            }
        }
    }

    pub(super) fn mark_debug_players(
        mut commands: Commands,
        query: Query<Entity, (Added<PlayerId>, With<Position>)>,
    ) {
        for entity in &query {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<Position>([
                    DebugSamplePoint::Update,
                ]));
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
}

#[cfg(feature = "server")]
mod server {
    use super::*;

    pub(super) fn mark_debug_players(
        mut commands: Commands,
        query: Query<Entity, (Added<PlayerId>, With<Position>)>,
    ) {
        for entity in &query {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<Position>([
                    DebugSamplePoint::Update,
                ]));
        }
    }
}
