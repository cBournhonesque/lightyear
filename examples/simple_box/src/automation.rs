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

    #[derive(Resource, Clone, Default)]
    pub(crate) struct AutomationSettings {
        direction: Option<Direction>,
    }

    impl AutomationSettings {
        #[cfg(not(target_family = "wasm"))]
        fn from_env() -> Self {
            let direction = std::env::var("LIGHTYEAR_SIMPLE_BOX_AUTOMOVE")
                .ok()
                .and_then(|value| parse_direction(&value));
            Self { direction }
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

    pub(crate) fn mark_debug_player_entities(
        mut commands: Commands,
        query: Query<(Entity, Has<Predicted>, Has<Interpolated>), Added<PlayerId>>,
    ) {
        for (entity, predicted, interpolated) in query.iter() {
            if predicted || interpolated {
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
                    ]),
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
