use bevy::prelude::*;
use lightyear::prelude::input::bei::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

// Components

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(pub PeerId);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut)]
pub struct Position(pub(crate) Vec2);

impl Ease for Position {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| {
            Position(Vec2::lerp(start.0, end.0, t))
        })
    }
}

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct PlayerColor(pub(crate) Color);

#[derive(Component, Deref, DerefMut)]
pub struct ShapeChangeTimer(pub(crate) Timer);

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub enum Shape {
    Circle,
    Triangle,
    Square,
}

#[derive(Component, Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
pub struct LowPriority;

#[derive(Component, Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
pub struct MediumPriority;

#[derive(Component, Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
pub struct HighPriority;

// Channels

pub struct Channel1;

// Messages

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message1(pub usize);

// Inputs
#[derive(Component, Serialize, Deserialize, Reflect, Clone, Debug, PartialEq)]
pub struct Player;

#[derive(Debug, InputAction)]
#[action_output(Vec2)]
pub struct Movement;

// Protocol
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // inputs
        app.add_plugins(InputPlugin::<Player>::default());
        app.register_input_action::<Movement>();

        // components
        app.component::<PlayerId>().replicate();

        app.component::<Position>()
            .replicate()
            .predict()
            .add_linear_interpolation();

        app.component::<PlayerColor>().replicate();

        app.component::<LowPriority>().replicate_once();
        app.component::<MediumPriority>().replicate_once();
        app.component::<HighPriority>().replicate_once();

        app.component::<Shape>().replicate();
    }
}
