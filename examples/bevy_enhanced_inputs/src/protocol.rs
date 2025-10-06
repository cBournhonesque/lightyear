use bevy::math::Curve;
use bevy::prelude::*;
use bevy::prelude::{App, Plugin};
use lightyear::input::prelude::InputConfig;
use lightyear::prelude::input::bei::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(pub PeerId);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut)]
pub struct PlayerPosition(pub Vec2);

impl Ease for PlayerPosition {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| {
            PlayerPosition(Vec2::lerp(start.0, end.0, t))
        })
    }
}

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct PlayerColor(pub(crate) Color);

// Inputs

// the context will be replicated
#[derive(Component, Serialize, Deserialize, Reflect, Clone, Debug, PartialEq)]
pub struct Player;

#[derive(Debug, InputAction)]
#[action_output(Vec2)]
pub struct Movement;

// Protocol
#[derive(Clone)]
pub struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // inputs
        app.add_plugins(InputPlugin::<Player> {
            config: InputConfig::<Player> {
                rebroadcast_inputs: true,
                ..default()
            },
        });
        app.register_input_action::<Movement>();

        // components
        app.register_component::<PlayerId>();

        app.register_component::<PlayerPosition>()
            .add_prediction()
            .add_linear_interpolation();

        app.register_component::<PlayerColor>();
    }
}
