use std::ops::Mul;

use bevy::app::{App, Plugin};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{default, Component, Entity, EntityMapper, Reflect, Resource};
use cfg_if::cfg_if;
use derive_more::{Add, Mul};
use lightyear_macros::ChannelInternal;
use serde::{Deserialize, Serialize};

use crate::client::components::ComponentSyncMode;
use crate::prelude::*;

// Messages
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
pub struct Message1(pub String);

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
pub struct Message2(pub u32);

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Add, Mul, Reflect)]
pub struct Component1(pub f32);

impl Mul<f32> for &Component1 {
    type Output = Component1;
    fn mul(self, rhs: f32) -> Self::Output {
        Component1(self.0 * rhs)
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Add, Mul, Reflect)]
pub struct Component2(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Add, Mul, Reflect)]
pub struct Component3(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct Component4(pub Entity);

impl MapEntities for Component4 {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.0 = entity_mapper.map_entity(self.0);
    }
}

// Resources
#[derive(Resource, Serialize, Deserialize, Debug, PartialEq, Clone, Add, Reflect)]
pub struct Resource1(pub f32);

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
        app.add_message::<Message1>(ChannelDirection::Bidirectional);
        app.add_message::<Message2>(ChannelDirection::Bidirectional);
        // inputs
        app.add_plugins(InputPlugin::<MyInput>::default());
        // components
        app.register_component::<Component1>(ChannelDirection::ServerToClient);
        app.add_prediction::<Component1>(ComponentSyncMode::Full);
        app.add_linear_interpolation_fn::<Component1>();

        app.register_component::<Component2>(ChannelDirection::ServerToClient);
        app.add_prediction::<Component2>(ComponentSyncMode::Simple);

        app.register_component::<Component3>(ChannelDirection::ServerToClient);
        app.add_prediction::<Component3>(ComponentSyncMode::Once);

        app.register_component::<Component4>(ChannelDirection::ServerToClient);
        app.add_prediction::<Component4>(ComponentSyncMode::Simple);
        app.add_component_map_entities::<Component4>();

        app.register_resource::<Resource1>(ChannelDirection::ServerToClient);
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
