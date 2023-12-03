use crate::bytes::BufferReader;
use crate::bytes::BufferWriter;
use crate::bytes::BytesReader;
use crate::bytes::BytesWriter;
use crate::bytes::EndOfBuffer;
use crate::ids::InvalidSessionId;
use crate::ids::SessionId;
use crate::varint::VarInt;
use std::borrow::Cow;

#[cfg(feature = "async")]
use crate::bytes::AsyncRead;

#[cfg(feature = "async")]
use crate::bytes::AsyncWrite;

#[cfg(feature = "async")]
use crate::bytes;

/// Error frame parsing.
#[derive(Debug)]
pub enum ParseError {
    /// Error for unknown frame ID.
    UnknownFrame,

    /// Error for invalid session ID.
    InvalidSessionId,

    /// Payload required too big.
    PayloadTooBig,
}

/// An error during frame I/O read operation.
#[cfg(feature = "async")]
#[cfg_attr(docsrs, doc(cfg(feature = "async")))]
#[derive(Debug)]
pub enum IoReadError {
    /// Error during parsing a frame.
    Parse(ParseError),

    /// Error due to I/O operation.
    IO(bytes::IoReadError),
}

#[cfg(feature = "async")]
impl From<bytes::IoReadError> for IoReadError {
    #[inline(always)]
    fn from(io_error: bytes::IoReadError) -> Self {
        IoReadError::IO(io_error)
    }
}

/// An error during frame I/O write operation.
#[cfg(feature = "async")]
pub type IoWriteError = bytes::IoWriteError;

/// Alias for [`Frame<'static>`](Frame);
pub type FrameOwned = Frame<'static>;

/// An HTTP3 [`Frame`] type.
#[derive(Copy, Clone, Debug)]
pub enum FrameKind {
    /// DATA frame type.
    Data,

    /// HEADERS frame type.
    Headers,

    /// SETTINGS frame type.
    Settings,

    /// WebTransport frame type.
    WebTransport,

    /// Exercise frame.
    Exercise(VarInt),
}

impl FrameKind {
    /// Checks whether an `id` is valid for a [`FrameKind::Exercise`].
    #[inline(always)]
    pub const fn is_id_exercise(id: VarInt) -> bool {
        id.into_inner() >= 0x21 && ((id.into_inner() - 0x21) % 0x1f == 0)
    }

    const fn parse(id: VarInt) -> Option<Self> {
        match id {
            frame_kind_ids::DATA => Some(FrameKind::Data),
            frame_kind_ids::HEADERS => Some(FrameKind::Headers),
            frame_kind_ids::SETTINGS => Some(FrameKind::Settings),
            frame_kind_ids::WEBTRANSPORT_STREAM => Some(FrameKind::WebTransport),
            id if FrameKind::is_id_exercise(id) => Some(FrameKind::Exercise(id)),
            _ => None,
        }
    }

    const fn id(self) -> VarInt {
        match self {
            FrameKind::Data => frame_kind_ids::DATA,
            FrameKind::Headers => frame_kind_ids::HEADERS,
            FrameKind::Settings => frame_kind_ids::SETTINGS,
            FrameKind::WebTransport => frame_kind_ids::WEBTRANSPORT_STREAM,
            FrameKind::Exercise(id) => id,
        }
    }
}

/// An HTTP3 frame.
pub struct Frame<'a> {
    kind: FrameKind,
    payload: Cow<'a, [u8]>,
    session_id: Option<SessionId>,
}

impl<'a> Frame<'a> {
    const MAX_PARSE_PAYLOAD_ALLOWED: usize = 4096;

    /// Creates a new frame of type [`FrameKind::Headers`].
    ///
    /// # Panics
    ///
    /// Panics if the `payload` size if greater than [`VarInt::MAX`].
    #[inline(always)]
    pub fn new_headers(payload: Cow<'a, [u8]>) -> Self {
        Self::new(FrameKind::Headers, payload, None)
    }

    /// Creates a new frame of type [`FrameKind::Settings`].
    ///
    /// # Panics
    ///
    /// Panics if the `payload` size if greater than [`VarInt::MAX`].
    #[inline(always)]
    pub fn new_settings(payload: Cow<'a, [u8]>) -> Self {
        Self::new(FrameKind::Settings, payload, None)
    }

    /// Creates a new frame of type [`FrameKind::WebTransport`].
    #[inline(always)]
    pub fn new_webtransport(session_id: SessionId) -> Self {
        Self::new(
            FrameKind::WebTransport,
            Cow::Owned(Default::default()),
            Some(session_id),
        )
    }

