use bevy::prelude::*;
use bevy_enhanced_input::action::Action;
use lightyear::prelude::*;
use lightyear_examples_common::automation::{
    env_flag, env_string, sync_pressed_keys, HeadlessInputPlugin,
};

use crate::protocol::{CursorPosition, PlayerId, PlayerPosition, SpawnPlayer};

#[cfg(feature = "client")]
pub struct AutomationClientPlugin;

#[cfg(feature = "client")]
impl Plugin for AutomationClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(HeadlessInputPlugin);
        app.add_systems(Startup, client::init_settings);
        app.add_systems(First, client::drive_keys);
        app.add_systems(Update, (client::move_cursor, client::log_entities));
    }
}

#[cfg(feature = "server")]
pub struct AutomationServerPlugin;

#[cfg(feature = "server")]
impl Plugin for AutomationServerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(server::DebugSettings::from_env());
        app.add_systems(Update, server::log_entities);
    }
}

#[cfg(feature = "client")]
mod client {
    use super::*;

    #[derive(Resource, Clone, Default)]
    pub(super) struct AutomationSettings {
        pressed_keys: Vec<KeyCode>,
        auto_spawn: bool,
        log_client: bool,
    }

    #[derive(Default)]
    pub(super) struct SpawnPulse {
        done: bool,
    }

    impl AutomationSettings {
        fn from_env() -> Self {
            Self {
                pressed_keys: parse_keys(env_string("LIGHTYEAR_AUTOMOVE")),
                auto_spawn: env_flag("LIGHTYEAR_AUTOSPAWN"),
                log_client: env_flag("LIGHTYEAR_LOG_CLIENT"),
            }
        }
    }

    pub(super) fn init_settings(mut commands: Commands) {
        commands.insert_resource(AutomationSettings::from_env());
    }

    pub(super) fn drive_keys(
        settings: Res<AutomationSettings>,
        spawn_actions: Query<(), With<Action<SpawnPlayer>>>,
        mut pulse: Local<SpawnPulse>,
        mut previous: Local<Vec<KeyCode>>,
        mut buttons: ResMut<ButtonInput<KeyCode>>,
    ) {
        let mut keys = settings.pressed_keys.clone();
        if settings.auto_spawn && !pulse.done && !spawn_actions.is_empty() {
            keys.push(KeyCode::Space);
            pulse.done = true;
        }
        sync_pressed_keys(&mut buttons, &mut previous, &keys);
    }

    pub(super) fn move_cursor(
        time: Res<Time>,
        mut cursors: Query<&mut CursorPosition, (With<Replicate>, Without<Replicated>)>,
    ) {
        let t = time.elapsed_secs();
        let x = (t * 80.0).sin() * 200.0;
        let y = (t * 40.0).cos() * 100.0;
        for mut cursor in &mut cursors {
            cursor.set_if_neq(CursorPosition(Vec2::new(x, y)));
        }
    }

    pub(super) fn log_entities(
        settings: Option<Res<AutomationSettings>>,
        cursors: Query<(&PlayerId, &CursorPosition, Has<Interpolated>), Changed<CursorPosition>>,
        players: Query<
            (
                &PlayerId,
                &PlayerPosition,
                Has<Predicted>,
                Has<Interpolated>,
            ),
            Or<(Added<PlayerPosition>, Changed<PlayerPosition>)>,
        >,
    ) {
        let Some(settings) = settings else {
            return;
        };
        if !settings.log_client {
            return;
        }
        for (player_id, cursor, interpolated) in &cursors {
            info!(
                ?player_id,
                cursor = ?cursor.0,
                interpolated,
                "client_replication client cursor update"
            );
        }
        for (player_id, position, predicted, interpolated) in &players {
            info!(
                ?player_id,
                position = ?position.0,
                predicted,
                interpolated,
                "client_replication client player update"
            );
        }
    }

    fn parse_keys(value: Option<String>) -> Vec<KeyCode> {
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

    pub(super) fn log_entities(
        settings: Res<DebugSettings>,
        cursors: Query<
            (&PlayerId, &CursorPosition),
            Or<(Added<CursorPosition>, Changed<CursorPosition>)>,
        >,
        players: Query<
            (&PlayerId, &PlayerPosition),
            Or<(Added<PlayerPosition>, Changed<PlayerPosition>)>,
        >,
    ) {
        if !settings.log_server {
            return;
        }
        for (player_id, cursor) in &cursors {
            info!(
                ?player_id,
                cursor = ?cursor.0,
                "client_replication server cursor update"
            );
        }
        for (player_id, position) in &players {
            info!(
                ?player_id,
                position = ?position.0,
                "client_replication server player update"
            );
        }
    }
}
