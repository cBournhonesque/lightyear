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
    DataCompressed = 2,
    /// A packet containing a fragment, where packet-level compression is enabled
    /// for the non-fragment data.
    ///
    /// Compression for the fragment payload itself is tracked separately in the
    /// fragment metadata serialized on the first fragment.
    DataFragmentCompressed = 3,
}

impl From<PacketType> for u8 {
    fn from(packet_type: PacketType) -> u8 {
        packet_type as u8
    }
}

impl TryFrom<u8> for PacketType {
    type Error = lightyear_serde::SerializationError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(PacketType::Data),
            1 => Ok(PacketType::DataFragment),
            2 => Ok(PacketType::DataCompressed),
            3 => Ok(PacketType::DataFragmentCompressed),
            _ => Err(lightyear_serde::SerializationError::InvalidPacketType),
        }
    }
}

impl PacketType {
    pub(crate) fn is_compressed(self) -> bool {
        matches!(
            self,
            PacketType::DataCompressed | PacketType::DataFragmentCompressed
        )
    }

    pub(crate) fn compressed_variant(self) -> Option<Self> {
        match self {
            PacketType::Data => Some(PacketType::DataCompressed),
            PacketType::DataFragment => Some(PacketType::DataFragmentCompressed),
            PacketType::DataCompressed | PacketType::DataFragmentCompressed => None,
        }
    }

    pub(crate) fn uncompressed_variant(self) -> Self {
        match self {
            PacketType::Data | PacketType::DataCompressed => PacketType::Data,
            PacketType::DataFragment | PacketType::DataFragmentCompressed => {
                PacketType::DataFragment
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PacketType;

    #[test]
    fn packet_type_roundtrip_includes_compressed_variants() {
        for packet_type in [
            PacketType::Data,
            PacketType::DataFragment,
            PacketType::DataCompressed,
            PacketType::DataFragmentCompressed,
        ] {
            let raw: u8 = packet_type.into();
            assert_eq!(PacketType::try_from(raw).unwrap(), packet_type);
        }
    }

    #[test]
    fn packet_type_compressed_helpers_map_to_original_variants() {
        assert_eq!(
            PacketType::Data.compressed_variant(),
            Some(PacketType::DataCompressed)
        );
        assert_eq!(
            PacketType::DataFragment.compressed_variant(),
            Some(PacketType::DataFragmentCompressed)
        );
        assert_eq!(
            PacketType::DataCompressed.uncompressed_variant(),
            PacketType::Data
        );
        assert_eq!(
            PacketType::DataFragmentCompressed.uncompressed_variant(),
            PacketType::DataFragment
        );
    }
}
