//! This file contains the shared [`Protocol`] that defines the messages that can be sent between the client and server.
//!
//! You will need to define the [`Components`], [`Messages`] and [`Inputs`] that make up the protocol.
//! You can use the `#[protocol]` attribute to specify additional behaviour:
//! - how entities contained in the message should be mapped from the remote world to the local world
//! - how the component should be synchronized between the `Confirmed` entity and the `Predicted`/`Interpolated` entity
use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;
use bevy_replicon::prelude::Diffable as RepliconDiffable;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    position: PlayerPosition,
    trail: PlayerTrail,
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
            trail: PlayerTrail::new(position),
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

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerTrailDiff {
    pub(crate) point: PlayerPosition,
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerTrail(pub Vec<PlayerPosition>);

impl PlayerTrail {
    const MAX_POINTS: usize = 50;

    pub(crate) fn new(position: Vec2) -> Self {
        Self(vec![PlayerPosition(position)])
    }

    fn push(&mut self, point: PlayerPosition) {
        self.0.push(point);
        let excess = self.0.len().saturating_sub(Self::MAX_POINTS);
        if excess > 0 {
            self.0.drain(..excess);
        }
    }
}

impl RepliconDiffable for PlayerTrail {
    type Diff = PlayerTrailDiff;
    const HISTORY_LEN: usize = 128;

    fn apply_diff(&mut self, diff: &Self::Diff) -> bevy::ecs::error::Result<()> {
        self.push(diff.point.clone());
        Ok(())
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

impl Default for Inputs {
    fn default() -> Self {
        Self::Direction(Direction {
            up: false,
            down: false,
            left: false,
            right: false,
        })
    }
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
        app.add_plugins(lightyear::prelude::input::native::InputPlugin::<Inputs>::default());
        // components
        // Use PredictionMode and InterpolationMode
        app.component::<PlayerId>().replicate();

        app.component::<PlayerPosition>()
            .replicate()
            .predict()
            .add_linear_interpolation();

        app.component::<PlayerTrail>().replicate_diff();

        app.component::<PlayerColor>().replicate();
    }
}
