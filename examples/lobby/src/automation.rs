use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::*;
use lightyear_examples_common::automation::{env_string, sync_pressed_keys, HeadlessInputPlugin};

#[cfg(feature = "client")]
use crate::client::AppState;
use crate::protocol::{Channel1, Inputs, JoinLobby, Lobbies, PlayerId, PlayerPosition, StartGame};

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
                client::auto_join_lobby,
                client::auto_start_game,
                client::mark_debug_lobbies,
                client::mark_debug_players,
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
            (server::mark_debug_lobbies, server::mark_debug_players),
        );
    }
}

#[cfg(feature = "client")]
mod client {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum AutoStartMode {
        Server,
        Host,
    }

    #[derive(Resource, Clone, Default)]
    pub(super) struct AutomationSettings {
        pressed_keys: Vec<KeyCode>,
        auto_start: Option<AutoStartMode>,
    }

    impl AutomationSettings {
        fn from_env() -> Self {
            let auto_start = env_string("LIGHTYEAR_AUTOSTART").and_then(|value| {
                match value.trim().to_ascii_lowercase().as_str() {
                    "server" => Some(AutoStartMode::Server),
                    "host" => Some(AutoStartMode::Host),
                    _ => None,
                }
            });
            Self {
                pressed_keys: parse_keys(env_string("LIGHTYEAR_AUTOMOVE")),
                auto_start,
            }
        }
    }

    pub(super) fn init_settings(mut commands: Commands) {
        commands.insert_resource(AutomationSettings::from_env());
    }

    pub(super) fn drive_keys(
        settings: Res<AutomationSettings>,
        app_state: Res<State<AppState>>,
        mut previous: Local<Vec<KeyCode>>,
        mut buttons: ResMut<ButtonInput<KeyCode>>,
    ) {
        let keys = if matches!(app_state.get(), AppState::Game) {
            settings.pressed_keys.clone()
        } else {
            Vec::new()
        };
        sync_pressed_keys(&mut buttons, &mut previous, &keys);
    }

    pub(super) fn auto_join_lobby(
        app_state: Res<State<AppState>>,
        mut next_state: ResMut<NextState<AppState>>,
        lobbies: Query<&Lobbies, Changed<Lobbies>>,
        mut join_sender: Single<&mut MessageSender<JoinLobby>>,
        mut joined: Local<bool>,
    ) {
        if *joined || !matches!(app_state.get(), AppState::Lobby { joined_lobby: None }) {
            return;
        }
        if lobbies.is_empty() {
            return;
        }
        join_sender.send::<Channel1>(JoinLobby { lobby_id: 0 });
        next_state.set(AppState::Lobby {
            joined_lobby: Some(0),
        });
        *joined = true;
    }

    pub(super) fn auto_start_game(
        settings: Res<AutomationSettings>,
        app_state: Res<State<AppState>>,
        lobbies: Single<&Lobbies>,
        mut start_sender: Single<(&LocalId, &mut MessageSender<StartGame>)>,
        mut sent: Local<bool>,
    ) {
        let Some(mode) = settings.auto_start else {
            return;
        };
        if *sent {
            return;
        }
        let AppState::Lobby {
            joined_lobby: Some(lobby_id),
        } = *app_state.get()
        else {
            return;
        };
        let lobbies = lobbies.into_inner();
        let Some(lobby) = lobbies.lobbies.get(lobby_id) else {
            return;
        };
        if lobby.players.len() < 2 {
            return;
        }
        let (local_id, mut sender) = start_sender.into_inner();
        let host = match mode {
            AutoStartMode::Server => None,
            AutoStartMode::Host => Some(local_id.0),
        };
        sender.send::<Channel1>(StartGame { lobby_id, host });
        *sent = true;
    }

    pub(super) fn mark_debug_lobbies(
        mut commands: Commands,
        lobbies: Query<Entity, Added<Lobbies>>,
    ) {
        for entity in &lobbies {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<Lobbies>([
                    DebugSamplePoint::Update,
                ]));
        }
    }

    pub(super) fn mark_debug_players(
        mut commands: Commands,
        players: Query<(Entity, Has<Predicted>, Has<Interpolated>), Added<PlayerId>>,
    ) {
        for (entity, predicted, interpolated) in &players {
            if predicted || interpolated {
                commands
                    .entity(entity)
                    .insert(LightyearDebug::component_at::<PlayerPosition>([
                        DebugSamplePoint::Update,
                    ]));
            }
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

    pub(super) fn mark_debug_lobbies(
        mut commands: Commands,
        lobbies: Query<Entity, Added<Lobbies>>,
    ) {
        for entity in &lobbies {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<Lobbies>([
                    DebugSamplePoint::Update,
                ]));
        }
    }

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
}
