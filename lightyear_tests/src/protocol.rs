#[cfg(not(feature = "std"))]
use alloc::string::String;
use avian2d::prelude::*;
use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;
use bevy_enhanced_input::prelude::InputAction;
use leafwing_input_manager::Actionlike;
use lightyear::avian2d::plugin::AvianReplicationMode;
use lightyear::frame_interpolation::FrameInterpolationPlugin;
use lightyear::prelude::input::*;
use lightyear::prelude::*;
use lightyear_connection::direction::NetworkDirection;
use lightyear_replication::delta::Diffable;
use serde::{Deserialize, Serialize};

// Messages
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
pub struct StringMessage(pub String);

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, MapEntities, Reflect)]
pub struct EntityMessage(#[entities] pub Entity);

// Triggers
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect, Event)]
pub struct StringTrigger(pub String);

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect, EntityEvent, MapEntities)]
pub struct EntityTrigger(
    #[event_target]
    #[entities]
    pub Entity,
);

// Channels
#[derive(Reflect)]
pub struct Channel1;

#[derive(Reflect)]
pub struct Channel2;

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompA(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompS(pub String);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompDisabled(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompReplicateOnce(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect, MapEntities)]
pub struct CompMap(#[entities] pub Entity);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompFull(pub f32);

impl Ease for CompFull {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| {
            CompFull(f32::lerp(start.0, end.0, t))
        })
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompSimple(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompOnce(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Default, Reflect)]
pub struct CompCorr(pub f32);

impl Ease for CompCorr {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| {
            CompCorr(f32::lerp(start.0, end.0, t))
        })
    }
}

impl Diffable for CompCorr {
    fn base_value() -> Self {
        Self(0.0)
    }

    fn diff(&self, other: &Self) -> Self {
        Self(other.0 - self.0)
    }

    fn apply_diff(&mut self, delta: &Self) {
        self.0 += delta.0;
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompNotNetworked(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompDelta(pub usize);
impl Diffable<usize> for CompDelta {
    fn base_value() -> Self {
        Self(0)
    }

    fn diff(&self, other: &Self) -> usize {
        other.0 - self.0
    }

    fn apply_diff(&mut self, delta: &usize) {
        self.0 += *delta;
    }
}

// Native Inputs
#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Eq, Clone, Copy, Reflect)]
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
#[derive(Serialize, Deserialize, Component, Clone, PartialEq, Debug, Reflect)]
pub struct BEIContext;

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, InputAction)]
#[action_output(bool)]
pub struct BEIAction1;

// Protocol
pub struct ProtocolPlugin {
    pub avian_mode: AvianReplicationMode,
}
impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // messages
        app.register_message::<StringMessage>()
            .add_direction(NetworkDirection::Bidirectional);
        app.register_message::<EntityMessage>()
            .add_map_entities()
            .add_direction(NetworkDirection::Bidirectional);
        // triggers
        app.register_event::<StringTrigger>()
            .add_direction(NetworkDirection::Bidirectional);
        app.register_event::<EntityTrigger>()
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
        app.register_component::<CompS>();
        app.register_component::<CompFull>()
            .add_prediction()
            .add_linear_interpolation();
        app.register_component::<CompSimple>();
        app.register_component::<CompOnce>();
        app.register_component::<CompCorr>()
            .add_prediction()
            .add_linear_correction_fn()
            .add_linear_interpolation();
        app.register_component::<CompMap>()
            .add_prediction()
            .add_map_entities();
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
        app.add_rollback::<CompNotNetworked>();
        app.register_component::<CompDelta>()
            .add_delta_compression();
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
        app.add_plugins(bei::InputPlugin::<BEIContext>::new(InputConfig::<
            BEIContext,
        > {
            rebroadcast_inputs: true,
            ..default()
        }));
        app.register_input_action::<BEIAction1>();

        app.add_plugins(lightyear::avian2d::plugin::LightyearAvianPlugin {
            replication_mode: self.avian_mode,
            ..default()
        });
        app.add_plugins(
            PhysicsPlugins::default()
                .build()
                // disable the position<>transform sync plugins as it is handled by lightyear_avian
                .disable::<PhysicsTransformPlugin>()
                .disable::<PhysicsInterpolationPlugin>(),
        );
        app.register_component::<FixedJoint>()
            .add_component_map_entities();

        match self.avian_mode {
            AvianReplicationMode::Position => {
                app.add_plugins(FrameInterpolationPlugin::<Position>::default());
                app.add_plugins(FrameInterpolationPlugin::<Rotation>::default());
            }
            AvianReplicationMode::PositionButInterpolateTransform
            | AvianReplicationMode::Transform => {
                app.add_plugins(FrameInterpolationPlugin::<Transform>::default());
            }
        }

        match self.avian_mode {
            AvianReplicationMode::Position
            | AvianReplicationMode::PositionButInterpolateTransform => {
                app.register_component::<Position>()
                    .add_prediction()
                    .add_linear_correction_fn()
                    .add_linear_interpolation();

                app.register_component::<Rotation>()
                    .add_prediction()
                    .add_linear_correction_fn()
                    .add_linear_interpolation();
            }
            AvianReplicationMode::Transform => {
                app.register_component::<Transform>()
                    .add_prediction()
                    .add_linear_correction_fn::<Isometry2d>()
                    .add_interpolation_with(TransformLinearInterpolation::lerp);
            }
        }
    }
}
