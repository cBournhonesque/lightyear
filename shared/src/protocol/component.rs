use crate::serialize::writer::WriteBuffer;
use crate::BitSerializable;

// client writes an Enum containing all their message type
// each message must derive message

// that big enum will implement MessageProtocol via a proc macro
pub trait ComponentProtocol: BitSerializable {}

// user provides an enum of all possible components that might be replicated
// enum MyComponentProtocol {
//  component1,
//  component2,
// }

// and via a derive macro we generate the final enum of things that we can send via the network

// enum ComponentProtocol {
//   EntitySpawned(Entity),
//   EntityDespawned(Entity),
//   ComponentInserted(Entity, MyComponentProtocol),
//   ComponentRemoved(Entity, ComponentKind),
//   EntityUpdate(Entity, Vec<MyComponentProtocol>),   -> entity + all components that changed
// }
