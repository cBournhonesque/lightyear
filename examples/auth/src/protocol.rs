//! This file contains the shared [`Protocol`] that defines the messages that can be sent between the client and server.
//!
//! You will need to define the [`Components`], [`Messages`] and [`Inputs`] that make up the protocol.
//! You can use the `#[protocol]` attribute to specify additional behaviour:
//! - how entities contained in the message should be mapped from the remote world to the local world
//! - how the component should be synchronized between the `Confirmed` entity and the `Predicted`/`Interpolated` entity
use std::ops::Mul;

use bevy::ecs::entity::MapEntities;
use bevy::prelude::{
    default, Bundle, Color, Component, Deref, DerefMut, Entity, EntityMapper, Vec2,
};
use bevy::prelude::{App, Plugin};
use derive_more::Add;
use serde::{Deserialize, Serialize};

use lightyear::client::components::ComponentSyncMode;
use lightyear::prelude::*;

// Protocol
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // messages
        // inputs
        // components
        // channels
    }
}
