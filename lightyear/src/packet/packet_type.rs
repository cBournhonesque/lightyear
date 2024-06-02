

#[repr(u8)]
#[derive(Copy, Debug, Clone, Eq, PartialEq)]
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
    Data = 0,
    DataFragment = 1,
}

impl From<PacketType> for u8 {
    fn from(packet_type: PacketType) -> u8 {
        packet_type as u8
    }
}

impl TryFrom<u8> for PacketType {
    type Error = crate::serialize::SerializationError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(PacketType::Data),
            1 => Ok(PacketType::DataFragment),
            _ => Err(crate::serialize::SerializationError::InvalidPacketType),
        }
    }
}
