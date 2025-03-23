//! This file contains the shared [`Protocol`] that defines the messages that can be sent between the client and server.
//!
//! You will need to define the [`Components`], [`Messages`] and [`Inputs`] that make up the protocol.
//! You can use the `#[protocol]` attribute to specify additional behaviour:
//! - how entities contained in the message should be mapped from the remote world to the local world
//! - how the component should be synchronized between the `Confirmed` entity and the `Predicted`/`Interpolated` entity
use core::ops::{Add, Mul};

use bevy::ecs::entity::MapEntities;
use bevy::prelude::{
    default, Bundle, Color, Component, Deref, DerefMut, Entity, EntityMapper, Transform, Vec2,
};
use bevy::prelude::{App, Plugin};
use lightyear::client::components::ComponentSyncMode;
use lightyear::prelude::*;
use lightyear::shared::replication::delta::Diffable;
use serde::{Deserialize, Serialize};
use tracing::{info, trace};

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    position: PlayerPosition,
    delta: DeltaCompression,
    color: PlayerColor,
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
            delta: DeltaCompression::default().add::<PlayerPosition>(),
            color: PlayerColor(color),
        }
    }
}

// Components

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(ClientId);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut)]
pub struct PlayerPosition(pub Vec2);

const MAX_POSITION_DELTA: f32 = 200.0;

// Since between two ticks the position doesn't change much, we could encode
// the diff using a discrete set of values
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

// Example of a component that contains an entity.
// This component, when replicated, needs to have the inner entity mapped from the Server world
// to the client World.
// You will need to derive the `MapEntities` trait for the component, and register
// app.add_map_entities<PlayerParent>() in your protocol
#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct PlayerParent(Entity);

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
#[derive(Clone)]
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
            // NOTE: remember to add delta compression in the protocol!
            .add_delta_compression()
            .add_prediction(ComponentSyncMode::Full)
            .add_interpolation(ComponentSyncMode::Full)
            .add_linear_interpolation_fn();

        app.register_component::<PlayerColor>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once)
            .add_interpolation(ComponentSyncMode::Once);
        // channels
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        });
    }
}
