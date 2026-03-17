use avian3d::prelude::Position;
use bevy::prelude::*;
use lightyear::prelude::*;
use lightyear_examples_common::automation::{
    env_flag, env_string, sync_pressed_keys, HeadlessInputPlugin,
};

use crate::protocol::{BlockMarker, CharacterMarker, ColorComponent, ProjectileMarker};

#[cfg(feature = "client")]
pub struct AutomationClientPlugin;

#[cfg(feature = "client")]
impl Plugin for AutomationClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(HeadlessInputPlugin);
        app.add_systems(Startup, client::init_settings);
        app.add_systems(First, client::drive_keys);
        app.add_systems(Update, client::log_entities);
    }
}

#[cfg(feature = "server")]
pub struct AutomationServerPlugin;

#[cfg(feature = "server")]
impl Plugin for AutomationServerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(server::DebugSettings::from_env());
        app.add_systems(FixedUpdate, server::log_entities);
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
                pressed_keys: parse_keys(env_string("LIGHTYEAR_AUTOMOVE")),
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

    pub(super) fn log_entities(
        settings: Option<Res<AutomationSettings>>,
        characters: Query<
            (&Position, Has<Predicted>, Has<Controlled>),
            (With<CharacterMarker>, Changed<Position>),
        >,
        blocks: Query<&Position, (With<BlockMarker>, Changed<Position>)>,
        projectiles: Query<&Position, (With<ProjectileMarker>, Changed<Position>)>,
    ) {
        let Some(settings) = settings else {
            return;
        };
        if !settings.log_client {
            return;
        }
        for (position, predicted, controlled) in &characters {
            info!(
                position = ?position.0,
                predicted,
                controlled,
                "avian_3d_character client character update"
            );
        }
        for position in &blocks {
            info!(position = ?position.0, "avian_3d_character client block update");
        }
        for position in &projectiles {
            info!(position = ?position.0, "avian_3d_character client projectile update");
        }
    }

    fn parse_keys(value: Option<String>) -> Vec<KeyCode> {
        let mut keys = Vec::new();
        let Some(value) = value else {
            return keys;
        };
        for token in value.split(',') {
            match token.trim().to_ascii_lowercase().as_str() {
                "up" | "u" | "forward" | "f" => keys.push(KeyCode::KeyW),
                "down" | "back" | "b" => keys.push(KeyCode::KeyS),
                "left" | "l" => keys.push(KeyCode::KeyA),
                "right" | "r" => keys.push(KeyCode::KeyD),
                "jump" => keys.push(KeyCode::Space),
                "shoot" => keys.push(KeyCode::KeyQ),
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
        characters: Query<&Position, (With<CharacterMarker>, Changed<Position>)>,
        blocks: Query<&Position, (With<BlockMarker>, Changed<Position>)>,
        projectiles: Query<&Position, (With<ProjectileMarker>, Changed<Position>)>,
    ) {
        if !settings.log_server {
            return;
        }
        for position in &characters {
            info!(position = ?position.0, "avian_3d_character server character update");
        }
        for position in &blocks {
            info!(position = ?position.0, "avian_3d_character server block update");
        }
        for position in &projectiles {
            info!(position = ?position.0, "avian_3d_character server projectile update");
        }
    }
}
