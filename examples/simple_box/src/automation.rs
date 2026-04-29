use crate::protocol::{Direction, Inputs, PlayerId, PlayerPosition};
use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::*;

#[cfg(feature = "client")]
pub struct AutomationClientPlugin;

#[cfg(feature = "client")]
impl Plugin for AutomationClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, client::init_automation_settings);
        app.add_systems(Update, client::mark_debug_player_entities);
    }
}

#[cfg(feature = "server")]
pub struct AutomationServerPlugin;

#[cfg(feature = "server")]
impl Plugin for AutomationServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, server::mark_debug_player_entities);
    }
}

pub(crate) mod client {
    use super::*;

    #[derive(Resource, Clone, Debug, Default)]
    pub(crate) struct AutomationSettings {
        mode: Option<AutomationMode>,
    }

    #[derive(Clone, Debug)]
    enum AutomationMode {
        Fixed(Direction),
        Random { seed: u64, interval_ticks: u32 },
    }

    impl AutomationSettings {
        #[cfg(not(target_family = "wasm"))]
        fn from_env() -> Self {
            let mode = std::env::var("LIGHTYEAR_SIMPLE_BOX_AUTOMOVE")
                .ok()
                .and_then(|value| parse_mode(&value));
            Self { mode }
        }

        #[cfg(target_family = "wasm")]
        fn from_env() -> Self {
            Self::default()
        }
    }

    pub(crate) fn init_automation_settings(mut commands: Commands) {
        let settings = AutomationSettings::from_env();
        if let Some(mode) = &settings.mode {
            info!(?mode, "Using automated client input");
        }
        commands.insert_resource(settings);
    }

    #[cfg(not(target_family = "wasm"))]
    fn parse_mode(value: &str) -> Option<AutomationMode> {
        let trimmed = value.trim();
        let lower = trimmed.to_ascii_lowercase();
        if trimmed.eq_ignore_ascii_case("random") || lower.starts_with("random:") {
            let seed = trimmed
                .split_once(':')
                .and_then(|(_, seed)| seed.parse::<u64>().ok())
                .or_else(|| parse_env_u64("LIGHTYEAR_SIMPLE_BOX_RANDOM_SEED"))
                .unwrap_or(1);
            return Some(AutomationMode::Random {
                seed,
                interval_ticks: parse_env_u32("LIGHTYEAR_SIMPLE_BOX_RANDOM_INTERVAL_TICKS")
                    .unwrap_or(12)
                    .max(1),
            });
        }
        parse_direction(trimmed).map(AutomationMode::Fixed)
    }

    #[cfg(not(target_family = "wasm"))]
    fn parse_env_u64(name: &str) -> Option<u64> {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
    }

    #[cfg(not(target_family = "wasm"))]
    fn parse_env_u32(name: &str) -> Option<u32> {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
    }

    #[cfg(not(target_family = "wasm"))]
    fn parse_direction(value: &str) -> Option<Direction> {
        let mut direction = Direction::default();
        let mut recognized = false;
        for token in value.split(',') {
            match token.trim().to_ascii_lowercase().as_str() {
                "" => {}
                "none" => recognized = true,
                "up" | "u" => {
                    recognized = true;
                    direction.up = true;
                }
                "down" | "d" => {
                    recognized = true;
                    direction.down = true;
                }
                "left" | "l" => {
                    recognized = true;
                    direction.left = true;
                }
                "right" | "r" => {
                    recognized = true;
                    direction.right = true;
                }
                other => {
                    warn!(token = other, "Ignoring unknown automated input token");
                }
            }
        }
        recognized.then_some(direction)
    }

    pub(crate) fn direction_override(
        settings: Option<Res<AutomationSettings>>,
        current_tick: Tick,
    ) -> Option<Direction> {
        let settings = settings?;
        match settings.mode.as_ref()? {
            AutomationMode::Fixed(direction) => Some(direction.clone()),
            AutomationMode::Random {
                seed,
                interval_ticks,
            } => Some(random_direction(current_tick.0 / *interval_ticks, *seed)),
        }
    }

    fn random_direction(bucket: u32, seed: u64) -> Direction {
        let value = splitmix64(seed ^ u64::from(bucket));
        match value % 9 {
            0 => Direction::default(),
            1 => Direction {
                up: true,
                ..default()
            },
            2 => Direction {
                down: true,
                ..default()
            },
            3 => Direction {
                left: true,
                ..default()
            },
            4 => Direction {
                right: true,
                ..default()
            },
            5 => Direction {
                up: true,
                right: true,
                ..default()
            },
            6 => Direction {
                down: true,
                right: true,
                ..default()
            },
            7 => Direction {
                up: true,
                left: true,
                ..default()
            },
            _ => Direction {
                down: true,
                left: true,
                ..default()
            },
        }
    }

    fn splitmix64(mut value: u64) -> u64 {
        value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    pub(crate) fn mark_debug_player_entities(
        mut commands: Commands,
        query: Query<(Entity, Has<Predicted>, Has<Interpolated>), Added<PlayerId>>,
    ) {
        for (entity, predicted, interpolated) in query.iter() {
            if predicted || interpolated {
                let input_sample_points = [
                    DebugSamplePoint::FixedPreUpdate,
                    DebugSamplePoint::FixedUpdate,
                ];
                commands.entity(entity).insert(
                    LightyearDebug::component_at::<PlayerPosition>([
                        DebugSamplePoint::Update,
                        DebugSamplePoint::PostUpdate,
                    ])
                    .with_component_at::<Predicted>([
                        DebugSamplePoint::Update,
                        DebugSamplePoint::PostUpdate,
                    ])
                    .with_component_at::<Interpolated>([
                        DebugSamplePoint::Update,
                        DebugSamplePoint::PostUpdate,
                    ])
                    .with_component_at::<ActionState<Inputs>>(input_sample_points),
                );
            }
        }
    }
}

pub(crate) mod server {
    use super::*;

    pub(crate) fn mark_debug_player_entities(
        mut commands: Commands,
        query: Query<Entity, Added<PlayerId>>,
    ) {
        for entity in query.iter() {
            let input_sample_points = [
                DebugSamplePoint::FixedPreUpdate,
                DebugSamplePoint::FixedUpdate,
            ];
            let debug =
                LightyearDebug::component_at::<PlayerPosition>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<ActionState<Inputs>>(input_sample_points);
            commands.entity(entity).insert(debug);
        }
    }
}
