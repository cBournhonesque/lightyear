#[cfg(not(feature = "std"))]
use alloc::string::String;
use avian2d::prelude::*;
use bevy::ecs::entity::MapEntities;
use bevy::ecs::error::Result;
use bevy::prelude::*;
use bevy_enhanced_input::prelude::InputAction;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::postcard_utils;
use bevy_replicon::shared::replication::diff::Diffable as RepliconDiffable;
use bevy_replicon::shared::replication::registry::ctx::{SerializeCtx, WriteCtx};
use leafwing_input_manager::Actionlike;
use lightyear::avian2d::plugin::AvianReplicationMode;
use lightyear::frame_interpolation::FrameInterpolationPlugin;
use lightyear::prelude::input::*;
use lightyear::prelude::*;
use lightyear_connection::direction::NetworkDirection;
use lightyear_replication::delta::Diffable as DeltaDiffable;
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

#[derive(Component, Clone, Debug, PartialEq, Reflect)]
pub struct CompCustomReplicateOnce(pub f32);

fn serialize_custom_replicate_once(
    _ctx: &mut SerializeCtx,
    component: &CompCustomReplicateOnce,
    message: &mut Vec<u8>,
) -> Result<()> {
    postcard_utils::to_extend_mut(&component.0, message)?;
    Ok(())
}

fn deserialize_custom_replicate_once(
    _ctx: &mut WriteCtx,
    message: &mut Bytes,
) -> Result<CompCustomReplicateOnce> {
    let value = postcard_utils::from_buf(message)?;
    Ok(CompCustomReplicateOnce(value))
}

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
pub struct CompBundleA(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompBundleB(pub f32);

fn bundle_lerp(
    start: (CompBundleA, CompBundleB),
    end: (CompBundleA, CompBundleB),
    t: f32,
) -> (CompBundleA, CompBundleB) {
    (
        CompBundleA(100.0 + start.0.0 + (end.0.0 - start.0.0) * t),
        CompBundleB(200.0 + start.1.0 + (end.1.0 - start.1.0) * t),
    )
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompCustomInterp(pub f32);

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

impl DeltaDiffable for CompCorr {
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
impl DeltaDiffable<usize> for CompDelta {
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

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct CompRepliconDiff(pub u32);

impl RepliconDiffable for CompRepliconDiff {
    type Diff = u32;

    fn apply_diff(&mut self, diff: &Self::Diff) -> Result<()> {
        self.0 = *diff;
        Ok(())
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
        app.component::<CompA>().replicate();
        app.component::<CompS>().replicate();
        app.component::<CompReplicateOnce>().replicate_once();
        app.component::<CompCustomReplicateOnce>()
            .replicate_once_with(bevy_replicon::prelude::RuleFns::new(
                serialize_custom_replicate_once,
                deserialize_custom_replicate_once,
            ));
        app.component::<CompFull>()
            .replicate()
            .predict()
            .add_linear_interpolation();
        app.component::<CompSimple>().replicate();
        app.component::<CompBundleA>().replicate();
        app.component::<CompBundleB>().replicate();
        app.interpolate_bundle_with::<(CompBundleA, CompBundleB)>(InterpolationFns::interpolate(
            bundle_lerp,
        ));
        app.component::<CompCustomInterp>()
            .replicate()
            .add_custom_interpolation();
        app.component::<CompOnce>().replicate();
        app.component::<CompCorr>()
            .replicate()
            .predict()
            .add_linear_correction_fn()
            .add_linear_interpolation();
        app.component::<CompMap>().replicate().predict();
        app.local_rollback::<CompNotNetworked>();
        app.component::<CompDelta>().replicate();
        app.component::<CompRepliconDiff>()
            .replicate_diff()
            .predict_diff()
            .add_custom_interpolation_diff();
        // .add_delta_compression();
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
                .disable::<IslandSleepingPlugin>()
                .disable::<PhysicsInterpolationPlugin>(),
        );
        // app.component::<Collider>().replicate();

        app.add_plugins(FrameInterpolationPlugin);

        match self.avian_mode {
            AvianReplicationMode::Position
            | AvianReplicationMode::PositionButInterpolateTransform => {
                app.component::<Position>()
                    .replicate()
                    .predict()
                    .add_linear_correction_fn()
                    .add_linear_interpolation();

                app.component::<Rotation>()
                    .replicate()
                    .predict()
                    .add_linear_correction_fn()
                    .add_linear_interpolation();
            }
            AvianReplicationMode::Transform => {
                app.component::<Transform>()
                    .replicate()
                    .predict()
                    .add_linear_correction_fn::<Isometry2d>()
                    .add_interpolation_with(TransformLinearInterpolation::lerp);
            }
        }
    }
}
