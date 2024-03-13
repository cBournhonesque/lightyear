use std::collections::VecDeque;
use std::ops::Mul;

use bevy::prelude::{
    default, Bundle, Color, Component, Deref, DerefMut, Entity, EntityMapper, Reflect, Vec2,
};
use derive_more::{Add, Mul};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, trace};

use lightyear::prelude::client::LerpFn;
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
    pub(crate) fn new(id: ClientId, position: Vec2, color: Color) -> Self {
        Self {
            id: PlayerId(id),
            position: PlayerPosition(position),
            color: PlayerColor(color),
            replicate: Replicate {
                // prediction_target: NetworkTarget::None,
                prediction_target: NetworkTarget::Single(id),
                // interpolation_target: NetworkTarget::None,
                interpolation_target: NetworkTarget::AllExceptSingle(id),
                // the default is: the replication group id is a u64 value generated from the entity (`entity.to_bits()`)
                replication_group: ReplicationGroup::default(),
                ..default()
            },
        }
    }
}

impl TailBundle {
    pub(crate) fn new(id: ClientId, parent: Entity, parent_position: Vec2, length: f32) -> Self {
        let default_direction = Direction::default();
        let tail = default_direction.get_tail(parent_position, length);
        let mut points = VecDeque::new();
        points.push_front((tail, default_direction));
        Self {
            parent: PlayerParent(parent),
            points: TailPoints(points),
            length: TailLength(length),
            replicate: Replicate {
                // prediction_target: NetworkTarget::None,
                prediction_target: NetworkTarget::Single(id),
                // interpolation_target: NetworkTarget::None,
                interpolation_target: NetworkTarget::AllExceptSingle(id),
                // replicate this entity within the same replication group as the parent
                replication_group: ReplicationGroup::default().set_id(parent.to_bits()),
                ..default()
            },
        }
    }
}

// Components

#[derive(Component, Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(ClientId);

#[derive(
    Component, Message, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut, Add,
)]
pub struct PlayerPosition(pub(crate) Vec2);

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
        unreachable!("a and b should be on the same x or y")
    }
}

#[derive(Component, Message, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct PlayerColor(pub(crate) Color);

#[derive(Component, Message, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct TailLength(pub(crate) f32);

#[derive(Component, Message, Deserialize, Serialize, Clone, Debug, PartialEq)]
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
// This can be done by adding a `#[message(custom_map)]` attribute to the component, and then
// deriving the `MapEntities` trait for the component.
#[derive(Component, Message, Deserialize, Serialize, Clone, Debug, PartialEq)]
#[message(custom_map)]
pub struct PlayerParent(pub(crate) Entity);

impl LightyearMapEntities for PlayerParent {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.0 = entity_mapper.map_entity(self.0);
    }
}

#[component_protocol(protocol = "MyProtocol")]
pub enum Components {
    #[sync(once)]
    PlayerId(PlayerId),
    #[sync(full)]
    PlayerPosition(PlayerPosition),
    #[sync(once)]
    PlayerColor(PlayerColor),
    #[sync(once)]
    TailLength(TailLength),
    // we set the interpolation function to NoInterpolation because we are using our own custom interpolation logic
    // (by default it would use LinearInterpolator, which requires Add and Mul bounds on this component)
    #[sync(full, lerp = "NullInterpolator")]
    TailPoints(TailPoints),
    #[sync(once)]
    PlayerParent(PlayerParent),
}

// Channels

#[derive(Channel)]
pub struct Channel1;

// Messages

#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message1(pub usize);

#[message_protocol(protocol = "MyProtocol")]
pub enum Messages {
    Message1(Message1),
}

// Inputs

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Default)]
// To simplify, we only allow one direction at a time
pub enum Direction {
    #[default]
    Up,
    Down,
    Left,
    Right,
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
        return None;
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
    // NOTE: the server MUST be able to distinguish between an input saying "the user is not doing any actions" and
    // "we haven't received the input for this tick", which means that the client must send inputs every tick
    // even if the user is not doing anything.
    None,
}

impl UserAction for Inputs {}

// Protocol

protocolize! {
    Self = MyProtocol,
    Message = Messages,
    Component = Components,
    Input = Inputs,
}

pub(crate) fn protocol() -> MyProtocol {
    let mut protocol = MyProtocol::default();
    protocol.add_channel::<Channel1>(ChannelSettings {
        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
        ..default()
    });
    protocol
}
