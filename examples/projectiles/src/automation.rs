use avian2d::prelude::Position;
use bevy::prelude::*;
use bevy_enhanced_input::action::mock::ActionMock;
use bevy_enhanced_input::prelude::{Action, ActionValue};
use lightyear::prelude::*;
use lightyear_examples_common::automation::{
    HeadlessInputPlugin, env_flag, env_string, sync_pressed_keys,
};

use crate::protocol::{
    Bot, BulletMarker, ClientContext, GameReplicationMode, MoveCursor, PlayerId, PlayerMarker,
    ProjectileReplicationMode, Score, Shoot,
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
                client::update_aim,
                client::mark_debug_players,
                client::mark_debug_bullets,
                client::mark_debug_modes,
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
            (
                server::mark_debug_players,
                server::mark_debug_bullets,
                server::mark_debug_modes,
            ),
        );
    }
}

#[cfg(feature = "client")]
mod client {
    use super::*;

    #[derive(Resource, Clone, Default)]
    pub(super) struct AutomationSettings {
        pressed_keys: Vec<KeyCode>,
        auto_shoot: bool,
    }

    #[derive(Default)]
    pub(super) struct ShootPulse {
        timer: Option<Timer>,
        pressed: bool,
    }

    impl AutomationSettings {
        fn from_env() -> Self {
            Self {
                pressed_keys: parse_keys(env_string("LIGHTYEAR_AUTOMOVE")),
                auto_shoot: env_flag("LIGHTYEAR_AUTOSHOOT"),
            }
        }
    }

    pub(super) fn init_settings(mut commands: Commands) {
        commands.insert_resource(AutomationSettings::from_env());
    }

    pub(super) fn drive_keys(
        time: Res<Time>,
        settings: Res<AutomationSettings>,
        mut pulse: Local<ShootPulse>,
        mut previous: Local<Vec<KeyCode>>,
        mut buttons: ResMut<ButtonInput<KeyCode>>,
    ) {
        let mut keys = settings.pressed_keys.clone();
        if settings.auto_shoot {
            let timer = pulse
                .timer
                .get_or_insert_with(|| Timer::from_seconds(0.2, TimerMode::Repeating));
            if timer.tick(time.delta()).just_finished() {
                pulse.pressed = !pulse.pressed;
            }
            if pulse.pressed {
                keys.push(KeyCode::Space);
            }
        }
        sync_pressed_keys(&mut buttons, &mut previous, &keys);
    }

    pub(super) fn update_aim(
        bots: Query<&Position, With<Bot>>,
        mut action_query: Query<&mut ActionMock, With<Action<MoveCursor>>>,
    ) {
        let target = bots.iter().next().map(|position| position.0);
        let Some(target) = target else {
            return;
        };
        for mut action_mock in &mut action_query {
            action_mock.value = ActionValue::Axis2D(target);
        }
    }

    pub(super) fn mark_debug_players(
        mut commands: Commands,
        players: Query<Entity, (With<PlayerMarker>, Added<PlayerId>)>,
    ) {
        for entity in &players {
            commands.entity(entity).insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::Update])
                    .with_component_at::<Score>([DebugSamplePoint::Update]),
            );
        }
    }

    pub(super) fn mark_debug_bullets(
        mut commands: Commands,
        bullets: Query<Entity, Added<BulletMarker>>,
    ) {
        for entity in &bullets {
            commands.entity(entity).insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::Update])
                    .with_component_at::<BulletMarker>([DebugSamplePoint::Update]),
            );
        }
    }

    pub(super) fn mark_debug_modes(
        mut commands: Commands,
        modes: Query<Entity, Added<ClientContext>>,
    ) {
        for entity in &modes {
            commands.entity(entity).insert(
                LightyearDebug::component_at::<GameReplicationMode>([DebugSamplePoint::Update])
                    .with_component_at::<ProjectileReplicationMode>([DebugSamplePoint::Update]),
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

    pub(super) fn mark_debug_players(
        mut commands: Commands,
        players: Query<Entity, (With<PlayerMarker>, Added<PlayerId>)>,
    ) {
        for entity in &players {
            commands.entity(entity).insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::Update])
                    .with_component_at::<Score>([DebugSamplePoint::Update]),
            );
        }
    }

    pub(super) fn mark_debug_bullets(
        mut commands: Commands,
        bullets: Query<Entity, Added<BulletMarker>>,
    ) {
        for entity in &bullets {
            commands.entity(entity).insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::Update])
                    .with_component_at::<BulletMarker>([DebugSamplePoint::Update]),
            );
        }
    }

    pub(super) fn mark_debug_modes(
        mut commands: Commands,
        modes: Query<Entity, Added<ClientContext>>,
    ) {
        for entity in &modes {
            commands.entity(entity).insert(
                LightyearDebug::component_at::<GameReplicationMode>([DebugSamplePoint::Update])
                    .with_component_at::<ProjectileReplicationMode>([DebugSamplePoint::Update]),
            );
        }
    }
}
