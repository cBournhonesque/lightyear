use avian2d::prelude::Position;
use bevy::prelude::*;
use bevy_enhanced_input::EnhancedInputSystems;
use bevy_enhanced_input::action::mock::ActionMock;
use bevy_enhanced_input::prelude::{Action, ActionValue};
use lightyear::prelude::*;
use lightyear_examples_common::automation::{
    HeadlessInputPlugin, env_flag, env_string, sync_pressed_keys,
};

use crate::protocol::{Bot, MoveCursor, PlayerId, PlayerMarker, Shoot};

#[cfg(feature = "server")]
use crate::{
    hit_detection::HitPolicy, representation::RepresentationKind, timeline::TimelinePolicy,
    trajectory::TrajectoryKind,
};

#[cfg(feature = "server")]
fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase().replace(['-', ' '], "_"))
}

#[cfg(feature = "server")]
pub(crate) fn initial_trajectory() -> TrajectoryKind {
    match env_value("LIGHTYEAR_INITIAL_TRAJECTORY").as_deref() {
        None | Some("hitscan" | "hit_scan" | "0") => TrajectoryKind::Hitscan,
        Some("linear" | "bullet" | "linear_projectile" | "1") => TrajectoryKind::Linear,
        Some(value) => {
            warn!(value, "Ignoring unknown LIGHTYEAR_INITIAL_TRAJECTORY");
            TrajectoryKind::default()
        }
    }
}

#[cfg(feature = "server")]
pub(crate) fn initial_representation() -> RepresentationKind {
    match env_value("LIGHTYEAR_INITIAL_REPRESENTATION").as_deref() {
        None | Some("state" | "state_entity" | "full" | "0") => RepresentationKind::StateEntity,
        Some("fire_data" | "fire_data_entity" | "direction" | "1") => {
            RepresentationKind::FireDataEntity
        }
        Some("shot_buffer" | "buffer" | "2") => RepresentationKind::ShotBuffer,
        Some(value) => {
            warn!(value, "Ignoring unknown LIGHTYEAR_INITIAL_REPRESENTATION");
            RepresentationKind::default()
        }
    }
}

#[cfg(feature = "server")]
pub(crate) fn initial_hit_policy() -> HitPolicy {
    match env_value("LIGHTYEAR_INITIAL_HIT_POLICY").as_deref() {
        None | Some("server_current" | "current" | "0") => HitPolicy::ServerCurrent,
        Some("server_rewound" | "rewound" | "lag_comp" | "1") => HitPolicy::ServerRewound,
        Some("client_reported" | "client" | "2") => HitPolicy::ClientReported,
        Some(value) => {
            warn!(value, "Ignoring unknown LIGHTYEAR_INITIAL_HIT_POLICY");
            HitPolicy::default()
        }
    }
}

#[cfg(feature = "server")]
pub(crate) fn initial_timeline() -> TimelinePolicy {
    match env_value("LIGHTYEAR_INITIAL_TIMELINE").as_deref() {
        None | Some("owner_predicted" | "default" | "0") => {
            TimelinePolicy::OwnerPredictedRemotesInterpolated
        }
        Some("all_predicted" | "predicted" | "1") => TimelinePolicy::AllPredicted,
        Some("all_interpolated" | "interpolated" | "2") => TimelinePolicy::AllInterpolated,
        Some(value) => {
            warn!(value, "Ignoring unknown LIGHTYEAR_INITIAL_TIMELINE");
            TimelinePolicy::default()
        }
    }
}

#[cfg(feature = "client")]
pub struct AutomationClientPlugin;

#[cfg(feature = "client")]
impl Plugin for AutomationClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(HeadlessInputPlugin);
        app.add_systems(Startup, client::init_settings);
        app.add_systems(
            FixedPreUpdate,
            client::drive_keys.before(EnhancedInputSystems::Update),
        );
        app.add_systems(
            Update,
            (
                client::update_aim,
                crate::debug::client::mark_debug_players,
                crate::debug::client::mark_debug_bullets,
                crate::debug::client::mark_debug_modes,
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
                crate::debug::server::mark_debug_players,
                crate::debug::server::mark_debug_bullets,
                crate::debug::server::mark_debug_modes,
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
        extra_keys: Vec<KeyCode>,
        auto_shoot: bool,
        auto_aim: bool,
    }

    #[derive(Default)]
    pub(super) struct ShootPulse {
        timer: Option<Timer>,
        pressed: bool,
    }

    impl AutomationSettings {
        fn from_env() -> Self {
            let auto_shoot = env_flag("LIGHTYEAR_AUTOSHOOT");
            Self {
                pressed_keys: parse_keys(env_string("LIGHTYEAR_AUTOMOVE")),
                extra_keys: parse_keys(env_string("LIGHTYEAR_AUTOKEYS")),
                auto_shoot,
                // Automated shooting keeps following a target for headless
                // smoke runs. An ordinary GUI client now uses only its mouse
                // and no longer silently tracks the bot.
                auto_aim: auto_shoot || env_flag("LIGHTYEAR_AUTOAIM"),
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
        keys.extend(settings.extra_keys.iter().copied());
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
        client: Query<&LocalId, With<Client>>,
        bots: Query<&Position, With<Bot>>,
        players: Query<(&PlayerId, &Position), With<PlayerMarker>>,
        mut action_query: Query<&mut ActionMock, With<Action<MoveCursor>>>,
    ) {
        if !settings.auto_aim {
            return;
        }
        let target = bots.iter().next().map(|position| position.0).or_else(|| {
            let Ok(client) = client.single() else {
                return None;
            };
            players
                .iter()
                .find(|(player_id, _)| player_id.0 != client.0)
                .map(|(_, position)| position.0)
        });
        let Some(target) = target else {
            return;
        };
        for mut action_mock in &mut action_query {
            action_mock.value = ActionValue::Axis2D(target);
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
                "q" | "keyq" => keys.push(KeyCode::KeyQ),
                "e" | "keye" => keys.push(KeyCode::KeyE),
                "keyr" => keys.push(KeyCode::KeyR),
                "t" | "keyt" => keys.push(KeyCode::KeyT),
                "space" | "shoot" => keys.push(KeyCode::Space),
                "" | "none" => {}
                other => warn!(token = other, "Ignoring unknown headless key token"),
            }
        }
        keys
    }
}
