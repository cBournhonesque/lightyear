use bevy::prelude::*;
use bevy_xpbd_2d::prelude::*;
use derive_more::{Add, Mul};
use leafwing_input_manager::prelude::*;
use serde::{Deserialize, Serialize};

use lightyear::client::components::{ComponentSyncMode, LerpFn};
use lightyear::client::interpolation::LinearInterpolator;
use lightyear::prelude::*;
use lightyear::utils::bevy_xpbd_2d::*;

use crate::shared::color_from_id;

pub const BALL_SIZE: f32 = 15.0;
pub const PLAYER_SIZE: f32 = 40.0;

// For prediction, we want everything entity that is predicted to be part of the same replication group
// This will make sure that they will be replicated in the same message and that all the entities in the group
// will always be consistent (= on the same tick)
pub const REPLICATION_GROUP: ReplicationGroup = ReplicationGroup::new_id(1);

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    position: Position,
    color: ColorComponent,
    replicate: Replicate,
    physics: PhysicsBundle,
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
            position: Position(position),
            color: ColorComponent(color),
            replicate: Replicate {
                target: ReplicationTarget {
                    // TODO: improve this! this should depend on the predict_all settings
                    // We still need to specify the interpolation/prediction target for this local entity
                    // in the case where we're running in HostServer mode
                    prediction: NetworkTarget::All,
                    ..default()
                },
                // NOTE (important): all entities that are being predicted need to be part of the same replication-group
                //  so that all their updates are sent as a single message and are consistent (on the same tick)
                group: REPLICATION_GROUP,
                ..default()
            },
            physics: PhysicsBundle::player(),
            inputs: InputManagerBundle::<PlayerActions> {
                action_state: ActionState::default(),
                input_map,
            },
            pre_predicted: PrePredicted::default(),
        }
    }
}

// Ball
#[derive(Bundle)]
pub(crate) struct BallBundle {
    position: Position,
    color: ColorComponent,
    replicate: Replicate,
    marker: BallMarker,
    physics: PhysicsBundle,
}

impl BallBundle {
    pub(crate) fn new(position: Vec2, color: Color, predicted: bool) -> Self {
        let mut replication_target = ReplicationTarget::default();
        let mut group = ReplicationGroup::default();
        if predicted {
            replication_target.prediction = NetworkTarget::All;
            group = REPLICATION_GROUP;
        } else {
            replication_target.interpolation = NetworkTarget::All;
        }
        let replicate = Replicate {
            target: replication_target,
            group,
            ..default()
        };
        Self {
            position: Position(position),
            color: ColorComponent(color),
            replicate,
            physics: PhysicsBundle::ball(),
            marker: BallMarker,
        }
    }
}

#[derive(Bundle)]
pub(crate) struct PhysicsBundle {
    pub(crate) collider: Collider,
    pub(crate) collider_density: ColliderDensity,
    pub(crate) rigid_body: RigidBody,
}

impl PhysicsBundle {
    pub(crate) fn ball() -> Self {
        Self {
            collider: Collider::circle(BALL_SIZE),
            collider_density: ColliderDensity(0.05),
            rigid_body: RigidBody::Dynamic,
        }
    }

    pub(crate) fn player() -> Self {
        Self {
            collider: Collider::rectangle(PLAYER_SIZE, PLAYER_SIZE),
            collider_density: ColliderDensity(0.2),
            rigid_body: RigidBody::Dynamic,
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
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
pub enum AdminActions {
    SendMessage,
    Reset,
}

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
        app.register_component::<PlayerId>(ChannelDirection::Bidirectional)
            .add_prediction(ComponentSyncMode::Once)
            .add_interpolation(ComponentSyncMode::Once);

        app.register_component::<ColorComponent>(ChannelDirection::Bidirectional)
            .add_prediction(ComponentSyncMode::Once)
            .add_interpolation(ComponentSyncMode::Once);

        app.register_component::<BallMarker>(ChannelDirection::Bidirectional)
            .add_prediction(ComponentSyncMode::Once)
            .add_interpolation(ComponentSyncMode::Once);

        app.register_component::<Position>(ChannelDirection::Bidirectional)
            .add_prediction(ComponentSyncMode::Full)
            .add_interpolation(ComponentSyncMode::Full)
            .add_interpolation_fn(position::lerp)
            .add_correction_fn(position::lerp);

        app.register_component::<Rotation>(ChannelDirection::Bidirectional)
            .add_prediction(ComponentSyncMode::Full)
            .add_interpolation(ComponentSyncMode::Full)
            .add_interpolation_fn(rotation::lerp)
            .add_correction_fn(rotation::lerp);

        // NOTE: interpolation/correction is only needed for components that are visually displayed!
        // we still need prediction to be able to correctly predict the physics on the client
        app.register_component::<LinearVelocity>(ChannelDirection::Bidirectional)
            .add_prediction(ComponentSyncMode::Full);

        app.register_component::<AngularVelocity>(ChannelDirection::Bidirectional)
            .add_prediction(ComponentSyncMode::Full);

        // channels
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        });
    }
}
