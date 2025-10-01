use avian2d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::input::prelude::InputConfig;
use lightyear::prelude::input::leafwing;
use lightyear::prelude::Channel;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::shared::color_from_id;

pub const BULLET_SIZE: f32 = 3.0;
pub const PLAYER_SIZE: f32 = 40.0;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct PredictedBot;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct InterpolatedBot;

// Components
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct PlayerId(pub PeerId);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct PlayerMarker;

/// Number of bullet hits
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct Score(pub usize);

#[derive(Component, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct ColorComponent(pub(crate) Color);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BulletMarker;

// Inputs

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect)]
pub enum PlayerActions {
    Up,
    Down,
    Left,
    Right,
    Shoot,
    MoveCursor,
}

impl Actionlike for PlayerActions {
    // Record what kind of inputs make sense for each action.
    fn input_control_kind(&self) -> InputControlKind {
        match self {
            Self::MoveCursor => InputControlKind::DualAxis,
            _ => InputControlKind::Button,
        }
    }
}

// Protocol
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // inputs
        // Use new input plugin path and default config
        app.add_plugins(leafwing::InputPlugin::<PlayerActions> {
            config: InputConfig::<PlayerActions> {
                // enable lag compensation; the input messages sent to the server will include the
                // interpolation delay of that client
                lag_compensation: true,
                ..default()
            },
        });
        // components
        app.register_component::<Name>();
        app.register_component::<PlayerId>();
        app.register_component::<PlayerMarker>();

        app.register_component::<Position>()
            .add_prediction()
            .add_linear_interpolation()
            .add_linear_correction_fn();

        app.register_component::<Rotation>()
            .add_prediction()
            .add_linear_interpolation()
            .add_linear_correction_fn();

        app.register_component::<ColorComponent>();

        app.register_component::<Score>();

        app.register_component::<RigidBody>();

        app.register_component::<BulletMarker>();

        app.register_component::<PredictedBot>();

        app.register_component::<InterpolatedBot>();
    }
}
