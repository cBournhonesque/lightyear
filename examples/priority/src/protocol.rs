use bevy::prelude::*;
use bevy::utils::EntityHashSet;
use derive_more::{Add, Mul};
use leafwing_input_manager::action_state::ActionState;
use leafwing_input_manager::input_map::InputMap;
use leafwing_input_manager::prelude::Actionlike;
use leafwing_input_manager::InputManagerBundle;
use lightyear::prelude::*;
use lightyear::shared::replication::components::ReplicationMode;
use serde::{Deserialize, Serialize};
use tracing::info;
use UserAction;

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    position: Position,
    color: PlayerColor,
    replicate: Replicate,
    action_state: ActionState<Inputs>,
}

impl PlayerBundle {
    pub(crate) fn new(id: ClientId, position: Vec2, color: Color) -> Self {
        let mut replicate = Replicate {
            prediction_target: NetworkTarget::Single(id),
            interpolation_target: NetworkTarget::AllExceptSingle(id),
            // use rooms for replication
            replication_mode: ReplicationMode::Room,
            ..default()
        };
        // We don't want to replicate the ActionState to the original client, since they are updating it with
        // their own inputs (if you replicate it to the original client, it will be added on the Confirmed entity,
        // which will keep syncing it to the Predicted entity because the ActionState gets updated every tick)!
        replicate.add_target::<ActionState<Inputs>>(NetworkTarget::AllExceptSingle(id));
        Self {
            id: PlayerId(id),
            position: Position(position),
            color: PlayerColor(color),
            replicate,
            action_state: ActionState::default(),
        }
    }
    pub(crate) fn get_input_map() -> InputMap<Inputs> {
        InputMap::new([
            (KeyCode::Right, Inputs::Right),
            (KeyCode::D, Inputs::Right),
            (KeyCode::Left, Inputs::Left),
            (KeyCode::A, Inputs::Left),
            (KeyCode::Up, Inputs::Up),
            (KeyCode::W, Inputs::Up),
            (KeyCode::Down, Inputs::Down),
            (KeyCode::S, Inputs::Down),
            (KeyCode::Delete, Inputs::Delete),
            (KeyCode::Space, Inputs::Spawn),
            (KeyCode::M, Inputs::Message),
        ])
    }
}

// Components

#[derive(Component, Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(pub ClientId);

#[derive(
    Component, Message, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut, Add, Mul,
)]
pub struct Position(pub(crate) Vec2);

#[derive(Component, Message, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct PlayerColor(pub(crate) Color);

#[derive(Component, Deref, DerefMut)]
pub struct ShapeChangeTimer(pub(crate) Timer);

#[derive(Component, Message, Deserialize, Serialize, Clone, Debug, PartialEq)]
// Marker component
pub struct Circle;

#[derive(Component, Message, Deserialize, Serialize, Clone, Debug, PartialEq)]
// Marker component
pub struct Triangle;

#[derive(Component, Message, Deserialize, Serialize, Clone, Debug, PartialEq)]
// Marker component
pub struct Square;

// Example of a component that contains an entity.
// This component, when replicated, needs to have the inner entity mapped from the Server world
// to the client World.
// This can be done by adding a `#[message(custom_map)]` attribute to the component, and then
// deriving the `MapEntities` trait for the component.
#[derive(Component, Message, Deserialize, Serialize, Clone, Debug, PartialEq)]
#[message(custom_map)]
pub struct PlayerParent(Entity);

impl<'a> MapEntities<'a> for PlayerParent {
    fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {
        info!("mapping parent entity {:?}", self.0);
        self.0.map_entities(entity_mapper);
        info!("After mapping: {:?}", self.0);
    }

    fn entities(&self) -> EntityHashSet<Entity> {
        EntityHashSet::from_iter(vec![self.0])
    }
}

#[component_protocol(protocol = "MyProtocol")]
pub enum Components {
    #[sync(once)]
    PlayerId(PlayerId),
    #[sync(full)]
    PlayerPosition(Position),
    #[sync(once)]
    PlayerColor(PlayerColor),
    Circle(Circle),
    Triangle(Triangle),
    Square(Square),
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

#[derive(
    Serialize, Deserialize, Debug, Default, PartialEq, Eq, Hash, Reflect, Clone, Copy, Actionlike,
)]
pub enum Inputs {
    Up,
    Down,
    Left,
    Right,
    Delete,
    Spawn,
    Message,
    #[default]
    None,
}

impl LeafwingUserAction for Inputs {}

// Protocol

protocolize! {
    Self = MyProtocol,
    Message = Messages,
    Component = Components,
    LeafwingInput1 = Inputs,
}

pub(crate) fn protocol() -> MyProtocol {
    let mut protocol = MyProtocol::default();
    protocol.add_channel::<Channel1>(ChannelSettings {
        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
        ..default()
    });
    protocol
}
