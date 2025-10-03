use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;
use lightyear::input::bei::prelude::{InputAction, InputPlugin};
use lightyear::prelude::input::InputRegistryExt;
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

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut)]
pub struct CursorPosition(pub Vec2);

impl Ease for CursorPosition {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| {
            CursorPosition(Vec2::lerp(start.0, end.0, t))
        })
    }
}

// Inputs

#[derive(Component, Serialize, Deserialize, Reflect, Clone, Debug, PartialEq)]
pub struct Player;

#[derive(Debug, InputAction)]
#[action_output(Vec2)]
pub struct Movement;

#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct DespawnPlayer;

#[derive(Component, Serialize, Deserialize, Reflect, Clone, Debug, PartialEq)]
pub struct Admin;

#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct SpawnPlayer;

pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // inputs
        app.add_plugins(InputPlugin::<Player>::default());
        app.add_plugins(InputPlugin::<Admin>::default());
        app.register_input_action::<Movement>();
        app.register_input_action::<SpawnPlayer>();
        app.register_input_action::<DespawnPlayer>();

        // components
        app.register_component::<PlayerId>();

        app.register_component::<Name>();

        app.register_component::<PlayerPosition>()
            .add_prediction()
            .add_linear_interpolation();

        app.register_component::<PlayerColor>();

        app.register_component::<CursorPosition>()
            .add_prediction()
            .add_linear_interpolation();
    }
}
