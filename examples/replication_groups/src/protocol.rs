extern crate alloc;
use alloc::collections::VecDeque;
use bevy::app::{App, Plugin};
use bevy::ecs::entity::MapEntities;
use bevy::math::Curve;
use bevy::prelude::*;
use core::ops::{Add, Mul};
use lightyear::input::native::plugin::InputPlugin;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::trace;

// Components

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct PlayerId(pub PeerId);

#[derive(
    Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Deref, DerefMut, Reflect,
)]
pub struct PlayerPosition(pub(crate) Vec2);

impl Ease for PlayerPosition {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| {
            PlayerPosition(Vec2::lerp(start.0, end.0, t))
        })
    }
}

impl Add for PlayerPosition {
    type Output = PlayerPosition;
    #[inline]
    fn add(self, rhs: PlayerPosition) -> PlayerPosition {
        PlayerPosition(self.0.add(rhs.0))
    }
}

impl Mul<f32> for &PlayerPosition {
    type Output = PlayerPosition;

    fn mul(self, rhs: f32) -> Self::Output {
        PlayerPosition(self.0 * rhs)
    }
}

impl PlayerPosition {
    /// Checks if the position is between two other positions.
    /// (the positions must have the same x or y)
    /// Will return None if it's not in between, otherwise will return where it is between a and b
    pub(crate) fn is_between(&self, a: Vec2, b: Vec2) -> Option<f32> {
        if a.x == b.x {
            if self.x != a.x {
                return None;
            }
            if a.y < b.y {
                if a.y <= self.y && self.y <= b.y {
                    return Some((self.y - a.y) / (b.y - a.y));
                } else {
                    return None;
                }
            } else {
                if b.y <= self.y && self.y <= a.y {
                    return Some((a.y - self.y) / (a.y - b.y));
                } else {
                    return None;
                }
            }
        } else if a.y == b.y {
            if self.y != a.y {
                return None;
            }
            if a.x < b.x {
                if a.x <= self.x && self.x <= b.x {
                    return Some((self.x - a.x) / (b.x - a.x));
                } else {
                    return None;
                }
            } else {
                if b.x <= self.x && self.x <= a.x {
                    return Some((a.x - self.x) / (a.x - b.x));
                } else {
                    return None;
                }
            }
        }
        unreachable!("a ({}) and b ({}) should be on the same x or y", a, b)
    }
}

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq, Reflect)]
pub struct PlayerColor(pub(crate) Color);

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq, Reflect)]
pub struct TailLength(pub(crate) f32);

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq, Reflect)]
// tail inflection points, from front (point closest to the head) to back (tail end point)
pub struct TailPoints(pub(crate) VecDeque<(Vec2, Direction)>);

pub fn segment_length(from: Vec2, to: Vec2) -> f32 {
    (from - to).length()
}
impl TailPoints {
    /// Make sure that the tail is exactly `length` long
    pub(crate) fn shorten_back(&mut self, head: Vec2, length: f32) {
        // find the index of the first point to modify (all points after that needs to be discarded)

        // treat the first point separately
        let mut current_length = segment_length(head, self.0.front().unwrap().0);
        if current_length >= length {
            trace!("shortening first segment");
            let direction = self.0.front().unwrap().1;
            let new_point = direction.get_tail(head, length);
            self.0 = VecDeque::new();
            self.0.push_front((new_point, direction));
            return;
        }
        for i in 1..self.0.len() {
            let segment_length = segment_length(self.0[i - 1].0, self.0[i].0);
            current_length += segment_length;
            if current_length > length {
                trace!("shortening tail");
                let direction = self.0[i].1;
                let new_segment_length = segment_length - (current_length - length);

                // shorten the segment, and drop the rest
                if new_segment_length > 0.0 {
                    let new_point = direction
                        .get_tail(self.0[i - 1].0, segment_length - (current_length - length));
                    // drop all elements from [i, ..[
                    let _ = self.0.split_off(i);
                    self.0.push_back((new_point, direction));
                } else {
                    // drop all elements from [i, ..[
                    let _ = self.0.split_off(i);
                }
                trace!("new tail: {:?}", self.0);
                return;
            }
        }
    }
}

// Example of a component that contains an entity.
// This component, when replicated, needs to have the inner entity mapped from the Server world
// to the client World.
// This can be done by calling `app.add_component_map_entities::<PlayerParent>()` in your protocol,
// and deriving the `MapEntities` trait for the component.
#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq, Reflect)]
pub struct PlayerParent(pub(crate) Entity);

impl MapEntities for PlayerParent {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.0 = entity_mapper.get_mapped(self.0);
    }
}

// Inputs

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Reflect)]
// To simplify, we only allow one direction at a time
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl MapEntities for Direction {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {}
}

impl Direction {
    // Get the direction from `from` to `to` (doesn't handle diagonals)
    pub fn from_points(from: Vec2, to: Vec2) -> Option<Self> {
        if from.x != to.x && from.y != to.y {
            trace!(?from, ?to, "diagonal");
            return None;
        }
        if from.y < to.y {
            return Some(Self::Up);
        }
        if from.y > to.y {
            return Some(Self::Down);
        }
        if from.x > to.x {
            return Some(Self::Left);
        }
        if from.x < to.x {
            return Some(Self::Right);
        }
        None
    }

    // Get the position of the point that would become `head` if we applied `length` * `self`
    pub fn get_tail(&self, head: Vec2, length: f32) -> Vec2 {
        match self {
            Direction::Up => Vec2::new(head.x, head.y - length),
            Direction::Down => Vec2::new(head.x, head.y + length),
            Direction::Left => Vec2::new(head.x + length, head.y),
            Direction::Right => Vec2::new(head.x - length, head.y),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
pub enum Inputs {
    Direction(Direction),
    Delete,
    Spawn,
}

impl MapEntities for Inputs {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {}
}

// Protocol
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<(Inputs, PlayerId, PlayerPosition, PlayerColor)>();

        // inputs
        app.add_plugins(InputPlugin::<Inputs>::default());
        // components
        app.register_component::<Name>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);
        app.register_component::<PlayerId>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<PlayerPosition>()
            .add_prediction(PredictionMode::Full)
            // NOTE: notice that we use custom interpolation here, this means that we don't run
            //  the interpolation function for this component, so we need to implement our own interpolation system
            //  (we do this because our interpolation system queries multiple components at once)
            .add_custom_interpolation(InterpolationMode::Full)
            // we still register an interpolation function which will be used for visual interpolation
            .add_linear_interpolation_fn();

        app.register_component::<PlayerColor>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<TailPoints>()
            .add_prediction(PredictionMode::Full)
            // NOTE: notice that we use custom interpolation here, this means that we don't run
            //  the interpolation function for this component, so we need to implement our own interpolation system
            //  (we do this because our interpolation system queries multiple components at once)
            .add_custom_interpolation(InterpolationMode::Full);
        // we do not register an interpolation function because we will use a custom interpolation system

        app.register_component::<TailLength>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<PlayerParent>()
            .add_map_entities()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);
    }
}
