use std::ops::Mul;

use bevy::app::{App, Plugin};
use bevy::prelude::{default, Bundle, Color, Component, Deref, DerefMut, Vec2};
use derive_more::{Add, Mul};
use serde::{Deserialize, Serialize};

use lightyear::client::components::ComponentSyncMode;
use lightyear::prelude::*;

use crate::shared::color_from_id;

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    position: PlayerPosition,
    color: PlayerColor,
    replicate: Replicate,
}

impl PlayerBundle {
    pub(crate) fn new(id: ClientId, position: Vec2) -> Self {
        let color = color_from_id(id);
        Self {
            id: PlayerId(id),
            position: PlayerPosition(position),
            color: PlayerColor(color),
            replicate: Replicate {
                // prediction_target: NetworkTarget::None,
                prediction_target: NetworkTarget::Only(vec![id]),
                interpolation_target: NetworkTarget::AllExcept(vec![id]),
                ..default()
            },
        }
    }
}

// Player
#[derive(Bundle)]
pub(crate) struct CursorBundle {
    id: PlayerId,
    position: CursorPosition,
    color: PlayerColor,
    replicate: Replicate,
}

impl CursorBundle {
    pub(crate) fn new(id: ClientId, position: Vec2, color: Color) -> Self {
        Self {
            id: PlayerId(id),
            position: CursorPosition(position),
            color: PlayerColor(color),
            replicate: Replicate {
                replication_target: NetworkTarget::All,
                interpolation_target: NetworkTarget::AllExcept(vec![id]),
                ..default()
            },
        }
    }
}

// Components

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(pub ClientId);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut, Add, Mul)]
pub struct PlayerPosition(Vec2);

impl Mul<f32> for &PlayerPosition {
    type Output = PlayerPosition;

    fn mul(self, rhs: f32) -> Self::Output {
        PlayerPosition(self.0 * rhs)
    }
}

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct PlayerColor(pub(crate) Color);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut, Add)]
pub struct CursorPosition(pub Vec2);

impl Mul<f32> for &CursorPosition {
    type Output = CursorPosition;

    fn mul(self, rhs: f32) -> Self::Output {
        CursorPosition(self.0 * rhs)
    }
}

// Channels

#[derive(Channel)]
pub struct Channel1;

// Messages

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message1(pub usize);

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
    Spawn,
    None,
}

// Protocol
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // messages
        app.add_message::<Message1>(ChannelDirection::Bidirectional);
        // inputs
        app.add_plugins(InputPlugin::<Inputs>::default());
        // components
        app.register_component::<PlayerId>(ChannelDirection::Bidirectional)
            .add_prediction(ComponentSyncMode::Once)
            .add_interpolation(ComponentSyncMode::Once);

        app.register_component::<PlayerPosition>(ChannelDirection::Bidirectional)
            .add_prediction(ComponentSyncMode::Full)
            .add_interpolation(ComponentSyncMode::Full)
            .add_linear_interpolation_fn();

        app.register_component::<PlayerColor>(ChannelDirection::Bidirectional)
            .add_prediction(ComponentSyncMode::Once)
            .add_interpolation(ComponentSyncMode::Once);

        app.register_component::<CursorPosition>(ChannelDirection::Bidirectional)
            .add_prediction(ComponentSyncMode::Full)
            .add_interpolation(ComponentSyncMode::Full)
            .add_linear_interpolation_fn();
        // channels
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        });
    }
}
