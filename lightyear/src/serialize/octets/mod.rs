use core::fmt;

#[derive(thiserror::Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerializationError {
    #[error(transparent)]
    BufferTooShort(#[from] octets::BufferTooShortError),
    InvalidNumSlices,
    EmptySlice,
    InvalidAckRange,
    InvalidPacketType,
}

pub trait ToBytes {
    fn len(&self) -> usize;
    fn to_bytes(&self, octets: &mut octets::OctetsMut) -> Result<(), SerializationError>;

    fn from_bytes(octets: &mut octets::Octets) -> Result<Self, SerializationError>
    where
        Self: Sized;
}
