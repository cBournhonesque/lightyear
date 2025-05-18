//! This file contains the shared [`Protocol`] that defines the messages that can be sent between the client and server.
//!
//! You will need to define the [`Components`], [`Messages`] and [`Inputs`] that make up the protocol.
//! You can use the `#[protocol]` attribute to specify additional behaviour:
//! - how entities contained in the message should be mapped from the remote world to the local world
//! - how the component should be synchronized between the `Confirmed` entity and the `Predicted`/`Interpolated` entity
use bevy::color::palettes;
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{
    default, Bundle, Color, Component, Deref, DerefMut, Entity, EntityMapper, Reflect, Vec2,
};
use bevy::prelude::{App, Plugin};
use core::ops::{Add, Mul};
use serde::{Deserialize, Serialize};

// Use preludes
use lightyear::prelude::client::*;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
// Removed AuthorityPeer import, it's part of prelude::server now
// use lightyear::prelude::server::AuthorityPeer;

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    position: Position,
    color: PlayerColor,
}

impl PlayerBundle {
    // Updated to use PeerId
    pub(crate) fn new(id: PeerId, position: Vec2) -> Self {
        // Color generation moved to shared.rs
        Self {
            id: PlayerId(id), // Store PeerId
            position: Position(position),
            // Color will be set separately or via a system using shared::color_from_id
            color: PlayerColor(Color::WHITE), // Default color, will be updated
        }
    }

    // Removed color_from_id from here, moved to shared.rs
    // pub(crate) fn color_from_id(id: ClientId) -> Color { ... }
}

// Components

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct PlayerId(pub PeerId); // Use PeerId

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut, Reflect)]
pub struct Position(pub Vec2);

impl Add for Position {
    type Output = Position;
    #[inline]
    fn add(self, rhs: Position) -> Position {
        Position(self.0.add(rhs.0))
    }
}

impl Mul<f32> for &Position {
    type Output = Position;

    fn mul(self, rhs: f32) -> Self::Output {
        Position(self.0 * rhs)
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut, Reflect)]
pub struct Speed(pub Vec2);

impl Add for Speed {
    type Output = Speed;
    #[inline]
    fn add(self, rhs: Speed) -> Speed {
        Speed(self.0.add(rhs.0))
    }
}

impl Mul<f32> for &Speed {
    type Output = Speed;

    fn mul(self, rhs: f32) -> Self::Output {
        Speed(self.0 * rhs)
    }
}

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq, Reflect)]
pub struct PlayerColor(pub(crate) Color);

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct BallMarker;

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
}

impl MapEntities for Inputs {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {}
}

// Protocol
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // inputs
        // Use new input plugin path
        app.add_plugins(input::InputPlugin::<Inputs>::default());
        // components
        // Use PredictionMode and InterpolationMode
        app.register_component::<PlayerId>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<BallMarker>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<Position>()
            .add_prediction(PredictionMode::Full)
            .add_interpolation(InterpolationMode::Full)
            .add_linear_interpolation_fn();

        app.register_component::<Speed>()
            .add_prediction(PredictionMode::Full)
            .add_interpolation(InterpolationMode::Full)
            .add_linear_interpolation_fn();

        app.register_component::<PlayerColor>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        // channels
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        });
    }
}
