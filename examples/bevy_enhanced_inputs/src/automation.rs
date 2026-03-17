use bevy::prelude::*;
use lightyear::prelude::*;
use lightyear_examples_common::automation::{
    env_flag, env_string, sync_pressed_keys, HeadlessInputPlugin,
};

use crate::protocol::{PlayerId, PlayerPosition};

#[cfg(feature = "client")]
pub struct AutomationClientPlugin;

#[cfg(feature = "client")]
impl Plugin for AutomationClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(HeadlessInputPlugin);
        app.add_systems(Startup, client::init_settings);
        app.add_systems(First, client::drive_keys);
        app.add_systems(Update, client::log_positions);
    }
}

#[cfg(feature = "server")]
pub struct AutomationServerPlugin;

#[cfg(feature = "server")]
impl Plugin for AutomationServerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(server::DebugSettings::from_env());
        app.add_systems(FixedUpdate, server::log_positions);
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

    pub(super) fn log_positions(
        settings: Option<Res<AutomationSettings>>,
        query: Query<
            (
                &PlayerId,
                &PlayerPosition,
                Has<Predicted>,
                Has<Interpolated>,
                Has<Controlled>,
            ),
            Changed<PlayerPosition>,
        >,
    ) {
        let Some(settings) = settings else {
            return;
        };
        if !settings.log_client {
            return;
        }
        for (player_id, position, predicted, interpolated, controlled) in &query {
            info!(
                ?player_id,
                position = ?position.0,
                predicted,
                interpolated,
                controlled,
                "bevy_enhanced_inputs client position update"
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

    pub(super) fn log_positions(
        settings: Res<DebugSettings>,
        query: Query<(&PlayerId, &PlayerPosition), Changed<PlayerPosition>>,
    ) {
        if !settings.log_server {
            return;
        }
        for (player_id, position) in &query {
            info!(
                ?player_id,
                position = ?position.0,
                "bevy_enhanced_inputs server position update"
            );
        }
    }
}
