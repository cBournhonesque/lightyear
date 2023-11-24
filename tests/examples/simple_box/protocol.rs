use bevy::prelude::{default, Bundle, Color, Component, Deref, DerefMut, Vec2};
use derive_more::{Add, Mul};
use lightyear_shared::prelude::*;
use lightyear_shared::replication::{NetworkTarget, Replicate};
use lightyear_shared::UserInput;
use serde::{Deserialize, Serialize};

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    position: PlayerPosition,
    color: PlayerColor,
    replicate: Replicate,
}

impl PlayerBundle {
    pub(crate) fn new(id: ClientId, position: Vec2, color: Color) -> Self {
        Self {
            id: PlayerId(id),
            position: PlayerPosition(position),
            color: PlayerColor(color),
            replicate: Replicate {
                prediction_target: NetworkTarget::None,
                // prediction_target: NetworkTarget::Only(id),
                // interpolation_target: NetworkTarget::AllExcept(id),
                ..default()
            },
        }
    }
}

// Components

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(ClientId);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut, Add, Mul)]
pub struct PlayerPosition(Vec2);

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct PlayerColor(pub(crate) Color);

#[component_protocol(protocol = "MyProtocol")]
pub enum Components {
    #[sync(once)]
    PlayerId(PlayerId),
    #[sync(simple)]
    PlayerPosition(PlayerPosition),
    #[sync(once)]
    PlayerColor(PlayerColor),
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

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct Direction {
    pub(crate) up: bool,
    pub(crate) down: bool,
    pub(crate) left: bool,
    pub(crate) right: bool,
}
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub enum Inputs {
    Direction(Direction),
    Delete,
    Spawn,
}

impl UserInput for Inputs {}

protocolize! {
    Self = MyProtocol,
    Message = Messages,
    Component = Components,
    Input = Inputs,
}
