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

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    position: Position,
    color: PlayerColor,
    replicate: Replicate,
    inputs: InputManagerBundle<Inputs>,
}

impl PlayerBundle {
    pub(crate) fn new(id: ClientId, position: Vec2, color: Color) -> Self {
        Self {
            id: PlayerId(id),
            position: Position(position),
            color: PlayerColor(color),
            replicate: Replicate {
                prediction_target: NetworkTarget::Only(vec![id]),
                interpolation_target: NetworkTarget::AllExcept(vec![id]),
                // use rooms for replication
                replication_mode: ReplicationMode::Room,
                ..default()
            },
            inputs: InputManagerBundle::<Inputs> {
                action_state: ActionState::default(),
                input_map: InputMap::new([
                    (KeyCode::Right, Inputs::Right),
                    (KeyCode::Left, Inputs::Left),
                    (KeyCode::Up, Inputs::Up),
                    (KeyCode::Down, Inputs::Down),
                    (KeyCode::Delete, Inputs::Delete),
                    (KeyCode::Space, Inputs::Spawn),
                ]),
            },
        }
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

#[derive(Component, Message, Deserialize, Serialize, Clone, Debug, PartialEq)]
// Marker component
pub struct Circle;

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
    #[sync(once)]
    Circle(Circle),
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
    Serialize, Deserialize, Debug, Default, PartialEq, Eq, Hash, Reflect, Clone, Actionlike,
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

impl Inputs {
    /// Get the mapping from keycodes to inputs
    pub(crate) fn get_input_map() -> InputMap<Inputs> {
        use KeyCode::*;
        InputMap::new([
            (Right, Inputs::Right),
            (D, Inputs::Right),
            (Left, Inputs::Left),
            (A, Inputs::Left),
            (Up, Inputs::Up),
            (W, Inputs::Up),
            (Down, Inputs::Down),
            (S, Inputs::Down),
            (Delete, Inputs::Delete),
            (Space, Inputs::Spawn),
            (M, Inputs::Message),
        ])
    }
}

impl UserAction for Inputs {}

// Protocol

protocolize! {
    Self = MyProtocol,
    Message = Messages,
    Component = Components,
    Input = Inputs,
}

pub(crate) fn protocol() -> MyProtocol {
    let mut protocol = MyProtocol::default();
    protocol.add_channel::<Channel1>(ChannelSettings {
        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
        direction: ChannelDirection::Bidirectional,
    });
    protocol
}
