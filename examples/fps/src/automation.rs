use avian2d::prelude::Position;
use bevy::prelude::*;
use leafwing_input_manager::plugin::InputManagerSystem;
use leafwing_input_manager::prelude::ActionState;
use lightyear::input::client::InputSystems;
use lightyear::prelude::*;
use lightyear_examples_common::automation::{
    env_flag, env_string, sync_pressed_keys, HeadlessInputPlugin,
};

use crate::protocol::{
    BulletMarker, InterpolatedBot, PlayerActions, PlayerId, PlayerMarker, PredictedBot, Score,
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
            FixedPreUpdate,
            client::update_aim
                .before(InputSystems::BufferClientInputs)
                .in_set(InputManagerSystem::ManualControl),
        );
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
            Update,
            (server::mark_debug_players, server::mark_debug_bullets),
        );
    }
}

#[cfg(feature = "client")]
mod client {
    use super::*;

    #[derive(Resource, Clone, Debug, Default)]
    pub(super) struct AutomationSettings {
        base_keys: Vec<KeyCode>,
        auto_shoot: bool,
        aim_target: AimTarget,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(super) enum AimTarget {
        #[default]
        Any,
        PredictedBot,
        InterpolatedBot,
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
                aim_target: parse_aim_target(env_string("LIGHTYEAR_AIM_TARGET")),
            }
        }
    }

    pub(super) fn init_settings(mut commands: Commands) {
        let settings = AutomationSettings::from_env();
        lightyear_debug_event!(
            DebugCategory::Input,
            DebugSamplePoint::Startup,
            "Startup",
            "fps_automation_settings",
            auto_shoot = settings.auto_shoot,
            aim_target = ?settings.aim_target,
            base_keys = ?settings.base_keys,
            "FPS automation settings"
        );
        commands.insert_resource(settings);
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

    pub(super) fn update_aim(
        settings: Res<AutomationSettings>,
        bots: Query<
            (Entity, &Transform, Has<PredictedBot>, Has<InterpolatedBot>),
            Or<(With<PredictedBot>, With<InterpolatedBot>)>,
        >,
        mut actions: Query<&mut ActionState<PlayerActions>, With<Predicted>>,
        mut previous_target: Local<Option<(Entity, AimTarget, Vec2)>>,
    ) {
        let target = bots.iter().find_map(
            |(entity, transform, is_predicted_bot, is_interpolated_bot)| {
                let matches_target = match settings.aim_target {
                    AimTarget::Any => true,
                    AimTarget::PredictedBot => is_predicted_bot,
                    AimTarget::InterpolatedBot => is_interpolated_bot,
                };
                matches_target.then_some((
                    entity,
                    transform.translation.truncate(),
                    is_predicted_bot,
                    is_interpolated_bot,
                ))
            },
        );
        let Some((target_entity, target, is_predicted_bot, is_interpolated_bot)) = target else {
            return;
        };
        let should_emit =
            previous_target
                .as_ref()
                .is_none_or(|(entity, aim_target, previous_position)| {
                    *entity != target_entity
                        || *aim_target != settings.aim_target
                        || previous_position.distance_squared(target) > 1.0
                });
        if should_emit {
            lightyear_debug_event!(
                DebugCategory::Input,
                DebugSamplePoint::Update,
                "Update",
                "fps_automation_aim_target",
                target = ?settings.aim_target,
                entity = ?target_entity,
                target_position = ?target,
                is_predicted_bot = is_predicted_bot,
                is_interpolated_bot = is_interpolated_bot,
                "FPS automation aim target"
            );
            *previous_target = Some((target_entity, settings.aim_target, target));
        }
        for mut action_state in &mut actions {
            action_state.set_axis_pair(&PlayerActions::MoveCursor, target);
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
                    .with_component_at::<PlayerId>([DebugSamplePoint::Update]),
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

    fn parse_aim_target(value: Option<String>) -> AimTarget {
        let Some(value) = value else {
            return AimTarget::Any;
        };
        match value.trim().to_ascii_lowercase().as_str() {
            "interpolated" | "interpolated_bot" | "lag_compensation" => AimTarget::InterpolatedBot,
            "predicted" | "predicted_bot" | "prediction" => AimTarget::PredictedBot,
            "" | "any" => AimTarget::Any,
            other => {
                warn!(token = other, "Ignoring unknown LIGHTYEAR_AIM_TARGET token");
                AimTarget::Any
            }
        }
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
                LightyearDebug::component_at::<Position>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<Score>([DebugSamplePoint::FixedUpdate]),
            );
        }
    }

    pub(super) fn mark_debug_bullets(
        mut commands: Commands,
        bullets: Query<Entity, Added<BulletMarker>>,
    ) {
        for entity in &bullets {
            commands.entity(entity).insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<PlayerId>([DebugSamplePoint::FixedUpdate]),
            );
        }
    }
}
