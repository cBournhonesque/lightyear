//! This file contains the shared [`Protocol`] that defines the messages that can be sent between the client and server.
//!
//! You will need to define the [`Components`], [`Messages`] and [`Inputs`] that make up the protocol.
//! You can use the `#[protocol]` attribute to specify additional behaviour:
//! - how entities contained in the message should be mapped from the remote world to the local world
//! - how the component should be synchronized between the `Confirmed` entity and the `Predicted`/`Interpolated` entity
use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;
use lightyear::prelude::client::*;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::{info, trace};

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    position: PlayerPosition,
    color: PlayerColor,
}

impl PlayerBundle {
    pub(crate) fn new(id: PeerId, position: Vec2) -> Self {
        // Generate pseudo random color from client id.
        let h = (((id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
        let s = 0.8;
        let l = 0.5;
        let color = Color::hsl(h, s, l);
        Self {
            id: PlayerId(id),
            position: PlayerPosition(position),
            color: PlayerColor(color),
        }
    }
}

// Components

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(PeerId);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut)]
pub struct PlayerPosition(pub Vec2);

impl Ease for PlayerPosition {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| {
            PlayerPosition(Vec2::lerp(start.0, end.0, t))
        })
    }
}

const MAX_POSITION_DELTA: f32 = 200.0;

// Since between two ticks the position doesn't change much, we could encode
// the diff using a discrete set of values to reduce the bandwidth
impl Diffable for PlayerPosition {
    type Delta = (i8, i8);

    fn base_value() -> Self {
        Self(Vec2::new(0.0, 0.0))
    }

    fn diff(&self, new: &Self) -> Self::Delta {
        let mut diff = new.0 - self.0;

        // Clamp the diff to a discrete set of values
        // i.e i8::MIN = -10.0, i8::MAX = 10.0
        diff.x = diff.x.clamp(-MAX_POSITION_DELTA, MAX_POSITION_DELTA);
        diff.y = diff.y.clamp(-MAX_POSITION_DELTA, MAX_POSITION_DELTA);
        diff.x = diff.x / MAX_POSITION_DELTA * (i8::MAX as f32);
        diff.y = diff.y / MAX_POSITION_DELTA * (i8::MAX as f32);
        trace!(
            "Computing diff between {:?} and {:?}: {:?}",
            self,
            new,
            diff
        );

        // Convert to i8
        (diff.x as i8, diff.y as i8)
    }

    fn apply_diff(&mut self, delta: &Self::Delta) {
        trace!("Applying diff {:?} to {:?}", delta, self);
        let mut diff = Vec2::new(delta.0 as f32, delta.1 as f32);
        diff.x = diff.x / (i8::MAX as f32) * MAX_POSITION_DELTA;
        diff.y = diff.y / (i8::MAX as f32) * MAX_POSITION_DELTA;
        self.0 += diff;
    }
}

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct PlayerColor(pub(crate) Color);

// Inputs

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Reflect)]
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

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
pub enum Inputs {
    Direction(Direction),
}

impl MapEntities for Inputs {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {}
}

// Protocol
#[derive(Clone)]
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // inputs
        app.register_type::<Inputs>();
        app.add_plugins(lightyear::prelude::input::native::InputPlugin::<Inputs>::default());
        // components
        // Use PredictionMode and InterpolationMode
        app.register_component::<PlayerId>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<PlayerPosition>()
            // NOTE: remember to add delta compression in the protocol!
            .add_delta_compression()
            .add_prediction(PredictionMode::Full)
            .add_interpolation(InterpolationMode::Full)
            .add_linear_interpolation_fn();

        app.register_component::<PlayerColor>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);
    }
}
