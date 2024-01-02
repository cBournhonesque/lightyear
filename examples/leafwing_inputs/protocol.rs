use bevy::prelude::*;
use bevy::utils::EntityHashSet;
use derive_more::{Add, Mul};
use leafwing_input_manager::prelude::*;
use lightyear::_reexport::ShouldBePredicted;
use lightyear::inputs::leafwing::UserAction;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    position: Position,
    color: ColorComponent,
    replicate: Replicate,
    inputs: InputManagerBundle<PlayerActions>,
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
            position: Position(position),
            color: ColorComponent(color),
            replicate: Replicate {
                // prediction_target: NetworkTarget::None,
                prediction_target: NetworkTarget::Only(vec![id]),
                interpolation_target: NetworkTarget::AllExcept(vec![id]),
                ..default()
            },
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
    position: Position,
    color: ColorComponent,
    replicate: Replicate,
    marker: BallMarker,
}

impl BallBundle {
    pub(crate) fn new(position: Vec2, color: Color) -> Self {
        Self {
            position: Position(position),
            color: ColorComponent(color),
            replicate: Replicate {
                replication_target: NetworkTarget::All,
                // the ball is predicted by all players!
                prediction_target: NetworkTarget::All,
                // prediction_target: NetworkTarget::Only(vec![id]),
                // interpolation_target: NetworkTarget::AllExcept(vec![id]),
                ..default()
            },
            marker: BallMarker,
        }
    }
}

// Components
#[derive(Component, Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(pub ClientId);

#[derive(
    Component, Message, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut, Add, Mul,
)]
pub struct Position(Vec2);

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
    #[sync(full)]
    Position(Position),
    #[sync(once)]
    ColorComponent(ColorComponent),
    #[sync(full)]
    CursorPosition(CursorPosition),
    #[sync(once)]
    BallMarker(BallMarker),
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

impl UserAction for PlayerActions {}
impl UserAction for AdminActions {}

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
