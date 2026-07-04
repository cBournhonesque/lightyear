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
            trail: PlayerTrail::new(position),
            color: PlayerColor(color),
        }
    }
}

// Components

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(PeerId);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut)]
pub struct TrailPoint(pub Vec2);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerTrailDiff {
    pub(crate) new_head: TrailPoint,
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerTrail(pub Vec<TrailPoint>);

impl PlayerTrail {
    const MAX_POINTS: usize = 50;

    pub(crate) fn new(position: Vec2) -> Self {
        Self(vec![TrailPoint(position)])
    }

    pub(crate) fn head(&self) -> Vec2 {
        self.0.first().map(|point| point.0).unwrap_or(Vec2::ZERO)
    }

    pub(crate) fn push_head(&mut self, point: TrailPoint) {
        self.0.insert(0, point);
        self.0.truncate(Self::MAX_POINTS);
    }
}

impl RepliconDiffable for PlayerTrail {
    type Diff = PlayerTrailDiff;
    const HISTORY_LEN: usize = 128;

    fn apply_diff(&mut self, diff: &Self::Diff) -> bevy::ecs::error::Result<()> {
        self.push_head(diff.new_head.clone());
        Ok(())
    }
}

fn interpolate_player_trail(start: PlayerTrail, end: PlayerTrail, t: f32) -> PlayerTrail {
    let t = t.clamp(0.0, 1.0);
    if t <= f32::EPSILON {
        return start;
    }
    if 1.0 - t <= f32::EPSILON {
        return end;
    }
    if start.0.is_empty() {
        return end;
    }
    if end.0.is_empty() {
        return start;
    }

    let start_tail = start.0.last().expect("start trail is non-empty");
    PlayerTrail(
        end.0
            .iter()
            .enumerate()
            .map(|(index, end_point)| {
                let start_point = start.0.get(index).unwrap_or(start_tail);
                TrailPoint(start_point.0.lerp(end_point.0, t))
            })
            .collect(),
    )
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

        app.component::<PlayerTrail>()
            .replicate_diff()
            .predict_diff()
            .add_interpolation_diff_with(interpolate_player_trail);

        app.component::<PlayerColor>().replicate();
    }
}
