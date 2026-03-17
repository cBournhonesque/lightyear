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
        app.add_systems(
            Update,
            (client::debug_player_entities, client::log_position_updates),
        );
    }
}

#[cfg(feature = "server")]
pub struct AutomationServerPlugin;

#[cfg(feature = "server")]
impl Plugin for AutomationServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, server::init_debug_logging);
        app.add_systems(FixedUpdate, server::log_player_updates);
    }
}

pub(crate) mod client {
    use super::*;

    #[derive(Resource, Clone, Default)]
    pub(crate) struct AutomationSettings {
        direction: Option<Direction>,
        log_positions: bool,
    }

    impl AutomationSettings {
        #[cfg(not(target_family = "wasm"))]
        fn from_env() -> Self {
            let direction = std::env::var("LIGHTYEAR_SIMPLE_BOX_AUTOMOVE")
                .ok()
                .and_then(|value| parse_direction(&value));
            let log_positions = std::env::var("LIGHTYEAR_SIMPLE_BOX_LOG_POSITIONS")
                .map(|value| value != "0")
                .unwrap_or(false);
            Self {
                direction,
                log_positions,
            }
        }

        #[cfg(target_family = "wasm")]
        fn from_env() -> Self {
            Self::default()
        }
    }

    pub(crate) fn init_automation_settings(mut commands: Commands) {
        let settings = AutomationSettings::from_env();
        if let Some(direction) = &settings.direction {
            info!(?direction, "Using automated client input");
        }
        if settings.log_positions {
            info!("Logging predicted and interpolated player position updates");
        }
        commands.insert_resource(settings);
    }

    #[cfg(not(target_family = "wasm"))]
    fn parse_direction(value: &str) -> Option<Direction> {
        let mut direction = Direction::default();
        for token in value.split(',') {
            match token.trim().to_ascii_lowercase().as_str() {
                "" | "none" => {}
                "up" | "u" => direction.up = true,
                "down" | "d" => direction.down = true,
                "left" | "l" => direction.left = true,
                "right" | "r" => direction.right = true,
                other => {
                    warn!(token = other, "Ignoring unknown automated input token");
                }
            }
        }
        Some(direction)
    }

    pub(crate) fn direction_override(
        settings: Option<Res<AutomationSettings>>,
    ) -> Option<Direction> {
        settings.and_then(|settings| settings.direction.clone())
    }

    pub(crate) fn debug_player_entities(
        query: Query<
            (
                Entity,
                &PlayerId,
                Has<Predicted>,
                Has<Interpolated>,
                Has<Controlled>,
                Has<Replicated>,
            ),
            Added<PlayerId>,
        >,
    ) {
        for (entity, player_id, predicted, interpolated, controlled, replicated) in query.iter() {
            warn!(
                ?entity,
                ?player_id,
                predicted,
                interpolated,
                controlled,
                replicated,
                "Player entity status on client"
            );
        }
    }

    pub(crate) fn log_position_updates(
        settings: Option<Res<AutomationSettings>>,
        query: Query<
            (
                Entity,
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
        if !settings.log_positions {
            return;
        }
        for (entity, player_id, position, predicted, interpolated, controlled) in query.iter() {
            if predicted || interpolated {
                info!(
                    ?entity,
                    ?player_id,
                    position = ?position.0,
                    predicted,
                    interpolated,
                    controlled,
                    "Player position update on client"
                );
            }
        }
    }
}

pub(crate) mod server {
    use super::*;

    #[derive(Resource, Default)]
    pub(crate) struct DebugLogging {
        enabled: bool,
    }

    impl DebugLogging {
        #[cfg(not(target_family = "wasm"))]
        fn from_env() -> Self {
            let enabled = std::env::var("LIGHTYEAR_SIMPLE_BOX_LOG_SERVER")
                .map(|value| value != "0")
                .unwrap_or(false);
            Self { enabled }
        }

        #[cfg(target_family = "wasm")]
        fn from_env() -> Self {
            Self::default()
        }
    }

    pub(crate) fn init_debug_logging(mut commands: Commands) {
        let logging = DebugLogging::from_env();
        if logging.enabled {
            info!("Logging server-side player inputs and position updates");
        }
        commands.insert_resource(logging);
    }

    pub(crate) fn log_player_updates(
        logging: Res<DebugLogging>,
        query: Query<
            (
                Entity,
                &PlayerId,
                &PlayerPosition,
                &ActionState<Inputs>,
                Has<Predicted>,
            ),
            Or<(Changed<PlayerPosition>, Changed<ActionState<Inputs>>)>,
        >,
    ) {
        if !logging.enabled {
            return;
        }
        for (entity, player_id, position, inputs, predicted) in query.iter() {
            info!(
                ?entity,
                ?player_id,
                position = ?position.0,
                ?inputs,
                predicted,
                "Server player update"
            );
        }
    }
}