    /// Creates a new frame of type [`FrameKind::Exercise`].
    ///
    /// # Panics
    ///
    /// * Panics if the `payload` size if greater than [`VarInt::MAX`].
    /// * Panics if `id` is not a valid exercise (see [`FrameKind::is_id_exercise`]).
    #[inline(always)]
    pub fn new_exercise(id: VarInt, payload: Cow<'a, [u8]>) -> Self {
        assert!(FrameKind::is_id_exercise(id));
        Self::new(FrameKind::Exercise(id), payload, None)
    }

    /// Reads a [`Frame`] from a [`BytesReader`].
    ///
    /// It returns [`None`] if the `bytes_reader` does not contain enough bytes
    /// to parse an entire frame.
    ///
    /// In case [`None`] or [`Err`], `bytes_reader` might be partially read.
    pub fn read<R>(bytes_reader: &mut R) -> Result<Option<Self>, ParseError>
    where
        R: BytesReader<'a>,
    {
        let kind = match bytes_reader.get_varint() {
            Some(kind_id) => FrameKind::parse(kind_id).ok_or(ParseError::UnknownFrame)?,
            None => return Ok(None),
        };

        if matches!(kind, FrameKind::WebTransport) {
            let session_id = match bytes_reader.get_varint() {
                Some(session_id) => SessionId::try_from_varint(session_id)
                    .map_err(|InvalidSessionId| ParseError::InvalidSessionId)?,
                None => return Ok(None),
            };

            Ok(Some(Self::new_webtransport(session_id)))
        } else {
            let payload_len = match bytes_reader.get_varint() {
                Some(payload_len) => payload_len.into_inner() as usize,
                None => return Ok(None),
            };

            if payload_len > Self::MAX_PARSE_PAYLOAD_ALLOWED {
                return Err(ParseError::PayloadTooBig);
            }

            let payload = match bytes_reader.get_bytes(payload_len) {
                Some(payload) => payload,
                None => return Ok(None),
            };

            Ok(Some(Self::new(kind, Cow::Borrowed(payload), None)))
        }
    }

    /// Reads a [`Frame`] from a `reader`.
    #[cfg(feature = "async")]
    #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
    pub async fn read_async<R>(reader: &mut R) -> Result<Frame<'a>, IoReadError>
    where
        R: AsyncRead + Unpin + ?Sized,
    {
        use crate::bytes::BytesReaderAsync;

        let kind_id = reader.get_varint().await?;
        let kind = FrameKind::parse(kind_id).ok_or(IoReadError::Parse(ParseError::UnknownFrame))?;

        if matches!(kind, FrameKind::WebTransport) {
            let session_id =
                SessionId::try_from_varint(reader.get_varint().await.map_err(|e| match e {
                    bytes::IoReadError::ImmediateFin => bytes::IoReadError::UnexpectedFin,
                    _ => e,
                })?)
                .map_err(|InvalidSessionId| IoReadError::Parse(ParseError::InvalidSessionId))?;

            Ok(Self::new_webtransport(session_id))
        } else {
            let payload_len = reader
                .get_varint()
                .await
                .map_err(|e| match e {
                    bytes::IoReadError::ImmediateFin => bytes::IoReadError::UnexpectedFin,
                    _ => e,
                })?
                .into_inner() as usize;

            if payload_len > Self::MAX_PARSE_PAYLOAD_ALLOWED {
                return Err(IoReadError::Parse(ParseError::PayloadTooBig));
            }

            let mut payload = vec![0; payload_len];

            reader.get_buffer(&mut payload).await.map_err(|e| match e {
                bytes::IoReadError::ImmediateFin => bytes::IoReadError::UnexpectedFin,
                _ => e,
            })?;

            payload.shrink_to_fit();

            Ok(Self::new(kind, Cow::Owned(payload), None))
        }
    }

    /// Reads a [`Frame`] from a [`BufferReader`].
    ///
    /// It returns [`None`] if the `buffer_reader` does not contain enough bytes
    /// to parse an entire frame.
    ///
    /// In case [`None`] or [`Err`], `buffer_reader` offset if not advanced.
    pub fn read_from_buffer(
        buffer_reader: &mut BufferReader<'a>,
    ) -> Result<Option<Self>, ParseError> {
        let mut buffer_reader_child = buffer_reader.child();

        match Self::read(&mut *buffer_reader_child)? {
            Some(frame) => {
                buffer_reader_child.commit();
                Ok(Some(frame))
            }
            None => Ok(None),
        }
    }

