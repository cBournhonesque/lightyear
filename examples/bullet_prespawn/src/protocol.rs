use bevy::prelude::*;
use derive_more::{Add, Mul};
use leafwing_input_manager::prelude::*;
use serde::{Deserialize, Serialize};

use lightyear::client::components::LerpFn;
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
    should_be_predicted: ShouldBePredicted,
}

impl PlayerBundle {
    pub(crate) fn new(
        id: ClientId,
        position: Vec2,
        color: Color,
        input_map: InputMap<PlayerActions>,
    ) -> Self {
        Self {
            id: PlayerId(id),
            transform: Transform::from_xyz(position.x, position.y, 0.0),
            color: ColorComponent(color),
            replicate: Replicate {
                // NOTE (important): all entities that are being predicted need to be part of the same replication-group
                //  so that all their updates are sent as a single message and are consistent (on the same tick)
                replication_group: ReplicationGroup::new_id(id),
                ..default()
            },
            inputs: InputManagerBundle::<PlayerActions> {
                action_state: ActionState::default(),
                input_map,
            },
            // IMPORTANT: this lets the server know that the entity is pre-predicted
            should_be_predicted: ShouldBePredicted::default(),
        }
    }
}

// Ball
#[derive(Bundle)]
pub(crate) struct BallBundle {
    transform: Transform,
    color: ColorComponent,
    // replicate: Replicate,
    marker: BallMarker,
}

impl BallBundle {
    pub(crate) fn new(
        position: Vec2,
        rotation_radians: f32,
        color: Color,
        predicted: bool,
    ) -> Self {
        // let mut replicate = Replicate {
        //     replication_target: NetworkTarget::None,
        //     ..default()
        // };
        // if predicted {
        //     replicate.prediction_target = NetworkTarget::All;
        //     replicate.replication_group = REPLICATION_GROUP;
        // } else {
        //     replicate.interpolation_target = NetworkTarget::All;
        // }
        let mut transform = Transform::from_xyz(position.x, position.y, 0.0);
        transform.rotate_z(rotation_radians);
        Self {
            transform,
            color: ColorComponent(color),
            // replicate,
            marker: BallMarker,
        }
    }
}

// Components
#[derive(Component, Message, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct PlayerId(pub ClientId);

#[derive(Component, Message, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct ColorComponent(pub(crate) Color);

#[derive(Component, Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BallMarker;

#[component_protocol(protocol = "MyProtocol")]
pub enum Components {
    #[sync(once)]
    PlayerId(PlayerId),
    #[sync(once)]
    ColorComponent(ColorComponent),
    #[sync(once)]
    BallMarker(BallMarker),
    #[sync(full, lerp = "TransformLinearInterpolation")]
    Transform(Transform),
}

// Channels

#[derive(Channel)]
pub struct Channel1;

// Messages

#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message1(pub usize);

#[message_protocol(protocol = "MyProtocol")]
pub enum Messages {
    Message1(Message1),
}

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

impl LeafwingUserAction for PlayerActions {}
impl LeafwingUserAction for AdminActions {}

// Protocol

protocolize! {
    Self = MyProtocol,
    Message = Messages,
    Component = Components,
    Input = (),
    LeafwingInput1 = PlayerActions,
    LeafwingInput2 = AdminActions,
}

pub(crate) fn protocol() -> MyProtocol {
    let mut protocol = MyProtocol::default();
    protocol.add_channel::<Channel1>(ChannelSettings {
        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
        ..default()
    });
    protocol
}
