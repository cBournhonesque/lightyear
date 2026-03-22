use avian2d::prelude::Position;
use bevy::prelude::*;
use leafwing_input_manager::plugin::InputManagerSystem;
use leafwing_input_manager::prelude::ActionState;
use lightyear::prelude::*;
use lightyear_examples_common::automation::{
    HeadlessInputPlugin, env_flag, env_string, sync_pressed_keys,
};

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
        app.add_systems(Update, client::log_players);
    }
}

#[cfg(feature = "server")]
pub struct AutomationServerPlugin;

#[cfg(feature = "server")]
impl Plugin for AutomationServerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(server::DebugSettings::from_env());
        app.add_systems(Update, server::log_players);
    }
}

#[cfg(feature = "client")]
mod client {
    use super::*;

    #[derive(Resource, Clone, Default)]
    pub(super) struct AutomationSettings {
        pressed_keys: Vec<KeyCode>,
        log_client: bool,
    }

    impl AutomationSettings {
        fn from_env() -> Self {
            Self {
                pressed_keys: parse_move_keys(env_string("LIGHTYEAR_AUTOMOVE")),
                log_client: env_flag("LIGHTYEAR_LOG_CLIENT"),
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
        mut query: Query<&mut ActionState<PlayerActions>>,
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

    pub(super) fn log_players(
        settings: Option<Res<AutomationSettings>>,
        query: Query<(&PlayerId, &Position, Has<Controlled>), Changed<Position>>,
    ) {
        let Some(settings) = settings else {
            return;
        };
        if !settings.log_client {
            return;
        }
        for (player_id, position, controlled) in &query {
            info!(
                ?player_id,
                position = ?position.0,
                controlled,
                "deterministic_replication client player update"
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
}

#[cfg(feature = "server")]
mod server {
    use super::*;

    #[derive(Resource, Default)]
    pub(super) struct DebugSettings {
        log_server: bool,
    }

    impl DebugSettings {
        pub(super) fn from_env() -> Self {
            Self {
                log_server: env_flag("LIGHTYEAR_LOG_SERVER"),
            }
        }
    }

    pub(super) fn log_players(
        settings: Res<DebugSettings>,
        query: Query<&PlayerId, Added<PlayerId>>,
    ) {
        if !settings.log_server {
            return;
        }
        for player_id in &query {
            info!(
                ?player_id,
                "deterministic_replication server player spawned"
            );
        }
    }
}
