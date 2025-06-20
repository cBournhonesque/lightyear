#[cfg(not(feature = "std"))]
use alloc::{
    string::{String, ToString},
    vec,
    vec::Vec,
};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;
use bevy_enhanced_input::prelude::{InputAction, InputContext};
use leafwing_input_manager::Actionlike;
use lightyear::prelude::input::*;
use lightyear::prelude::*;
use lightyear_connection::direction::NetworkDirection;
use serde::{Deserialize, Serialize};

// Messages
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
pub struct StringMessage(pub String);

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, MapEntities, Reflect)]
pub struct EntityMessage(#[entities] pub Entity);

// Triggers
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect, Event)]
pub struct StringTrigger(pub String);

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect, Event, MapEntities)]
pub struct EntityTrigger(#[entities] pub Entity);



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

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect, MapEntities)]
pub struct CompMap(#[entities] pub Entity);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompFull(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompSimple(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompOnce(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompCorr(pub f32);

impl Ease for CompCorr {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| {
            CompCorr(f32::lerp(start.0, end.0, t))
        })
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompNotNetworked(pub f32);

// Native Inputs
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Reflect)]
pub struct NativeInput(pub i16);

impl MapEntities for NativeInput {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {}
}


// Leafwing Inputs
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
pub enum LeafwingInput1 {
    Jump,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
pub enum LeafwingInput2 {
    Crouch,
}

// BEI Inputs
#[derive(InputContext)]
#[input_context(schedule = FixedPreUpdate)]
pub struct BEIContext;

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, InputAction)]
#[input_action(output = bool)]
pub struct BEIAction1;

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
        // triggers
        app.add_trigger::<StringTrigger>()
            .add_direction(NetworkDirection::Bidirectional);
        app.add_trigger::<EntityTrigger>()
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
        app.register_component::<CompFull>()
            .add_prediction(PredictionMode::Full)
            .add_interpolation(InterpolationMode::Full);
        app.register_component::<CompSimple>()
            .add_prediction(PredictionMode::Simple)
            .add_interpolation(InterpolationMode::Simple);
        app.register_component::<CompOnce>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);
        app.register_component::<CompCorr>()
            .add_prediction(PredictionMode::Full)
            .add_linear_correction_fn()
            .add_interpolation(InterpolationMode::Full);
        app.register_component::<CompMap>()
            .add_prediction(PredictionMode::Full)
            .add_map_entities();
        app.add_rollback::<CompNotNetworked>();
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
        app.add_plugins(native::InputPlugin::<NativeInput> {
            config: InputConfig::<NativeInput> {
                rebroadcast_inputs: true,
                ..default()
            },
        });
        app.add_plugins(leafwing::InputPlugin::<LeafwingInput1> {
            config: InputConfig::<LeafwingInput1> {
                rebroadcast_inputs: true,
                ..default()
            },
        });
        app.add_plugins(bei::InputPlugin::<BEIContext> {
            config: InputConfig::<BEIContext> {
                rebroadcast_inputs: true,
                ..default()
            },
        });
        app.register_input_action::<BEIAction1>();
    }
}
