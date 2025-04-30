use bevy::app::{App, Plugin};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;
use lightyear::input::native::plugin::InputPlugin;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(pub PeerId);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut)]
pub struct PlayerPosition(pub Vec2);

impl Ease for PlayerPosition {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| PlayerPosition(Vec2::lerp(start.0, end.0, t)))
    }
}

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct PlayerColor(pub(crate) Color);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut)]
pub struct CursorPosition(pub Vec2);

impl Ease for CursorPosition {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| CursorPosition(Vec2::lerp(start.0, end.0, t)))
    }
}

// Inputs
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct Direction {
    pub(crate) up: bool,
    pub(crate) down: bool,
    pub(crate) left: bool,
    pub(crate) right: bool,
}

impl Direction {
    pub(crate) fn is_none(&self) -> bool {
        !self.up && !self.down && !self.left && !self.right
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub enum Inputs {
    Direction(Direction),
    Delete,
}

// Inputs must all implement MapEntities
impl MapEntities for Inputs {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {}
}

// Protocol
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // inputs
        // Use new input plugin path
        app.add_plugins(InputPlugin::<Inputs>::default());
        // components
        // Use PredictionMode and InterpolationMode
        app.register_component::<PlayerId>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<PlayerPosition>()
            .add_prediction(PredictionMode::Full)
            .add_interpolation(InterpolationMode::Full)
            .add_linear_interpolation_fn();

        app.register_component::<PlayerColor>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<CursorPosition>()
            .add_prediction(PredictionMode::Full)
            .add_interpolation(InterpolationMode::Full)
            .add_linear_interpolation_fn();
    }
}
