use std::ops::{Add, Mul};

use bevy::app::{App, Plugin};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{default, Component, Entity, EntityMapper, Reflect, Resource};
use bevy::utils::HashSet;
use byteorder::{NetworkEndian, ReadBytesExt, WriteBytesExt};
use cfg_if::cfg_if;
use lightyear_macros::ChannelInternal;
use serde::{Deserialize, Serialize};

use crate::client::components::ComponentSyncMode;
use crate::prelude::*;
use crate::protocol::serialize::SerializeFns;
use crate::serialize::reader::Reader;
use crate::serialize::writer::Writer;
use crate::serialize::SerializationError;
use crate::shared::replication::delta::Diffable;

// Messages
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
pub struct Message1(pub String);

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
pub struct Message2(pub u32);

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct Component1(pub f32);

impl Mul<f32> for &Component1 {
    type Output = Component1;
    fn mul(self, rhs: f32) -> Self::Output {
        Component1(self.0 * rhs)
    }
}

impl Add<Component1> for Component1 {
    type Output = Self;

    fn add(self, rhs: Component1) -> Self::Output {
        Component1(self.0 + rhs.0)
    }
}

#[derive(Component, Clone, Debug, PartialEq, Reflect)]
pub struct Component2(pub f32);

pub(crate) fn serialize_component2(
    data: &Component2,
    writer: &mut Writer,
) -> Result<(), SerializationError> {
    writer.write_u32::<NetworkEndian>(data.0.to_bits())?;
    Ok(())
}

pub(crate) fn deserialize_component2(
    reader: &mut Reader,
) -> Result<Component2, SerializationError> {
    let data = f32::from_bits(reader.read_u32::<NetworkEndian>()?);
    Ok(Component2(data))
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct Component3(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct Component4(pub Entity);

impl MapEntities for Component4 {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.0 = entity_mapper.map_entity(self.0);
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct Component5(pub f32);

impl Mul<f32> for &Component5 {
    type Output = Component5;
    fn mul(self, rhs: f32) -> Self::Output {
        Component5(self.0 * rhs)
    }
}

impl Add<Self> for Component5 {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct Component6(pub Vec<usize>);

// NOTE: for the delta-compression to work, the components must have the same prefix, starting with [1]
impl Diffable for Component6 {
    // const IDEMPOTENT: bool = false;
    type Delta = Vec<usize>;

    fn base_value() -> Self {
        Self(vec![1])
    }

    fn diff(&self, other: &Self) -> Self::Delta {
        Vec::from_iter(other.0[self.0.len()..].iter().cloned())
    }

    fn apply_diff(&mut self, delta: &Self::Delta) {
        self.0.extend(delta);
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct Component7(pub HashSet<usize>);

// NOTE: for the delta-compression to work, the components must have the same prefix, starting with [1]
impl Diffable for Component7 {
    // const IDEMPOTENT: bool = true;
    // additions, removals
    type Delta = (HashSet<usize>, HashSet<usize>);

    fn base_value() -> Self {
        Self(HashSet::new())
    }

    fn diff(&self, other: &Self) -> Self::Delta {
        let added = other.0.difference(&self.0).cloned().collect();
        let removed = self.0.difference(&other.0).cloned().collect();
        (added, removed)
    }

    fn apply_diff(&mut self, delta: &Self::Delta) {
        let (added, removed) = delta;
        self.0.extend(added);
        self.0.retain(|x| !removed.contains(x));
    }
}

// Resources
#[derive(Resource, Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
pub struct Resource1(pub f32);

/// Resource where we provide our own serialization/deserialization functions
#[derive(Resource, Debug, PartialEq, Clone, Reflect)]
pub struct Resource2(pub f32);

pub(crate) fn serialize_resource2(
    data: &Resource2,
    writer: &mut Writer,
) -> Result<(), SerializationError> {
    writer.write_u32::<NetworkEndian>(data.0.to_bits())?;
    Ok(())
}

pub(crate) fn deserialize_resource2(reader: &mut Reader) -> Result<Resource2, SerializationError> {
    let data = f32::from_bits(reader.read_u32::<NetworkEndian>()?);
    Ok(Resource2(data))
}

// Inputs

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Reflect)]
pub struct MyInput(pub i16);

// Protocol
cfg_if! {
    if #[cfg(feature = "leafwing")] {
        use leafwing_input_manager::Actionlike;
        #[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
        pub enum LeafwingInput1 {
            Jump,
        }

        #[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
        pub enum LeafwingInput2 {
            Crouch,
        }
    }
}

// Channels
#[derive(ChannelInternal, Reflect)]
pub struct Channel1;

#[derive(ChannelInternal, Reflect)]
pub struct Channel2;

// Protocol

pub(crate) struct ProtocolPlugin;
impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // messages
        app.register_message::<Message1>(ChannelDirection::Bidirectional);
        app.register_message::<Message2>(ChannelDirection::Bidirectional);
        // inputs
        app.add_plugins(InputPlugin::<MyInput>::default());
        // components
        app.register_component::<Component1>(ChannelDirection::Bidirectional)
            .add_prediction(ComponentSyncMode::Full)
            .add_interpolation(ComponentSyncMode::Full)
            .add_linear_interpolation_fn();

        app.register_component_custom_serde::<Component2>(
            ChannelDirection::ServerToClient,
            SerializeFns {
                serialize: serialize_component2,
                deserialize: deserialize_component2,
            },
        )
        .add_prediction(ComponentSyncMode::Simple);

        app.register_component::<Component3>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once);

        app.register_component::<Component4>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Simple)
            .add_map_entities();

        app.register_component::<Component5>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Full)
            .add_interpolation(ComponentSyncMode::Full)
            .add_linear_interpolation_fn();

        app.register_component::<Component6>(ChannelDirection::ServerToClient)
            .add_delta_compression();

        app.register_component::<Component7>(ChannelDirection::ServerToClient)
            .add_delta_compression();

        // resources
        app.register_resource::<Resource1>(ChannelDirection::ServerToClient);
        app.register_resource_custom_serde::<Resource2>(
            ChannelDirection::Bidirectional,
            SerializeFns {
                serialize: serialize_resource2,
                deserialize: deserialize_resource2,
            },
        );
        // channels
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        });
        app.add_channel::<Channel2>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliableWithAcks,
            ..default()
        });
    }
}