    /// Writes a [`Frame`] into a [`BytesWriter`].
    ///
    /// It returns [`Err`] if the `bytes_writer` does not have enough capacity
    /// to write the entire frame.
    /// See [`Self::write_size`] to retrieve the exact amount of required capacity.
    ///
    /// In case [`Err`], `bytes_writer` might be partially written.
    ///
    /// # Panics
    ///
    /// Panics if the payload size if greater than [`VarInt::MAX`].
    pub fn write<W>(&self, bytes_writer: &mut W) -> Result<(), EndOfBuffer>
    where
        W: BytesWriter,
    {
        bytes_writer.put_varint(self.kind.id())?;

        if let Some(session_id) = self.session_id() {
            bytes_writer.put_varint(session_id.into_varint())?;
        } else {
            bytes_writer.put_varint(
                VarInt::try_from(self.payload.len() as u64)
                    .expect("Payload cannot be larger than varint max"),
            )?;
            bytes_writer.put_bytes(&self.payload)?;
        }

        Ok(())
    }

    /// Writes a [`Frame`] into a `writer`.
    ///
    /// # Panics
    ///
    /// Panics if the payload size if greater than [`VarInt::MAX`].
    #[cfg(feature = "async")]
    #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
    pub async fn write_async<W>(&self, writer: &mut W) -> Result<(), IoWriteError>
    where
        W: AsyncWrite + Unpin + ?Sized,
    {
        use crate::bytes::BytesWriterAsync;

        writer.put_varint(self.kind.id()).await?;

        if let Some(session_id) = self.session_id() {
            writer.put_varint(session_id.into_varint()).await?;
        } else {
            writer
                .put_varint(
                    VarInt::try_from(self.payload.len() as u64)
                        .expect("Payload cannot be larger than varint max"),
                )
                .await?;
            writer.put_buffer(&self.payload).await?;
        }

        Ok(())
    }

    /// Writes this [`Frame`] into a buffer via [`BufferWriter`].
    ///
    /// In case [`Err`], `buffer_writer` is not advanced.
    ///
    /// # Panics
    ///
    /// Panics if the payload size if greater than [`VarInt::MAX`].
    pub fn write_to_buffer(&self, buffer_writer: &mut BufferWriter) -> Result<(), EndOfBuffer> {
        if buffer_writer.capacity() < self.write_size() {
            return Err(EndOfBuffer);
        }

        self.write(buffer_writer)
            .expect("Enough capacity for frame");

        Ok(())
    }

    /// Returns the needed capacity to write this frame into a buffer.
    pub fn write_size(&self) -> usize {
        if let Some(session_id) = self.session_id() {
            self.kind.id().size() + session_id.into_varint().size()
        } else {
            self.kind.id().size()
                + VarInt::try_from(self.payload.len() as u64)
                    .expect("Payload cannot be larger than varint max")
                    .size()
                + self.payload.len()
        }
    }

    /// Returns the [`FrameKind`] of this [`Frame`].
    #[inline(always)]
    pub const fn kind(&self) -> FrameKind {
        self.kind
    }

    /// Returns the payload of this [`Frame`].
    #[inline(always)]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Returns the [`SessionId`] if frame is [`FrameKind::WebTransport`],
    /// otherwise returns [`None`].
    #[inline(always)]
    pub fn session_id(&self) -> Option<SessionId> {
        matches!(self.kind, FrameKind::WebTransport).then(|| {
            self.session_id
                .expect("WebTransport frame contains session id")
        })
    }

    /// # Panics
    ///
    /// Panics if the `payload` size if greater than [`VarInt::MAX`].
    fn new(kind: FrameKind, payload: Cow<'a, [u8]>, session_id: Option<SessionId>) -> Self {
        if let FrameKind::Exercise(id) = kind {
            debug_assert!(FrameKind::is_id_exercise(id));
        } else if let FrameKind::WebTransport = kind {
            debug_assert!(payload.is_empty());
            debug_assert!(session_id.is_some());
        }

        assert!(payload.len() <= VarInt::MAX.into_inner() as usize);

        Self {
            kind,
            payload,
            session_id,
        }
    }

    #[cfg(test)]
    pub(crate) fn into_owned<'b>(self) -> Frame<'b> {
        Frame {
            kind: self.kind,
            payload: Cow::Owned(self.payload.into_owned()),
            session_id: self.session_id,
        }
    }

    #[cfg(test)]
    pub(crate) fn serialize_any(kind: VarInt, payload: &[u8]) -> Vec<u8> {
        let mut buffer = Vec::new();

        Self {
            kind: FrameKind::Exercise(kind),
            payload: Cow::Owned(payload.to_vec()),
            session_id: None,
        }
        .write(&mut buffer)
        .unwrap();

        buffer
    }

