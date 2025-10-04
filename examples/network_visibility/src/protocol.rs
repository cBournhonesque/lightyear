use bevy::ecs::entity::MapEntities;
use bevy::math::Vec2;
use bevy::prelude::*;
use lightyear::input::native::plugin::InputPlugin;
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

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
// Marker component
pub struct CircleMarker;

// Inputs

#[derive(Serialize, Deserialize, Default, Debug, PartialEq, Eq, Clone, Reflect)]
pub struct Inputs {
    pub(crate) up: bool,
    pub(crate) down: bool,
    pub(crate) left: bool,
    pub(crate) right: bool,
}

impl Inputs {
    pub(crate) fn is_none(&self) -> bool {
        !self.up && !self.down && !self.left && !self.right
    }
}

impl MapEntities for Inputs {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {}
}

// Protocol
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // inputs
        app.add_plugins(InputPlugin::<Inputs>::default());
        // components
        app.register_component::<PlayerId>();

        app.register_component::<Position>()
            .add_prediction()
            .add_linear_interpolation();
        app.register_component::<PlayerColor>();
        app.register_component::<CircleMarker>();
    }
}
