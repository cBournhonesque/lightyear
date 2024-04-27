use bevy::app::{App, Plugin};
use bevy::prelude::Component;
use bevy::utils::default;
use derive_more::{Add, Mul};
use lightyear::client::components::ComponentSyncMode;
use serde::{Deserialize, Serialize};
use std::ops::Mul;

use lightyear::prelude::*;

// Messages
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Message1(pub String);

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Message2(pub u32);

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Add)]
pub struct Component1(pub f32);

impl Mul<f32> for &Component1 {
    type Output = Component1;

    fn mul(self, rhs: f32) -> Self::Output {
        Component1(self.0 * rhs)
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Component2(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Component3(pub f32);

// Inputs

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct MyInput(pub i16);
impl UserAction for MyInput {}

// Channels
#[derive(Channel)]
pub struct Channel1;

#[derive(Channel)]
pub struct Channel2;

// Protocol
pub(crate) struct ProtocolPlugin;
impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // messages
        app.add_message::<Message1>(ChannelDirection::ClientToServer);
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
        // channels
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        });
        app.add_channel::<Channel2>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        });
    }
}