    #[cfg(test)]
    pub(crate) fn serialize_webtransport(session_id: SessionId) -> Vec<u8> {
        let mut buffer = Vec::new();

        Self {
            kind: FrameKind::WebTransport,
            payload: Cow::Owned(Default::default()),
            session_id: Some(session_id),
        }
        .write(&mut buffer)
        .unwrap();

        buffer
    }
}

mod frame_kind_ids {
    use crate::varint::VarInt;

    pub const DATA: VarInt = VarInt::from_u32(0x00);
    pub const HEADERS: VarInt = VarInt::from_u32(0x01);
    pub const SETTINGS: VarInt = VarInt::from_u32(0x04);
    pub const WEBTRANSPORT_STREAM: VarInt = VarInt::from_u32(0x41);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::Headers;
    use crate::ids::StreamId;
    use crate::settings::Settings;

    #[test]
    fn settings() {
        let settings = Settings::builder()
            .qpack_blocked_streams(VarInt::from_u32(1))
            .qpack_max_table_capacity(VarInt::from_u32(2))
            .enable_h3_datagrams()
            .enable_webtransport()
            .webtransport_max_sessions(VarInt::from_u32(3))
            .build();

        let frame = settings.generate_frame();
        assert!(frame.session_id().is_none());
        assert!(matches!(frame.kind(), FrameKind::Settings));

        let frame = utils::assert_serde(frame);
        Settings::with_frame(&frame).unwrap();
    }

    #[tokio::test]
    async fn settings_async() {
        let settings = Settings::builder()
            .qpack_blocked_streams(VarInt::from_u32(1))
            .qpack_max_table_capacity(VarInt::from_u32(2))
            .enable_h3_datagrams()
            .enable_webtransport()
            .webtransport_max_sessions(VarInt::from_u32(3))
            .build();

        let frame = settings.generate_frame();
        assert!(frame.session_id().is_none());
        assert!(matches!(frame.kind(), FrameKind::Settings));

        let frame = utils::assert_serde_async(frame).await;
        Settings::with_frame(&frame).unwrap();
    }

    #[test]
    fn headers() {
        let stream_id = StreamId::new(VarInt::from_u32(0));
        let headers = Headers::from_iter([("key1", "value1")]);

        let frame = headers.generate_frame(stream_id);
        assert!(frame.session_id().is_none());
        assert!(matches!(frame.kind(), FrameKind::Headers));

        let frame = utils::assert_serde(frame);
        Headers::with_frame(&frame, stream_id).unwrap();
    }

    #[tokio::test]
    async fn headers_async() {
        let stream_id = StreamId::new(VarInt::from_u32(0));
        let headers = Headers::from_iter([("key1", "value1")]);

        let frame = headers.generate_frame(stream_id);
        assert!(frame.session_id().is_none());
        assert!(matches!(frame.kind(), FrameKind::Headers));

        let frame = utils::assert_serde_async(frame).await;
        Headers::with_frame(&frame, stream_id).unwrap();
    }

    #[test]
    fn webtransport() {
        let session_id = SessionId::try_from_varint(VarInt::from_u32(0)).unwrap();
        let frame = Frame::new_webtransport(session_id);

        assert!(frame.payload().is_empty());
        assert!(matches!(frame.session_id(), Some(x) if x == session_id));
        assert!(matches!(frame.kind(), FrameKind::WebTransport));

        let frame = utils::assert_serde(frame);

        assert!(frame.payload().is_empty());
        assert!(matches!(frame.session_id(), Some(x) if x == session_id));
        assert!(matches!(frame.kind(), FrameKind::WebTransport));
    }

    #[tokio::test]
    async fn webtransport_async() {
        let session_id = SessionId::try_from_varint(VarInt::from_u32(0)).unwrap();
        let frame = Frame::new_webtransport(session_id);

        assert!(frame.payload().is_empty());
        assert!(matches!(frame.session_id(), Some(x) if x == session_id));
        assert!(matches!(frame.kind(), FrameKind::WebTransport));

        let frame = utils::assert_serde_async(frame).await;

        assert!(frame.payload().is_empty());
        assert!(matches!(frame.session_id(), Some(x) if x == session_id));
        assert!(matches!(frame.kind(), FrameKind::WebTransport));
    }

