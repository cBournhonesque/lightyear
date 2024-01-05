use bevy::prelude::*;
use bevy::utils::EntityHashSet;
use bevy_xpbd_2d::prelude::*;
use derive_more::{Add, Mul};
use leafwing_input_manager::prelude::*;
use lightyear::_reexport::ShouldBePredicted;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

pub const BALL_SIZE: f32 = 15.0;
pub const PLAYER_SIZE: f32 = 40.0;

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    // transform: Transform,
    position: Position,
    color: ColorComponent,
    replicate: Replicate,
    physics: PhysicsBundle,
    inputs: InputManagerBundle<PlayerActions>,
}

impl PlayerBundle {
    pub(crate) fn new(
        id: ClientId,
        // transform: Vec2,
        position: Vec2,
        color: Color,
        input_map: InputMap<PlayerActions>,
    ) -> Self {
        Self {
            id: PlayerId(id),
            // transform: Transform::from_xyz(transform.x, transform.y, 0.0),
            position: Position(position),
            color: ColorComponent(color),
            replicate: Replicate {
                // prediction_target: NetworkTarget::None,
                prediction_target: NetworkTarget::Only(vec![id]),
                interpolation_target: NetworkTarget::AllExcept(vec![id]),
                // NOTE (important): all entities that are being predicted need to be part of the same replication-group
                //  so that all their updates are sent as a single message and are consistent (on the same tick)
                replication_group: ReplicationGroup::Group(1),
                ..default()
            },
            physics: PhysicsBundle::player(),
            inputs: InputManagerBundle::<PlayerActions> {
                action_state: ActionState::default(),
                input_map,
            },
        }
    }
}

// Ball
#[derive(Bundle)]
pub(crate) struct BallBundle {
    // transform: Transform,
    position: Position,
    color: ColorComponent,
    replicate: Replicate,
    marker: BallMarker,
    physics: PhysicsBundle,
}

#[derive(Bundle)]
pub(crate) struct PhysicsBundle {
    collider: Collider,
    collider_density: ColliderDensity,
    rigid_body: RigidBody,
}

impl PhysicsBundle {
    pub(crate) fn ball() -> Self {
        Self {
            collider: Collider::ball(BALL_SIZE),
            collider_density: ColliderDensity(2.0),
            rigid_body: RigidBody::Dynamic,
        }
    }

    pub(crate) fn player() -> Self {
        Self {
            collider: Collider::cuboid(PLAYER_SIZE, PLAYER_SIZE),
            collider_density: ColliderDensity(0.2),
            rigid_body: RigidBody::Dynamic,
        }
    }
}

impl BallBundle {
    pub(crate) fn new(position: Vec2, color: Color) -> Self {
        Self {
            // transform: Transform::from_xyz(transform.x, transform.y, 0.0),
            position: Position(position),
            color: ColorComponent(color),
            replicate: Replicate {
                replication_target: NetworkTarget::All,
                // the ball is predicted by all players!
                // prediction_target: NetworkTarget::All,
                // prediction_target: NetworkTarget::Only(vec![id]),
                interpolation_target: NetworkTarget::All,
                // interpolation_target: NetworkTarget::AllExcept(vec![id]),
                ..default()
            },
            physics: PhysicsBundle::ball(),
            marker: BallMarker,
        }
    }
}

// Components
#[derive(Component, Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(pub ClientId);

// #[derive(
//     Component, Message, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut, Add, Mul,
// )]
// pub struct Position(Vec2);

#[derive(Component, Message, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct ColorComponent(pub(crate) Color);

#[derive(Component, Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BallMarker;

#[derive(
    Component, Message, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut, Add, Mul,
)]
pub struct CursorPosition(pub Vec2);

#[component_protocol(protocol = "MyProtocol")]
pub enum Components {
    #[sync(once)]
    PlayerId(PlayerId),
    #[sync(once)]
    ColorComponent(ColorComponent),
    #[sync(full)]
    CursorPosition(CursorPosition),
    #[sync(once)]
    BallMarker(BallMarker),
    // external components have to be marked with this attribute, to avoid compile errors
    // the necessary traits (Message, SyncComponent) must already been implemented on the external type
    // this will be improved in future releases
    // #[sync(external, full)]
    // Transform(Transform),
    #[sync(external, full)]
    Position(Position),
    #[sync(external, full)]
    Rotation(Rotation),
    #[sync(external, full)]
    LinearVelocity(LinearVelocity),
    #[sync(external, full)]
    AngularVelocity(AngularVelocity),
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
    None,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
pub enum AdminActions {
    Reset,
    None,
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
        direction: ChannelDirection::Bidirectional,
    });
    protocol
}
