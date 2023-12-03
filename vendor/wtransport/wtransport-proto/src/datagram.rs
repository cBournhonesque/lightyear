use crate::bytes::BufferReader;
use crate::bytes::BufferWriter;
use crate::bytes::BytesReader;
use crate::bytes::BytesWriter;
use crate::bytes::EndOfBuffer;
use crate::error::ErrorCode;
use crate::ids::InvalidQStreamId;
use crate::ids::QStreamId;

/// An HTTP3 datagram.
pub struct Datagram<'a> {
    qstream_id: QStreamId,
    payload: &'a [u8],
}

impl<'a> Datagram<'a> {
    /// Creates a new [`Datagram`] with a given payload.
    #[inline(always)]
    pub fn new(qstream_id: QStreamId, payload: &'a [u8]) -> Self {
        Self {
            qstream_id,
            payload,
        }
    }

    /// Reads [`Datagram`] from a QUIC datagram.
    pub fn read(quic_datagram: &'a [u8]) -> Result<Self, ErrorCode> {
        let mut buffer_reader = BufferReader::new(quic_datagram);

        let varint = buffer_reader.get_varint().ok_or(ErrorCode::Datagram)?;

        let qstream_id =
            QStreamId::try_from_varint(varint).map_err(|InvalidQStreamId| ErrorCode::Datagram)?;

        let payload = buffer_reader.buffer_remaining();

        Ok(Self {
            qstream_id,
            payload,
        })
    }

    /// Writes a [`Datagram`] as QUIC datagram into `buffer`.
    ///
    /// It returns the number of bytes written.
    /// It returns [`Err`] if the `buffer` does not have enough capacity.
    /// See [`Self::write_size`].
    ///
    /// In case of [`Err`], `buffer` is not written.
    pub fn write(&self, buffer: &mut [u8]) -> Result<usize, EndOfBuffer> {
        if buffer.len() < self.write_size() {
            return Err(EndOfBuffer);
        }

        let mut buffer_writer = BufferWriter::new(buffer);

        buffer_writer
            .put_varint(self.qstream_id.into_varint())
            .expect("Buffer has capacity");

        buffer_writer
            .put_bytes(self.payload)
            .expect("Buffer has capacity");

        Ok(buffer_writer.offset())
    }

    /// Returns the needed capacity to write this datagram into a buffer.
    #[inline(always)]
    pub fn write_size(&self) -> usize {
        Self::header_size(self.qstream_id) + self.payload.len()
    }

    /// Returns the HTTP3 header.
    ///
    /// Computes the space overhead (HTTP3 header) due to the `qstream_id`
    /// encoding into an HTTP3 datagram.
    #[inline(always)]
    pub fn header_size(qstream_id: QStreamId) -> usize {
        qstream_id.into_varint().size()
    }

    /// Returns the associated [`QStreamId`].
    #[inline(always)]
    pub fn qstream_id(&self) -> QStreamId {
        self.qstream_id
    }

    /// Returns the payload.
    #[inline(always)]
    pub fn payload(&self) -> &[u8] {
        self.payload
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::varint::VarInt;
    use utils::build_datagram;
    use utils::QStreamIdType;
    use utils::PAYLOAD;

    #[test]
    fn read_ok() {
        let dgram = build_datagram(QStreamIdType::Valid, PAYLOAD);
        let qstream_id = dgram.qstream_id();

        let mut buffer = vec![0; dgram.write_size() + 42];
        let written = dgram.write(&mut buffer).unwrap();

        let dgram = Datagram::read(&buffer[..written]).unwrap();
        assert_eq!(dgram.qstream_id(), qstream_id);
        assert_eq!(dgram.payload(), PAYLOAD);
    }

    #[test]
    fn read_too_short() {
        let dgram = build_datagram(QStreamIdType::Valid, PAYLOAD);

        let mut buffer = vec![0; dgram.write_size() + 42];
        dgram.write(&mut buffer).unwrap();

        assert!(matches!(
            Datagram::read(&buffer[..1]),
            Err(ErrorCode::Datagram)
        ));
    }

    #[test]
    fn read_invalid_qstream_id() {
        let dgram = build_datagram(QStreamIdType::Invalid, PAYLOAD);

        let mut buffer = vec![0; dgram.write_size() + 42];
        let written = dgram.write(&mut buffer).unwrap();

        assert!(matches!(
            Datagram::read(&buffer[..written]),
            Err(ErrorCode::Datagram)
        ));
    }

    #[test]
    fn write_ok() {
        let dgram = build_datagram(QStreamIdType::Valid, PAYLOAD);
        let dgram_write_size = dgram.write_size();

        let mut buffer = vec![0; dgram_write_size];
        let written = dgram.write(&mut buffer).unwrap();
        assert_eq!(written, dgram_write_size);
    }

    #[test]
    fn write_out() {
        let dgram = build_datagram(QStreamIdType::Valid, PAYLOAD);
        let dgram_write_size = dgram.write_size();

        let mut buffer = vec![0; dgram_write_size - 1];
        assert!(dgram.write(&mut buffer).is_err());
    }

    mod utils {
        use super::*;

        pub const PAYLOAD: &[u8] = b"This is a testing payload";

        pub enum QStreamIdType {
            Valid,
            Invalid,
        }

        impl QStreamIdType {
            /// This function is for **testing purpose only**; it might produce an invalid `QStreamId`!
            fn into_session_id(self) -> QStreamId {
                match self {
                    QStreamIdType::Valid => QStreamId::MAX,
                    QStreamIdType::Invalid => {
                        let varint = VarInt::try_from_u64(QStreamId::MAX.into_u64() + 1).unwrap();
                        QStreamId::maybe_invalid(varint)
                    }
                }
            }
        }

        /// This function is for **testing purpose only**; it might produce an invalid `Datagram`!
        pub fn build_datagram(qstream_id_type: QStreamIdType, payload: &[u8]) -> Datagram {
            Datagram::new(qstream_id_type.into_session_id(), payload)
        }
    }
}
