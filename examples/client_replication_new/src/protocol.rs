use core::ops::{Add, Mul};

use bevy::app::{App, Plugin};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{default, Bundle, Color, Component, Deref, DerefMut, EntityMapper, Vec2};
use serde::{Deserialize, Serialize};

// Use preludes
use lightyear::prelude::client::*;
use lightyear::prelude::server::*;
use lightyear::prelude::*;

use crate::shared::color_from_id;

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    position: PlayerPosition,
    color: PlayerColor,
    // Removed replicate field, add Replicate component manually when spawning
    // replicate: Replicate,
}

impl PlayerBundle {
    // Updated to use PeerId
    pub(crate) fn new(id: PeerId, position: Vec2) -> Self {
        let color = color_from_id(id);
        Self {
            id: PlayerId(id), // Store PeerId
            position: PlayerPosition(position),
            color: PlayerColor(color),
            // replicate: Replicate::default(), // Removed
        }
    }
}

// Player
#[derive(Bundle)]
pub(crate) struct CursorBundle {
    id: PlayerId,
    position: CursorPosition,
    color: PlayerColor,
    // Removed replicate field, add Replicate component manually when spawning
    // replicate: Replicate,
}

impl CursorBundle {
    // Updated to use PeerId
    pub(crate) fn new(id: PeerId, position: Vec2, color: Color) -> Self {
        Self {
            id: PlayerId(id), // Store PeerId
            position: CursorPosition(position),
            color: PlayerColor(color),
            // replicate: Replicate::default(), // Removed
        }
    }
}

// Components

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(pub PeerId); // Use PeerId

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut)]
pub struct PlayerPosition(Vec2);

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

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct PlayerColor(pub(crate) Color);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut)]
pub struct CursorPosition(pub Vec2);

impl Add for CursorPosition {
    type Output = CursorPosition;
    #[inline]
    fn add(self, rhs: CursorPosition) -> CursorPosition {
        CursorPosition(self.0.add(rhs.0))
    }
}

impl Mul<f32> for &CursorPosition {
    type Output = CursorPosition;

    fn mul(self, rhs: f32) -> Self::Output {
        CursorPosition(self.0 * rhs)
    }
}

// Channels

#[derive(Channel)]
pub struct Channel1;

impl Channel for Channel1 {
    fn name(&self) -> &'static str {
        "Channel1"
    }
}

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
}

// Inputs must all implement MapEntities
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
        // Use new input plugin path
        app.add_plugins(input::InputPlugin::<Inputs>::default());
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
        // channels
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        });
    }
}
