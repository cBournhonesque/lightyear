use bevy::diagnostic::DiagnosticsPlugin;
use bevy::log::{Level, LogPlugin};
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;

#[derive(Resource, Clone, Copy, Debug)]
pub struct ClientStartupConfig {
    pub client_id: u64,
    pub host_server: Option<Entity>,
}

#[derive(Resource, Clone, Copy, Debug)]
pub struct ServerStartupConfig {
    pub auto_spawn: bool,
}

impl Default for ServerStartupConfig {
    fn default() -> Self {
        Self { auto_spawn: true }
    }
}

pub fn headless_enabled() -> bool {
    std::env::var("LIGHTYEAR_HEADLESS")
        .map(|value| value != "0")
        .unwrap_or(false)
}

pub fn build_base_app() -> App {
    let mut app = App::new();
    if headless_enabled() {
        app.add_plugins((
            MinimalPlugins,
            LogPlugin {
                level: Level::INFO,
                ..default()
            },
            StatesPlugin,
            DiagnosticsPlugin,
        ));
    } else {
        app.add_plugins(DefaultPlugins);
    }
    app
}
