use core::ops::{Add, Mul};

#[cfg(not(feature = "std"))]
use alloc::{string::{String, ToString}, vec, vec::Vec};
use bevy::app::{App, Plugin};
use bevy::ecs::entity::MapEntities;
use bevy::platform::collections::HashSet;
use bevy::prelude::{default, Component, Entity, EntityMapper, Event, Reflect, Resource};
use cfg_if::cfg_if;
use lightyear_macros::ChannelInternal;
use serde::{Deserialize, Serialize};

use crate::client::components::ComponentSyncMode;
use crate::prelude::*;
use crate::protocol::message::registry::AppMessageExt;
use crate::protocol::message::resource::AppResourceExt;
use crate::protocol::message::trigger::AppTriggerExt;
use crate::protocol::serialize::SerializeFns;
use crate::serialize::reader::{ReadInteger, Reader};
use crate::serialize::writer::{WriteInteger, Writer};
use crate::serialize::SerializationError;
use crate::shared::input::InputConfig;
use crate::shared::replication::delta::Diffable;

// Event
#[derive(Event, Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
pub struct IntegerEvent(pub u32);

// Messages
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
pub struct StringMessage(pub String);

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
pub struct EntityMessage(pub Entity);

impl MapEntities for EntityMessage {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.0 = entity_mapper.get_mapped(self.0);
    }
}

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct ComponentSyncModeFull(pub f32);

impl Mul<f32> for &ComponentSyncModeFull {
    type Output = ComponentSyncModeFull;
    fn mul(self, rhs: f32) -> Self::Output {
        ComponentSyncModeFull(self.0 * rhs)
    }
}

impl Add<ComponentSyncModeFull> for ComponentSyncModeFull {
    type Output = Self;

    fn add(self, rhs: ComponentSyncModeFull) -> Self::Output {
        ComponentSyncModeFull(self.0 + rhs.0)
    }
}

#[derive(Component, Clone, Debug, PartialEq, Reflect)]
pub struct ComponentSyncModeSimple(pub f32);

pub(crate) fn serialize_component2(
    data: &ComponentSyncModeSimple,
    writer: &mut Writer,
) -> Result<(), SerializationError> {
    writer.write_u32(data.0.to_bits())?;
    Ok(())
}

pub(crate) fn deserialize_component2(
    reader: &mut Reader,
) -> Result<ComponentSyncModeSimple, SerializationError> {
    let data = f32::from_bits(reader.read_u32()?);
    Ok(ComponentSyncModeSimple(data))
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct ComponentSyncModeOnce(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct ComponentMapEntities(pub Entity);

impl MapEntities for ComponentMapEntities {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.0 = entity_mapper.get_mapped(self.0);
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct ComponentCorrection(pub f32);

impl Mul<f32> for &ComponentCorrection {
    type Output = ComponentCorrection;
    fn mul(self, rhs: f32) -> Self::Output {
        ComponentCorrection(self.0 * rhs)
    }
}

impl Add<Self> for ComponentCorrection {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct ComponentDeltaCompression(pub Vec<usize>);

// NOTE: for the delta-compression to work, the components must have the same prefix, starting with [1]
impl Diffable for ComponentDeltaCompression {
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
pub struct ComponentDeltaCompression2(pub HashSet<usize>);

impl Diffable for ComponentDeltaCompression2 {
    // const IDEMPOTENT: bool = true;
    // additions, removals
    type Delta = (HashSet<usize>, HashSet<usize>);

    fn base_value() -> Self {
        Self(HashSet::default())
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

#[derive(Component, Clone, Debug, PartialEq, Reflect)]
pub struct ComponentRollback(pub f32);

#[derive(Component, Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct ComponentClientToServer(pub f32);

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
    writer.write_u32(data.0.to_bits())?;
    Ok(())
}

pub(crate) fn deserialize_resource2(reader: &mut Reader) -> Result<Resource2, SerializationError> {
    let data = f32::from_bits(reader.read_u32()?);
    Ok(Resource2(data))
}

// Inputs

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Reflect)]
pub struct MyInput(pub i16);

impl MapEntities for MyInput {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {}
}

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
        // events
        app.register_trigger::<IntegerEvent>(ChannelDirection::Bidirectional);
        // messages
        app.register_message::<StringMessage>(ChannelDirection::Bidirectional);
        app.register_message::<EntityMessage>(ChannelDirection::Bidirectional)
            .add_map_entities();
        // inputs
        app.add_plugins(InputPlugin::<MyInput> {
            config: InputConfig::<MyInput> {
                rebroadcast_inputs: true,
                ..default()
            },
        });
        // components
        app.register_component::<ComponentSyncModeFull>(ChannelDirection::Bidirectional)
            .add_prediction(ComponentSyncMode::Full)
            .add_interpolation(ComponentSyncMode::Full)
            .add_linear_interpolation_fn();

        app.register_component_custom_serde::<ComponentSyncModeSimple>(
            ChannelDirection::ServerToClient,
            SerializeFns {
                serialize: serialize_component2,
                deserialize: deserialize_component2,
            },
        )
        .add_prediction(ComponentSyncMode::Simple);

        app.register_component::<ComponentSyncModeOnce>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once);

        app.register_component::<ComponentMapEntities>(ChannelDirection::Bidirectional)
            .add_prediction(ComponentSyncMode::Simple)
            .add_map_entities();

        app.register_component::<ComponentCorrection>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Full)
            .add_linear_correction_fn()
            .add_interpolation(ComponentSyncMode::Full)
            .add_linear_interpolation_fn();

        app.register_component::<ComponentDeltaCompression>(ChannelDirection::ServerToClient)
            .add_delta_compression();

        app.register_component::<ComponentDeltaCompression2>(ChannelDirection::ServerToClient)
            .add_delta_compression();

        app.add_rollback::<ComponentRollback>();

        app.register_component::<ComponentClientToServer>(ChannelDirection::ClientToServer);

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