    #[test]
    fn read_eof() {
        let buffer = Frame::serialize_any(FrameKind::Data.id(), b"This is a test payload");
        assert!(Frame::read(&mut &buffer[..buffer.len() - 1])
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn read_eof_async() {
        let buffer = Frame::serialize_any(FrameKind::Data.id(), b"This is a test payload");

        for len in 0..buffer.len() {
            let result = Frame::read_async(&mut &buffer[..len]).await;

            match len {
                0 => assert!(matches!(
                    result,
                    Err(IoReadError::IO(bytes::IoReadError::ImmediateFin))
                )),
                _ => assert!(matches!(
                    result,
                    Err(IoReadError::IO(bytes::IoReadError::UnexpectedFin))
                )),
            }
        }
    }

    #[tokio::test]
    async fn read_eof_webtransport_async() {
        let session_id = SessionId::try_from_varint(VarInt::from_u32(0)).unwrap();
        let buffer = Frame::serialize_webtransport(session_id);

        for len in 0..buffer.len() {
            let result = Frame::read_async(&mut &buffer[..len]).await;

            match len {
                0 => assert!(matches!(
                    result,
                    Err(IoReadError::IO(bytes::IoReadError::ImmediateFin))
                )),
                _ => assert!(matches!(
                    result,
                    Err(IoReadError::IO(bytes::IoReadError::UnexpectedFin))
                )),
            }
        }
    }

    #[test]
    fn unknown_frame() {
        let buffer = Frame::serialize_any(VarInt::from_u32(0x0042_4242), b"This is a test payload");

        assert!(matches!(
            Frame::read(&mut buffer.as_slice()),
            Err(ParseError::UnknownFrame)
        ));
    }

    #[tokio::test]
    async fn unknown_frame_async() {
        let buffer = Frame::serialize_any(VarInt::from_u32(0x0042_4242), b"This is a test payload");

        assert!(matches!(
            Frame::read_async(&mut buffer.as_slice()).await,
            Err(IoReadError::Parse(ParseError::UnknownFrame))
        ));
    }

    #[test]
    fn invalid_session_id() {
        let invalid_session_id = SessionId::maybe_invalid(VarInt::from_u32(1));
        let buffer = Frame::serialize_webtransport(invalid_session_id);

        assert!(matches!(
            Frame::read(&mut buffer.as_slice()),
            Err(ParseError::InvalidSessionId)
        ));
    }

    #[tokio::test]
    async fn invalid_session_id_async() {
        let invalid_session_id = SessionId::maybe_invalid(VarInt::from_u32(1));
        let buffer = Frame::serialize_webtransport(invalid_session_id);

        assert!(matches!(
            Frame::read_async(&mut buffer.as_slice()).await,
            Err(IoReadError::Parse(ParseError::InvalidSessionId))
        ));
    }

    #[test]
    fn payload_too_big() {
        let mut buffer = Vec::new();
        buffer.put_varint(FrameKind::Data.id()).unwrap();
        buffer
            .put_varint(VarInt::from_u32(
                Frame::MAX_PARSE_PAYLOAD_ALLOWED as u32 + 1,
            ))
            .unwrap();

        assert!(matches!(
            Frame::read_from_buffer(&mut BufferReader::new(&buffer)),
            Err(ParseError::PayloadTooBig)
        ));
    }

    #[tokio::test]
    async fn payload_too_big_async() {
        let mut buffer = Vec::new();
        buffer.put_varint(FrameKind::Data.id()).unwrap();
        buffer
            .put_varint(VarInt::from_u32(
                Frame::MAX_PARSE_PAYLOAD_ALLOWED as u32 + 1,
            ))
            .unwrap();

        assert!(matches!(
            Frame::read_async(&mut &*buffer).await,
            Err(IoReadError::Parse(ParseError::PayloadTooBig)),
        ));
    }

    mod utils {
        use super::*;

        pub fn assert_serde(frame: Frame) -> Frame {
            let mut buffer = Vec::new();

            frame.write(&mut buffer).unwrap();
            assert_eq!(buffer.len(), frame.write_size());

            let mut buffer = buffer.as_slice();
            let frame = Frame::read(&mut buffer).unwrap().unwrap();
            assert!(buffer.is_empty());

            frame.into_owned()
        }

        #[cfg(feature = "async")]
        pub async fn assert_serde_async(frame: Frame<'_>) -> Frame {
            let mut buffer = Vec::new();

            frame.write_async(&mut buffer).await.unwrap();
            assert_eq!(buffer.len(), frame.write_size());

            let mut buffer = buffer.as_slice();
            let frame = Frame::read_async(&mut buffer).await.unwrap();
            assert!(buffer.is_empty());

            frame.into_owned()
        }
    }
}
