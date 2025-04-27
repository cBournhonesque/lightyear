use bevy::app::{App, Plugin};
use bevy::math::{Curve, FloatExt};
use bevy::prelude::{Component, Ease, FunctionCurve, Interval};
use bevy::utils::default;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

// Messages
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Message1(pub String);

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Message2(pub u32);

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Component1(pub f32);

impl Ease for Component1 {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::EVERYWHERE, move |t| Self(f32::lerp(start.0, end.0, t)))
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Component2(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Component3(pub f32);

// Inputs

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct MyInput(pub i16);

// Channels
pub struct Channel1;

pub struct Channel2;

// Protocol
pub(crate) struct ProtocolPlugin;
impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // messages
        app.add_message::<Message1>()
            .add_direction(NetworkDirection::Bidirectional);
        app.add_message::<Message2>()
            .add_direction(NetworkDirection::Bidirectional);
        // inputs
        // app.add_plugins(InputPlugin::<MyInput>::default());
        // components
        app.register_component::<Component1>()
            .add_prediction(PredictionMode::Full)
            .add_linear_interpolation_fn();
        app.register_component::<Component2>()
            .add_prediction(PredictionMode::Simple);
        app.register_component::<Component3>()
            .add_prediction(PredictionMode::Once);
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
