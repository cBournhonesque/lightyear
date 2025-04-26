use alloc::collections::VecDeque;
use core::ops::{Add, Mul};

use bevy::app::{App, Plugin};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{
    default, Bundle, Color, Component, Deref, DerefMut, Entity, EntityMapper, Reflect, Vec2,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, trace};

use lightyear::client::components::ComponentSyncMode;
use lightyear::prelude::client::LerpFn;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear::shared::replication::components::ReplicationGroup;

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    position: PlayerPosition,
    color: PlayerColor,
    replicate: Replicate,
}

// Tail
#[derive(Bundle)]
pub(crate) struct TailBundle {
    parent: PlayerParent,
    points: TailPoints,
    length: TailLength,
    replicate: Replicate,
}

impl PlayerBundle {
    pub(crate) fn new(id: ClientId, position: Vec2) -> Self {
        // Generate pseudo random color from client id.
        let h = (((id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
        let s = 0.8;
        let l = 0.5;
        let color = Color::hsl(h, s, l);
        Self {
            id: PlayerId(id),
            position: PlayerPosition(position),
            color: PlayerColor(color),
            replicate: Replicate {
                sync: SyncTarget {
                    prediction: NetworkTarget::Single(id),
                    interpolation: NetworkTarget::AllExceptSingle(id),
                },
                controlled_by: ControlledBy {
                    target: NetworkTarget::Single(id),
                    ..default()
                },
                // the default is: the replication group id is a u64 value generated from the entity (`entity.to_bits()`)
                group: ReplicationGroup::default(),
                ..default()
            },
        }
    }
}

impl TailBundle {
    pub(crate) fn new(id: ClientId, parent: Entity, parent_position: Vec2, length: f32) -> Self {
        let default_direction = Direction::Up;
        let tail = default_direction.get_tail(parent_position, length);
        let mut points = VecDeque::new();
        points.push_front((tail, default_direction));
        Self {
            parent: PlayerParent(parent),
            points: TailPoints(points),
            length: TailLength(length),
            replicate: Replicate {
                sync: SyncTarget {
                    prediction: NetworkTarget::Single(id),
                    interpolation: NetworkTarget::AllExceptSingle(id),
                },
                controlled_by: ControlledBy {
                    target: NetworkTarget::Single(id),
                    ..default()
                },
                // replicate this entity within the same replication group as the parent
                group: ReplicationGroup::default().set_id(parent.to_bits()),
                ..default()
            },
        }
    }
}

// Components

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct PlayerId(ClientId);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut, Reflect)]
pub struct PlayerPosition(pub(crate) Vec2);

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

// Channels

#[derive(Channel)]
pub struct Channel1;

// Messages

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message1(pub usize);

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

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
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
        // messages
        app.register_message::<Message1>(ChannelDirection::Bidirectional);
        // inputs
        app.add_plugins(InputPlugin::<Inputs>::default());
        // components
        app.register_component::<PlayerId>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once)
            .add_interpolation(ComponentSyncMode::Once);

        app.register_component::<PlayerPosition>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Full)
            // NOTE: notice that we use custom interpolation here, this means that we don't run
            //  the interpolation function for this component, so we need to implement our own interpolation system
            //  (we do this because our interpolation system queries multiple components at once)
            .add_custom_interpolation(ComponentSyncMode::Full)
            // we still register an interpolation function which will be used for visual interpolation
            .add_linear_interpolation_fn();

        app.register_component::<PlayerColor>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once)
            .add_interpolation(ComponentSyncMode::Once);

        app.register_component::<TailPoints>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Full)
            // NOTE: notice that we use custom interpolation here, this means that we don't run
            //  the interpolation function for this component, so we need to implement our own interpolation system
            //  (we do this because our interpolation system queries multiple components at once)
            .add_custom_interpolation(ComponentSyncMode::Full);
        // we do not register an interpolation function because we will use a custom interpolation system

        app.register_component::<TailLength>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once)
            .add_interpolation(ComponentSyncMode::Once);

        app.register_component::<PlayerParent>(ChannelDirection::ServerToClient)
            .add_map_entities()
            .add_prediction(ComponentSyncMode::Once)
            .add_interpolation(ComponentSyncMode::Once);
        // channels
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        });
    }
}
