use core::ops::{Add, Mul};


#[cfg(not(feature = "std"))]
use alloc::{string::{String, ToString}, vec, vec::Vec};
use bevy::app::{App, Plugin};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;
use cfg_if::cfg_if;
use lightyear_connection::direction::{AppChannelDirectionExt, AppMessageDirectionExt, NetworkDirection};
use lightyear_macros::ChannelInternal;
use lightyear_messages::prelude::*;
use lightyear_new::prelude::input::native::*;
use lightyear_new::prelude::input::*;
use lightyear_replication::components::ComponentReplicationConfig;
use lightyear_replication::registry::registry::AppComponentExt;
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::writer::{WriteInteger, Writer};
use lightyear_serde::SerializationError;
use lightyear_transport::prelude::{AppChannelExt, ChannelMode, ChannelSettings};
use serde::{Deserialize, Serialize};

// Messages
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
pub struct StringMessage(pub String);

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, MapEntities, Reflect)]
pub struct EntityMessage(#[entities] pub Entity);

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
#[derive(Reflect)]
pub struct Channel1;

#[derive(Reflect)]
pub struct Channel2;

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompA(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompDisabled(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompReplicateOnce(pub f32);

// Inputs
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Reflect)]
pub struct NativeInput(pub i16);

impl MapEntities for NativeInput {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {}
}

// Protocol
pub(crate) struct ProtocolPlugin;
impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // messages
        app.add_message::<StringMessage>()
            .add_direction(NetworkDirection::Bidirectional);
        app.add_message::<EntityMessage>()
            .add_map_entities()
            .add_direction(NetworkDirection::Bidirectional);
        // channels
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        })
            .add_direction(NetworkDirection::Bidirectional);
        app.add_channel::<Channel2>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliableWithAcks,
            ..default()
        })
        .add_direction(NetworkDirection::Bidirectional);
        // components
        app.register_component::<CompA>();
        app.register_component::<CompDisabled>()
            .with_replication_config(ComponentReplicationConfig {
                disable: true,
                ..default()
            });
        app.register_component::<CompReplicateOnce>()
            .with_replication_config(ComponentReplicationConfig {
                replicate_once: true,
                ..default()
            });
        // inputs
        app.add_plugins(InputPlugin::<NativeInput> {
            config: InputConfig::<NativeInput> {
                rebroadcast_inputs: false,
                ..default()
            },
        });
    }

}
