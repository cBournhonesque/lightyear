use crate::serialize::writer::WriteBuffer;
use crate::BitSerializable;

// client writes an Enum containing all their message type
// each message must derive message

// that big enum will implement MessageProtocol via a proc macro
pub trait MessageProtocol: BitSerializable {}
