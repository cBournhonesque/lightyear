use serde::{Deserialize, Serialize};

use bitcode::{Decode, Encode};

#[repr(u8)]
#[derive(Copy, Debug, Clone, Eq, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub enum PacketType {
    /// A packet containing actual data
    ///
    /// Will be serialized like:
    /// - header
    /// - channel_id_1
    /// - num messages
    /// - single_data_1
    /// - single_data_2
    /// - channel_id_2
    /// - num messages
    /// - ...
    /// - channel_id = 0 = indication of end of packet
    #[bitcode_hint(frequency = 100)]
    Data = 0,
    // // A packet sent to maintain the connection by preventing a timeout
    // #[bitcode_hint(frequency = 50)]
    // KeepAlive,
    // // A Ping message, used to calculate RTT. Must be responded to with a Pong
    // // message
    // #[bitcode_hint(frequency = 1)]
    // Ping,
    // // A Pong message, used to calculate RTT. Must be the response to all Ping
    // // messages
    // #[bitcode_hint(frequency = 1)]
    // Pong,
    // A packet containing actual data, but which is fragmented into multiple parts
    #[bitcode_hint(frequency = 5)]
    DataFragment = 1,
}

impl From<PacketType> for u8 {
    fn from(packet_type: PacketType) -> u8 {
        packet_type as u8
    }
}

impl TryFrom<u8> for PacketType {
    type Error = crate::serialize::octets::SerializationError::InvalidPacketType;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(PacketType::Data),
            1 => Ok(PacketType::DataFragment),
            _ => Err(crate::serialize::octets::SerializationError::InvalidPacketType),
        }
    }
}
