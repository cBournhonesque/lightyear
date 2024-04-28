use bevy::prelude::*;
use derive_more::{Add, Mul};
use leafwing_input_manager::prelude::*;
use serde::{Deserialize, Serialize};

use crate::shared::color_from_id;
use lightyear::client::components::{ComponentSyncMode, LerpFn};
use lightyear::prelude::*;
use lightyear::shared::replication::components::ReplicationGroupIdBuilder;
use lightyear::utils::bevy::*;

pub const BALL_SIZE: f32 = 10.0;
pub const PLAYER_SIZE: f32 = 40.0;

// For prediction, we want everything entity that is predicted to be part of the same replication group
// This will make sure that they will be replicated in the same message and that all the entities in the group
// will always be consistent (= on the same tick)
pub const REPLICATION_GROUP: ReplicationGroup = ReplicationGroup::new_id(1);

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    transform: Transform,
    color: ColorComponent,
    replicate: Replicate,
    inputs: InputManagerBundle<PlayerActions>,
    // IMPORTANT: this lets the server know that the entity is pre-predicted
    // when the server replicates this entity; we will get a Confirmed entity which will use this entity
    // as the Predicted version
    pre_predicted: PrePredicted,
}

impl PlayerBundle {
    pub(crate) fn new(id: ClientId, position: Vec2, input_map: InputMap<PlayerActions>) -> Self {
        let color = color_from_id(id);
        Self {
            id: PlayerId(id),
            transform: Transform::from_xyz(position.x, position.y, 0.0),
            color: ColorComponent(color),
            replicate: Replicate {
                // NOTE (important): all entities that are being predicted need to be part of the same replication-group
                //  so that all their updates are sent as a single message and are consistent (on the same tick)
                replication_group: ReplicationGroup::new_id(id.to_bits()),
                // For HostServer mode, remember to also set prediction/interpolation targets for other clients
                interpolation_target: NetworkTarget::AllExceptSingle(id),
                ..default()
            },
            inputs: InputManagerBundle::<PlayerActions> {
                action_state: ActionState::default(),
                input_map,
            },
            // IMPORTANT: this lets the server know that the entity is pre-predicted
            pre_predicted: PrePredicted::default(),
        }
    }
}

// Ball
#[derive(Bundle)]
pub(crate) struct BallBundle {
    transform: Transform,
    color: ColorComponent,
    marker: BallMarker,
}

impl BallBundle {
    pub(crate) fn new(
        position: Vec2,
        rotation_radians: f32,
        color: Color,
        predicted: bool,
    ) -> Self {
        let mut transform = Transform::from_xyz(position.x, position.y, 0.0);
        transform.rotate_z(rotation_radians);
        Self {
            transform,
            color: ColorComponent(color),
            marker: BallMarker,
        }
    }
}

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct PlayerId(pub ClientId);

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct ColorComponent(pub(crate) Color);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BallMarker;

// Channels

#[derive(Channel)]
pub struct Channel1;

// Messages

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message1(pub usize);

// Inputs

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
pub enum PlayerActions {
    Up,
    Down,
    Left,
    Right,
    Shoot,
    MoveCursor,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
pub enum AdminActions {
    SendMessage,
    Reset,
}

// Protocol

// Protocol
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // messages
        app.add_message::<Message1>(ChannelDirection::Bidirectional);
        // inputs
        app.add_plugins(LeafwingInputPlugin::<PlayerActions>::default());
        app.add_plugins(LeafwingInputPlugin::<AdminActions>::default());
        // components
        app.register_component::<PlayerId>(ChannelDirection::Bidirectional);
        app.add_prediction::<PlayerId>(ComponentSyncMode::Once);
        app.add_interpolation::<PlayerId>(ComponentSyncMode::Once);

        app.register_component::<Transform>(ChannelDirection::ServerToClient);
        app.add_prediction::<Transform>(ComponentSyncMode::Full);
        app.add_interpolation::<Transform>(ComponentSyncMode::Full);
        app.add_interpolation_fn::<Transform>(TransformLinearInterpolation::lerp);

        app.register_component::<ColorComponent>(ChannelDirection::ServerToClient);
        app.add_prediction::<ColorComponent>(ComponentSyncMode::Once);
        app.add_interpolation::<ColorComponent>(ComponentSyncMode::Once);

        app.register_component::<BallMarker>(ChannelDirection::ServerToClient);
        app.add_prediction::<BallMarker>(ComponentSyncMode::Once);
        app.add_interpolation::<BallMarker>(ComponentSyncMode::Once);
        // channels
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        });
    }
}
