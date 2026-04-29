use avian2d::prelude::{LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::prelude::*;
use lightyear_examples_common::automation::{
    HeadlessInputPlugin, env_flag, env_string, sync_pressed_keys,
};

use crate::protocol::{BulletMarker, ColorComponent, Player, PlayerActions, Score, Weapon};

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
            (client::mark_debug_players, client::mark_debug_bullets),
        );
    }
}

#[cfg(feature = "server")]
pub struct AutomationServerPlugin;

#[cfg(feature = "server")]
impl Plugin for AutomationServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            (server::mark_debug_players, server::mark_debug_bullets),
        );
    }
}

#[cfg(feature = "client")]
mod client {
    use super::*;

    #[derive(Resource, Clone, Default)]
    pub(super) struct AutomationSettings {
        base_keys: Vec<KeyCode>,
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
                base_keys: parse_keys(env_string("LIGHTYEAR_AUTOMOVE")),
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
        let mut keys = settings.base_keys.clone();
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

    pub(super) fn mark_debug_players(
        mut commands: Commands,
        players: Query<Entity, Added<Player>>,
    ) {
        for entity in &players {
            commands.entity(entity).try_insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::Update])
                    .with_component_at::<Rotation>([DebugSamplePoint::Update])
                    .with_component_at::<LinearVelocity>([DebugSamplePoint::Update])
                    .with_component_at::<ColorComponent>([DebugSamplePoint::Update])
                    .with_component_at::<Player>([DebugSamplePoint::Update])
                    .with_component_at::<Score>([DebugSamplePoint::Update])
                    .with_component_at::<Weapon>([DebugSamplePoint::Update])
                    .with_component_at::<ActionState<PlayerActions>>([DebugSamplePoint::Update]),
            );
        }
    }

    pub(super) fn mark_debug_bullets(
        mut commands: Commands,
        bullets: Query<Entity, Added<BulletMarker>>,
    ) {
        for entity in &bullets {
            commands.entity(entity).try_insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::Update])
                    .with_component_at::<BulletMarker>([DebugSamplePoint::Update]),
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
        players: Query<Entity, Added<Player>>,
    ) {
        for entity in &players {
            commands.entity(entity).try_insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<Rotation>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<LinearVelocity>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<ColorComponent>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<Player>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<Score>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<Weapon>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<ActionState<PlayerActions>>([
                        DebugSamplePoint::FixedUpdate,
                    ]),
            );
        }
    }

    pub(super) fn mark_debug_bullets(
        mut commands: Commands,
        bullets: Query<Entity, Added<BulletMarker>>,
    ) {
        for entity in &bullets {
            commands.entity(entity).try_insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<BulletMarker>([DebugSamplePoint::FixedUpdate]),
            );
        }
    }
}
