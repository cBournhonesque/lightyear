use crate::bytes::BufferReader;
use crate::bytes::BufferWriter;
use crate::bytes::BytesReader;
use crate::bytes::BytesWriter;
use crate::bytes::EndOfBuffer;
use crate::error::ErrorCode;
use crate::frame;
use crate::frame::Frame;
use crate::frame::FrameKind;
use crate::ids::SessionId;
use crate::session::SessionRequest;
use crate::stream_header;
use crate::stream_header::StreamHeader;
use crate::stream_header::StreamKind;

#[cfg(feature = "async")]
use crate::bytes::AsyncRead;

#[cfg(feature = "async")]
use crate::bytes::AsyncWrite;

#[cfg(feature = "async")]
use crate::bytes;

/// An error during stream I/O read operation.
#[cfg(feature = "async")]
#[cfg_attr(docsrs, doc(cfg(feature = "async")))]
#[derive(Debug)]
pub enum IoReadError {
    /// Error on HTTP3 protocol.
    H3(ErrorCode),

    /// Error due to I/O operation.
    IO(bytes::IoReadError),
}

/// An error during stream I/O write operation.
#[cfg(feature = "async")]
pub type IoWriteError = bytes::IoWriteError;

/// A QUIC/HTTP3/WebTransport stream.
#[derive(Debug)]
pub struct Stream<K, S> {
    kind: K,
    stage: S,
}

/// Bidirectional remote stream implementations.
pub mod biremote {
    use super::*;
    use types::*;

    /// QUIC bidirectional remote stream.
    pub type StreamBiRemoteQuic = Stream<BiRemote, Quic>;

    /// HTTP3 bidirectional remote stream.
    pub type StreamBiRemoteH3 = Stream<BiRemote, H3>;

    /// WebTransport bidirectional remote stream.
    pub type StreamBiRemoteWT = Stream<BiRemote, WT>;

    impl StreamBiRemoteQuic {
        /// Creates a new remote-initialized bidirectional stream.
        pub fn accept_bi() -> Self {
            Self {
                kind: BiRemote::default(),
                stage: Quic,
            }
        }

        /// Upgrades to an HTTP3 stream.
        pub fn upgrade(self) -> StreamBiRemoteH3 {
            StreamBiRemoteH3 {
                kind: self.kind,
                stage: H3::new(None),
            }
        }
    }

    impl StreamBiRemoteH3 {
        /// See [`Frame::read`].
        pub fn read_frame<'a, R>(
            &mut self,
            bytes_reader: &mut R,
        ) -> Result<Option<Frame<'a>>, ErrorCode>
        where
            R: BytesReader<'a>,
        {
            loop {
                match Frame::read(bytes_reader) {
                    Ok(Some(frame)) => {
                        return Ok(Some(self.validate_frame(frame)?));
                    }
                    Ok(None) => {
                        return Ok(None);
                    }
                    Err(frame::ParseError::UnknownFrame) => {
                        continue;
                    }
                    Err(frame::ParseError::InvalidSessionId) => {
                        return Err(ErrorCode::Id);
                    }
                    Err(frame::ParseError::PayloadTooBig) => {
                        return Err(ErrorCode::ExcessiveLoad);
                    }
                }
            }
        }

        /// See [`Frame::read_async`].
        #[cfg(feature = "async")]
        #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
        pub async fn read_frame_async<'a, R>(
            &mut self,
            reader: &mut R,
        ) -> Result<Frame<'a>, IoReadError>
        where
            R: AsyncRead + Unpin + ?Sized,
        {
            loop {
                match Frame::read_async(reader).await {
                    Ok(frame) => {
                        return self.validate_frame(frame).map_err(IoReadError::H3);
                    }
                    Err(frame::IoReadError::Parse(frame::ParseError::UnknownFrame)) => {
                        continue;
                    }
                    Err(frame::IoReadError::Parse(frame::ParseError::InvalidSessionId)) => {
                        return Err(IoReadError::H3(ErrorCode::Id));
                    }
                    Err(frame::IoReadError::Parse(frame::ParseError::PayloadTooBig)) => {
                        return Err(IoReadError::H3(ErrorCode::ExcessiveLoad));
                    }
                    Err(frame::IoReadError::IO(io_error)) => {
                        if matches!(io_error, bytes::IoReadError::UnexpectedFin) {
                            return Err(IoReadError::H3(ErrorCode::Frame));
                        }

                        return Err(IoReadError::IO(io_error));
                    }
                }
            }
        }

        /// See [`Frame::read_from_buffer`].
        pub fn read_frame_from_buffer<'a>(
            &mut self,
            buffer_reader: &mut BufferReader<'a>,
        ) -> Result<Option<Frame<'a>>, ErrorCode> {
            let mut buffer_reader_child = buffer_reader.child();

            match self.read_frame(&mut *buffer_reader_child)? {
                Some(frame) => {
                    buffer_reader_child.commit();
                    Ok(Some(frame))
                }
                None => Ok(None),
            }
        }

        /// See [`Frame::write`].
        pub fn write_frame<W>(&self, frame: Frame, bytes_writer: &mut W) -> Result<(), EndOfBuffer>
        where
            W: BytesWriter,
        {
            frame.write(bytes_writer)
        }

        /// See [`Frame::write_async`].
        #[cfg(feature = "async")]
        #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
        pub async fn write_frame_async<'a, W>(
            &self,
            frame: Frame<'a>,
            writer: &mut W,
        ) -> Result<(), IoWriteError>
        where
            W: AsyncWrite + Unpin + ?Sized,
        {
            frame.write_async(writer).await
        }

        /// See [`Frame::write_to_buffer`].
        pub fn write_frame_to_buffer(
            &self,
            frame: Frame,
            buffer_writer: &mut BufferWriter,
        ) -> Result<(), EndOfBuffer> {
            frame.write_to_buffer(buffer_writer)
        }

        /// Upgrades to a WebTransport stream.
        ///
        /// **Note**: upgrade should be performed only when [`FrameKind::WebTransport`] is
        /// received as first frame on this HTTP3 stream.
        pub fn upgrade(self, session_id: SessionId) -> StreamBiRemoteWT {
            StreamBiRemoteWT {
                kind: self.kind,
                stage: WT::new(session_id),
            }
        }

        /// Converts the stream into a `StreamSession`.
        pub fn into_session(self, session_request: SessionRequest) -> session::StreamSession {
            session::StreamSession {
                kind: Bi,
                stage: Session::new(session_request),
            }
        }

        fn validate_frame<'a>(&mut self, frame: Frame<'a>) -> Result<Frame<'a>, ErrorCode> {
            let first_frame_done = self.stage.set_first_frame();

            match frame.kind() {
                FrameKind::Data => Ok(frame),
                FrameKind::Headers => Ok(frame),
                FrameKind::Settings => Err(ErrorCode::FrameUnexpected),
                FrameKind::WebTransport => {
                    if !first_frame_done {
                        Ok(frame)
                    } else {
                        Err(ErrorCode::Frame)
                    }
                }
                FrameKind::Exercise(_) => Ok(frame),
            }
        }
    }

    impl StreamBiRemoteWT {
        /// Returns the [`SessionId`] associated with this stream.
        #[inline(always)]
        pub fn session_id(&self) -> SessionId {
            self.stage.session_id()
        }
    }
}

/// Bidirectional local stream implementations.
pub mod bilocal {
    use super::*;
    use types::*;

    /// QUIC bidirectional local stream.
    pub type StreamBiLocalQuic = Stream<BiLocal, Quic>;

    /// HTTP3 bidirectional local stream.
    pub type StreamBiLocalH3 = Stream<BiLocal, H3>;

    /// WebTransport bidirectional local stream.
    pub type StreamBiLocalWT = Stream<BiLocal, WT>;

    impl StreamBiLocalQuic {
        /// Creates a new locally-initialized bidirectional stream.
        pub fn open_bi() -> Self {
            Self {
                kind: BiLocal::default(),
                stage: Quic,
            }
        }

        /// Upgrades to an HTTP3 stream.
        pub fn upgrade(self) -> StreamBiLocalH3 {
            StreamBiLocalH3 {
                kind: self.kind,
                stage: H3::new(None),
            }
        }
    }

    impl StreamBiLocalH3 {
        /// See [`Frame::read`].
        pub fn read_frame<'a, R>(
            &self,
            bytes_reader: &mut R,
        ) -> Result<Option<Frame<'a>>, ErrorCode>
        where
            R: BytesReader<'a>,
        {
            loop {
                match Frame::read(bytes_reader) {
                    Ok(Some(frame)) => {
                        return Ok(Some(self.validate_frame(frame)?));
                    }
                    Ok(None) => {
                        return Ok(None);
                    }
                    Err(frame::ParseError::UnknownFrame) => {
                        continue;
                    }
                    Err(frame::ParseError::InvalidSessionId) => {
                        return Err(ErrorCode::Id);
                    }
                    Err(frame::ParseError::PayloadTooBig) => {
                        return Err(ErrorCode::ExcessiveLoad);
                    }
                }
            }
        }

        /// See [`Frame::read_async`].
        #[cfg(feature = "async")]
        #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
        pub async fn read_frame_async<'a, R>(
            &self,
            reader: &mut R,
        ) -> Result<Frame<'a>, IoReadError>
        where
            R: AsyncRead + Unpin + ?Sized,
        {
            loop {
                match Frame::read_async(reader).await {
                    Ok(frame) => {
                        return self.validate_frame(frame).map_err(IoReadError::H3);
                    }
                    Err(frame::IoReadError::Parse(frame::ParseError::UnknownFrame)) => {
                        continue;
                    }
                    Err(frame::IoReadError::Parse(frame::ParseError::InvalidSessionId)) => {
                        return Err(IoReadError::H3(ErrorCode::Id));
                    }
                    Err(frame::IoReadError::Parse(frame::ParseError::PayloadTooBig)) => {
                        return Err(IoReadError::H3(ErrorCode::ExcessiveLoad));
                    }
                    Err(frame::IoReadError::IO(io_error)) => {
                        if matches!(io_error, bytes::IoReadError::UnexpectedFin) {
                            return Err(IoReadError::H3(ErrorCode::Frame));
                        }

                        return Err(IoReadError::IO(io_error));
                    }
                }
            }
        }

        /// See [`Frame::read_from_buffer`].
        pub fn read_frame_from_buffer<'a>(
            &self,
            buffer_reader: &mut BufferReader<'a>,
        ) -> Result<Option<Frame<'a>>, ErrorCode> {
            let mut buffer_reader_child = buffer_reader.child();

            match self.read_frame(&mut *buffer_reader_child)? {
                Some(frame) => {
                    buffer_reader_child.commit();
                    Ok(Some(frame))
                }
                None => Ok(None),
            }
        }

        /// See [`Frame::write`].
        ///
        /// # Panics
        ///
        /// Panics if [`FrameKind::WebTransport`] (use `upgrade` for that).
        pub fn write_frame<W>(
            &mut self,
            frame: Frame,
            bytes_writer: &mut W,
        ) -> Result<(), EndOfBuffer>
        where
            W: BytesWriter,
        {
            assert!(!matches!(frame.kind(), FrameKind::WebTransport));
            frame.write(bytes_writer)?;
            self.stage.set_first_frame();
            Ok(())
        }

        /// See [`Frame::write_async`].
        ///
        /// # Panics
        ///
        /// Panics if [`FrameKind::WebTransport`] (use `upgrade` for that).
        #[cfg(feature = "async")]
        #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
        pub async fn write_frame_async<'a, W>(
            &mut self,
            frame: Frame<'a>,
            writer: &mut W,
        ) -> Result<(), IoWriteError>
        where
            W: AsyncWrite + Unpin + ?Sized,
        {
            assert!(!matches!(frame.kind(), FrameKind::WebTransport));
            frame.write_async(writer).await?;
            self.stage.set_first_frame();
            Ok(())
        }

        /// See [`Frame::write_to_buffer`].
        ///
        /// # Panics
        ///
        /// Panics if [`FrameKind::WebTransport`] (use `upgrade` for that).
        pub fn write_frame_to_buffer(
            &mut self,
            frame: Frame,
            buffer_writer: &mut BufferWriter,
        ) -> Result<(), EndOfBuffer> {
            assert!(!matches!(frame.kind(), FrameKind::WebTransport));
            frame.write_to_buffer(buffer_writer)?;
            self.stage.set_first_frame();
            Ok(())
        }

        /// Upgrades to a WebTransport stream.
        ///
        /// # Panics
        ///
        /// * Panics if any other I/O operation has been performed on this stream before upgrade.
        /// * Panics if `bytes_writer` does not have enough capacity. See [`Self::upgrade_size`].
        pub fn upgrade<W>(mut self, session_id: SessionId, bytes_writer: &mut W) -> StreamBiLocalWT
        where
            W: BytesWriter,
        {
            assert!(!self.stage.set_first_frame());

            Frame::new_webtransport(session_id)
                .write(bytes_writer)
                .expect("Upgrade failed because buffer too short");

            StreamBiLocalWT {
                kind: self.kind,
                stage: WT::new(session_id),
            }
        }

        /// Upgrades to a WebTransport stream.
        ///
        /// # Panics
        ///
        /// * Panics if any other I/O operation has been performed on this stream before upgrade.
        #[cfg(feature = "async")]
        #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
        pub async fn upgrade_async<W>(
            mut self,
            session_id: SessionId,
            writer: &mut W,
        ) -> Result<StreamBiLocalWT, IoWriteError>
        where
            W: AsyncWrite + Unpin + ?Sized,
        {
            assert!(!self.stage.set_first_frame());

            Frame::new_webtransport(session_id)
                .write_async(writer)
                .await?;

            Ok(StreamBiLocalWT {
                kind: self.kind,
                stage: WT::new(session_id),
            })
        }

        /// Returns the needed capacity for upgrade via [`Self::upgrade`].
        pub fn upgrade_size(&self, session_id: SessionId) -> usize {
            Frame::new_webtransport(session_id).write_size()
        }

        /// Converts the stream into a `StreamSession`.
        pub fn into_session(self, session_request: SessionRequest) -> session::StreamSession {
            session::StreamSession {
                kind: Bi,
                stage: Session::new(session_request),
            }
        }

        fn validate_frame<'a>(&self, frame: Frame<'a>) -> Result<Frame<'a>, ErrorCode> {
            match frame.kind() {
                FrameKind::Data => Ok(frame),
                FrameKind::Headers => Ok(frame),
                FrameKind::Settings => Err(ErrorCode::FrameUnexpected),
                FrameKind::WebTransport => Err(ErrorCode::FrameUnexpected),
                FrameKind::Exercise(_) => Ok(frame),
            }
        }
    }

    impl StreamBiLocalWT {
        /// Returns the [`SessionId`] associated with this stream.
        #[inline(always)]
        pub fn session_id(&self) -> SessionId {
            self.stage.session_id()
        }
    }
}

/// unidirectional remote stream implementations.
pub mod uniremote {
    use super::*;
    use types::*;

    /// A result of attempt to upgrade a `UniRemote` stream.
    pub enum MaybeUpgradeH3 {
        /// Stream cannot be upgraded. Not enough data.
        Quic(StreamUniRemoteQuic),

        /// Stream upgraded to HTTP3.
        H3(StreamUniRemoteH3),
    }

    /// QUIC unidirectional remote stream.
    pub type StreamUniRemoteQuic = Stream<UniRemote, Quic>;

    /// HTTP3 unidirectional remote stream.
    pub type StreamUniRemoteH3 = Stream<UniRemote, H3>;

    /// WebTransport unidirectional remote stream.
    pub type StreamUniRemoteWT = Stream<UniRemote, WT>;

    impl StreamUniRemoteQuic {
        /// Creates a new remote-initialized unidirectional stream.
        pub fn accept_uni() -> Self {
            Self {
                kind: UniRemote::default(),
                stage: Quic,
            }
        }

        /// Upgrades to an HTTP3 stream.
        ///
        /// Because `bytes_reader` could not contain all required data, this behaves more like
        /// an attempt of upgrading.
        ///
        /// In case there are no enough information, [`MaybeUpgradeH3::Quic`] (i.e, `self`)
        /// will be returned.
        ///
        /// If the stream type is unknown [`ErrorCode::StreamCreation`] is returned.
        /// In that case, MUST NOT consider unknown stream types to be a connection error of any kind.
        pub fn upgrade<'a, R>(self, bytes_reader: &mut R) -> Result<MaybeUpgradeH3, ErrorCode>
        where
            R: BytesReader<'a>,
        {
            match StreamHeader::read(bytes_reader) {
                Ok(Some(stream_header)) => Ok(MaybeUpgradeH3::H3(StreamUniRemoteH3 {
                    kind: self.kind,
                    stage: H3::new(Some(stream_header)),
                })),
                Ok(None) => Ok(MaybeUpgradeH3::Quic(self)),
                Err(stream_header::ParseError::UnknownStream) => Err(ErrorCode::StreamCreation),
                Err(stream_header::ParseError::InvalidSessionId) => Err(ErrorCode::Id),
            }
        }

        /// Upgrades to an HTTP3 stream.
        #[cfg(feature = "async")]
        #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
        pub async fn upgrade_async<R>(
            self,
            reader: &mut R,
        ) -> Result<StreamUniRemoteH3, IoReadError>
        where
            R: AsyncRead + Unpin + ?Sized,
        {
            match StreamHeader::read_async(reader).await {
                Ok(stream_header) => Ok(StreamUniRemoteH3 {
                    kind: self.kind,
                    stage: H3::new(Some(stream_header)),
                }),

                Err(stream_header::IoReadError::Parse(
                    stream_header::ParseError::UnknownStream,
                )) => Err(IoReadError::H3(ErrorCode::StreamCreation)),

                Err(stream_header::IoReadError::Parse(
                    stream_header::ParseError::InvalidSessionId,
                )) => Err(IoReadError::H3(ErrorCode::Id)),

                Err(stream_header::IoReadError::IO(io_error)) => {
                    if matches!(io_error, bytes::IoReadError::UnexpectedFin) {
                        // TODO(bfesta): Check if this scenario use Frame code error
                        Err(IoReadError::H3(ErrorCode::Frame))
                    } else {
                        Err(IoReadError::IO(io_error))
                    }
                }
            }
        }
    }

    impl StreamUniRemoteH3 {
        /// See [`Frame::read`].
        ///
        /// # Panics
        ///
        /// Panics if the stream kind is [`StreamKind::WebTransport`]. In that case, use `upgrade` method.
        pub fn read_frame<'a, R>(
            &mut self,
            bytes_reader: &mut R,
        ) -> Result<Option<Frame<'a>>, ErrorCode>
        where
            R: BytesReader<'a>,
        {
            assert!(!matches!(self.kind(), StreamKind::WebTransport));

            loop {
                match Frame::read(bytes_reader) {
                    Ok(Some(frame)) => {
                        return Ok(Some(self.validate_frame(frame)?));
                    }
                    Ok(None) => {
                        return Ok(None);
                    }
                    Err(frame::ParseError::UnknownFrame) => {
                        continue;
                    }
                    Err(frame::ParseError::InvalidSessionId) => {
                        return Err(ErrorCode::Id);
                    }
                    Err(frame::ParseError::PayloadTooBig) => {
                        return Err(ErrorCode::ExcessiveLoad);
                    }
                }
            }
        }

        /// See [`Frame::read_async`].
        ///
        /// # Panics
        ///
        /// Panics if the stream kind is [`StreamKind::WebTransport`]. In that case, use `upgrade` method.
        #[cfg(feature = "async")]
        #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
        pub async fn read_frame_async<'a, R>(
            &mut self,
            reader: &mut R,
        ) -> Result<Frame<'a>, IoReadError>
        where
            R: AsyncRead + Unpin + ?Sized,
        {
            assert!(!matches!(self.kind(), StreamKind::WebTransport));

            loop {
                match Frame::read_async(reader).await {
                    Ok(frame) => {
                        return self.validate_frame(frame).map_err(IoReadError::H3);
                    }
                    Err(frame::IoReadError::Parse(frame::ParseError::UnknownFrame)) => {
                        continue;
                    }
                    Err(frame::IoReadError::Parse(frame::ParseError::InvalidSessionId)) => {
                        return Err(IoReadError::H3(ErrorCode::Id));
                    }
                    Err(frame::IoReadError::Parse(frame::ParseError::PayloadTooBig)) => {
                        return Err(IoReadError::H3(ErrorCode::ExcessiveLoad));
                    }
                    Err(frame::IoReadError::IO(io_error)) => {
                        if matches!(io_error, bytes::IoReadError::UnexpectedFin) {
                            return Err(IoReadError::H3(ErrorCode::Frame));
                        }

                        return Err(IoReadError::IO(io_error));
                    }
                }
            }
        }

        /// See [`Frame::read_from_buffer`].
        ///
        /// # Panics
        ///
        /// Panics if the stream kind is [`StreamKind::WebTransport`]. In that case, use `upgrade` method.
        pub fn read_frame_from_buffer<'a>(
            &mut self,
            buffer_reader: &mut BufferReader<'a>,
        ) -> Result<Option<Frame<'a>>, ErrorCode> {
            let mut buffer_reader_child = buffer_reader.child();

            match self.read_frame(&mut *buffer_reader_child)? {
                Some(frame) => {
                    buffer_reader_child.commit();
                    Ok(Some(frame))
                }
                None => Ok(None),
            }
        }

        /// Upgrades to a WebTransport stream.
        ///
        /// # Panics
        ///
        /// Panics if the stream kind is not [`StreamKind::WebTransport`].
        pub fn upgrade(self) -> StreamUniRemoteWT {
            assert!(matches!(self.kind(), StreamKind::WebTransport));

            StreamUniRemoteWT {
                kind: self.kind,
                stage: WT::new(
                    self.stage
                        .stream_header()
                        .expect("Unistream has header")
                        .session_id()
                        .expect("WebTransport type has session id"),
                ),
            }
        }

        /// Returns the [`StreamKind`] associated with the stream.
        pub fn kind(&self) -> StreamKind {
            self.stage
                .stream_header()
                .expect("Unistream has header")
                .kind()
        }

        /// Returns the [`SessionId`] if stream is [`StreamKind::WebTransport`],
        /// otherwise returns [`None`].
        pub fn session_id(&self) -> Option<SessionId> {
            self.stage
                .stream_header()
                .expect("Unistream has header")
                .session_id()
        }

        fn validate_frame<'a>(&mut self, frame: Frame<'a>) -> Result<Frame<'a>, ErrorCode> {
            match frame.kind() {
                FrameKind::Data => Err(ErrorCode::FrameUnexpected),
                FrameKind::Headers => Err(ErrorCode::FrameUnexpected),
                FrameKind::Settings => Ok(frame),
                FrameKind::WebTransport => Err(ErrorCode::FrameUnexpected),
                FrameKind::Exercise(_) => Ok(frame),
            }
        }
    }

    impl StreamUniRemoteWT {
        /// Returns the [`SessionId`] associated with this stream.
        #[inline(always)]
        pub fn session_id(&self) -> SessionId {
            self.stage.session_id()
        }
    }
}

/// Unidirectional local stream implementations.
pub mod unilocal {
    use super::*;
    use types::*;

    /// QUIC unidirectional remote stream.
    pub type StreamUniLocalQuic = Stream<UniLocal, Quic>;

    /// HTTP3 unidirectional remote stream.
    pub type StreamUniLocalH3 = Stream<UniLocal, H3>;

    /// WebTransport unidirectional remote stream.
    pub type StreamUniLocalWT = Stream<UniLocal, WT>;

    impl StreamUniLocalQuic {
        /// Creates a new locally-initialized unidirectional stream.
        pub fn open_uni() -> Self {
            Self {
                kind: UniLocal::default(),
                stage: Quic,
            }
        }

        /// Upgrades to an HTTP3 stream.
        ///
        /// # Panics
        ///
        /// Panics if `bytes_writer` does not have enough capacity to write
        /// the `stream_header`.
        /// Check it with [`Self::upgrade_size`].
        pub fn upgrade<W>(
            self,
            stream_header: StreamHeader,
            bytes_writer: &mut W,
        ) -> StreamUniLocalH3
        where
            W: BytesWriter,
        {
            stream_header
                .write(bytes_writer)
                .expect("Upgrade failed because buffer too short");

            StreamUniLocalH3 {
                kind: self.kind,
                stage: H3::new(Some(stream_header)),
            }
        }

        /// Upgrades to an HTTP3 stream.
        #[cfg(feature = "async")]
        #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
        pub async fn upgrade_async<W>(
            self,
            stream_header: StreamHeader,
            writer: &mut W,
        ) -> Result<StreamUniLocalH3, IoWriteError>
        where
            W: AsyncWrite + Unpin + ?Sized,
        {
            stream_header.write_async(writer).await?;

            Ok(StreamUniLocalH3 {
                kind: self.kind,
                stage: H3::new(Some(stream_header)),
            })
        }

        /// Returns the buffer capacity needed for [`Self::upgrade`].
        pub fn upgrade_size(stream_header: StreamHeader) -> usize {
            stream_header.write_size()
        }
    }

    impl StreamUniLocalH3 {
        /// See [`Frame::write`].
        ///
        /// # Panics
        ///
        /// Panics if the stream kind is [`StreamKind::WebTransport`]. In that case, use `upgrade` method.
        pub fn write_frame<W>(
            &mut self,
            frame: Frame,
            bytes_writer: &mut W,
        ) -> Result<(), EndOfBuffer>
        where
            W: BytesWriter,
        {
            assert!(!matches!(self.kind(), StreamKind::WebTransport));
            frame.write(bytes_writer)?;
            Ok(())
        }

        /// See [`Frame::write_async`].
        ///
        /// # Panics
        ///
        /// Panics if the stream kind is [`StreamKind::WebTransport`]. In that case, use `upgrade` method.
        #[cfg(feature = "async")]
        #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
        pub async fn write_frame_async<'a, W>(
            &mut self,
            frame: Frame<'a>,
            writer: &mut W,
        ) -> Result<(), IoWriteError>
        where
            W: AsyncWrite + Unpin + ?Sized,
        {
            assert!(!matches!(self.kind(), StreamKind::WebTransport));
            frame.write_async(writer).await?;
            Ok(())
        }

        /// See [`Frame::write_to_buffer`].
        ///
        /// # Panics
        ///
        /// Panics if the stream kind is [`StreamKind::WebTransport`]. In that case, use `upgrade` method.
        pub fn write_frame_to_buffer(
            &mut self,
            frame: Frame,
            buffer_writer: &mut BufferWriter,
        ) -> Result<(), EndOfBuffer> {
            assert!(!matches!(self.kind(), StreamKind::WebTransport));
            frame.write_to_buffer(buffer_writer)?;
            Ok(())
        }

        /// Upgrades to a WebTransport stream.
        ///
        /// # Panics
        ///
        /// Panics if the stream kind is not [`StreamKind::WebTransport`].
        pub fn upgrade(self) -> StreamUniLocalWT {
            assert!(matches!(self.kind(), StreamKind::WebTransport));

            StreamUniLocalWT {
                kind: self.kind,
                stage: WT::new(
                    self.stage
                        .stream_header()
                        .expect("Unistream has header")
                        .session_id()
                        .expect("WebTransport type has session id"),
                ),
            }
        }

        /// Returns the [`StreamKind`] associated with the stream.
        pub fn kind(&self) -> StreamKind {
            self.stage
                .stream_header()
                .expect("Unistream has header")
                .kind()
        }

        /// Returns the [`SessionId`] if stream is [`StreamKind::WebTransport`],
        /// otherwise returns [`None`].
        pub fn session_id(&self) -> Option<SessionId> {
            self.stage
                .stream_header()
                .expect("Unistream has header")
                .session_id()
        }
    }

    impl StreamUniLocalWT {
        /// Returns the [`SessionId`] associated with this stream.
        #[inline(always)]
        pub fn session_id(&self) -> SessionId {
            self.stage.session_id()
        }
    }
}

/// Bidirectional local/remote stream implementations.
///
/// For WebTransport session request/response.
pub mod session {
    use super::*;
    use types::*;

    /// HTTP3 bidirectional stream carrying CONNECT request and response.
    pub type StreamSession = Stream<Bi, Session>;

    impl StreamSession {
        /// See [`Frame::read`].
        pub fn read_frame<'a, R>(
            &self,
            bytes_reader: &mut R,
        ) -> Result<Option<Frame<'a>>, ErrorCode>
        where
            R: BytesReader<'a>,
        {
            loop {
                match Frame::read(bytes_reader) {
                    Ok(Some(frame)) => {
                        return Ok(Some(self.validate_frame(frame)?));
                    }
                    Ok(None) => {
                        return Ok(None);
                    }
                    Err(frame::ParseError::UnknownFrame) => {
                        continue;
                    }
                    Err(frame::ParseError::InvalidSessionId) => {
                        return Err(ErrorCode::Id);
                    }
                    Err(frame::ParseError::PayloadTooBig) => {
                        return Err(ErrorCode::ExcessiveLoad);
                    }
                }
            }
        }

        /// See [`Frame::read_async`].
        #[cfg(feature = "async")]
        #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
        pub async fn read_frame_async<'a, R>(
            &self,
            reader: &mut R,
        ) -> Result<Frame<'a>, IoReadError>
        where
            R: AsyncRead + Unpin + ?Sized,
        {
            loop {
                match Frame::read_async(reader).await {
                    Ok(frame) => {
                        return self.validate_frame(frame).map_err(IoReadError::H3);
                    }
                    Err(frame::IoReadError::Parse(frame::ParseError::UnknownFrame)) => {
                        continue;
                    }
                    Err(frame::IoReadError::Parse(frame::ParseError::InvalidSessionId)) => {
                        return Err(IoReadError::H3(ErrorCode::Id));
                    }
                    Err(frame::IoReadError::Parse(frame::ParseError::PayloadTooBig)) => {
                        return Err(IoReadError::H3(ErrorCode::ExcessiveLoad));
                    }
                    Err(frame::IoReadError::IO(io_error)) => {
                        if matches!(io_error, bytes::IoReadError::UnexpectedFin) {
                            return Err(IoReadError::H3(ErrorCode::Frame));
                        }

                        return Err(IoReadError::IO(io_error));
                    }
                }
            }
        }

        /// See [`Frame::read_from_buffer`].
        pub fn read_frame_from_buffer<'a>(
            &self,
            buffer_reader: &mut BufferReader<'a>,
        ) -> Result<Option<Frame<'a>>, ErrorCode> {
            let mut buffer_reader_child = buffer_reader.child();

            match self.read_frame(&mut *buffer_reader_child)? {
                Some(frame) => {
                    buffer_reader_child.commit();
                    Ok(Some(frame))
                }
                None => Ok(None),
            }
        }

        /// See [`Frame::write`].
        pub fn write_frame<W>(&self, frame: Frame, bytes_writer: &mut W) -> Result<(), EndOfBuffer>
        where
            W: BytesWriter,
        {
            frame.write(bytes_writer)
        }

        /// See [`Frame::write_async`].
        #[cfg(feature = "async")]
        #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
        pub async fn write_frame_async<'a, W>(
            &self,
            frame: Frame<'a>,
            writer: &mut W,
        ) -> Result<(), IoWriteError>
        where
            W: AsyncWrite + Unpin + ?Sized,
        {
            frame.write_async(writer).await
        }

        /// See [`Frame::write_to_buffer`].
        pub fn write_frame_to_buffer(
            &self,
            frame: Frame,
            buffer_writer: &mut BufferWriter,
        ) -> Result<(), EndOfBuffer> {
            frame.write_to_buffer(buffer_writer)
        }

        /// Returns the [`SessionRequest`] associated.
        #[inline(always)]
        pub fn request(&self) -> &SessionRequest {
            self.stage.request()
        }

        fn validate_frame<'a>(&self, frame: Frame<'a>) -> Result<Frame<'a>, ErrorCode> {
            match frame.kind() {
                FrameKind::Data => Ok(frame),
                FrameKind::Headers => Ok(frame),
                FrameKind::Settings => Err(ErrorCode::FrameUnexpected),
                FrameKind::WebTransport => Err(ErrorCode::FrameUnexpected),
                FrameKind::Exercise(_) => Ok(frame),
            }
        }
    }
}

/// Types and states of a stream.
pub mod types {
    use super::*;

    /// QUIC stream type.
    pub struct Quic;

    /// HTTP3 stream type.
    pub struct H3 {
        stream_header: Option<StreamHeader>,
        first_frame_done: bool,
    }

    impl H3 {
        #[inline(always)]
        pub(super) fn new(stream_header: Option<StreamHeader>) -> Self {
            Self {
                stream_header,
                first_frame_done: false,
            }
        }

        /// Sets the first frame to done.
        ///
        /// Returns the previous value (false it this is the first frame).
        #[inline(always)]
        pub(super) fn set_first_frame(&mut self) -> bool {
            std::mem::replace(&mut self.first_frame_done, true)
        }

        #[inline(always)]
        pub(super) fn stream_header(&self) -> Option<&StreamHeader> {
            self.stream_header.as_ref()
        }
    }

    /// WebTransport stream type.
    pub struct WT {
        session_id: SessionId,
    }

    impl WT {
        #[inline(always)]
        pub(super) fn new(session_id: SessionId) -> Self {
            Self { session_id }
        }

        #[inline(always)]
        pub(super) fn session_id(&self) -> SessionId {
            self.session_id
        }
    }

    /// Session (HTTP3-CONNECT) stream type.
    #[derive(Debug)]
    pub struct Session {
        session_request: SessionRequest,
    }

    impl Session {
        #[inline(always)]
        pub(super) fn new(session_request: SessionRequest) -> Self {
            Self { session_request }
        }

        #[inline(always)]
        pub(super) fn request(&self) -> &SessionRequest {
            &self.session_request
        }
    }

    /// Bidirectional stream type.
    #[derive(Debug)]
    pub struct Bi;

    /// Unidirectional stream type.
    #[derive(Debug)]
    pub struct Uni;

    /// Remote-initialized stream type.
    #[derive(Debug)]
    pub struct Remote;

    /// Local-initialized stream type.
    #[derive(Debug)]
    pub struct Local;

    /// Remote-initialized bi-directional stream type.
    #[derive(Debug)]
    pub struct BiRemote(Bi, Remote);

    impl Default for BiRemote {
        #[inline(always)]
        fn default() -> Self {
            Self(Bi, Remote)
        }
    }

    /// Local-initialized bi-directional stream type.
    pub struct BiLocal(Bi, Local);

    impl Default for BiLocal {
        #[inline(always)]
        fn default() -> Self {
            Self(Bi, Local)
        }
    }

    /// Remote-initialized uni-directional stream type.
    pub struct UniRemote(Uni, Remote);

    impl Default for UniRemote {
        #[inline(always)]
        fn default() -> Self {
            Self(Uni, Remote)
        }
    }

    /// Local-initialized uni-directional stream type.
    pub struct UniLocal(Uni, Local);

    impl Default for UniLocal {
        #[inline(always)]
        fn default() -> Self {
            Self(Uni, Local)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::varint::VarInt;
    use std::borrow::Cow;

    #[test]
    fn bi_remote_webtransport() {
        let mut buffer = Vec::new();
        Frame::new_webtransport(SessionId::maybe_invalid(VarInt::from_u32(0)))
            .write(&mut buffer)
            .unwrap();

        let mut buffer_reader = BufferReader::new(buffer.as_slice());
        let mut stream = Stream::accept_bi().upgrade();
        let frame = stream
            .read_frame_from_buffer(&mut buffer_reader)
            .unwrap()
            .unwrap();

        let stream = stream.upgrade(frame.session_id().unwrap());

        assert_eq!(
            stream.session_id(),
            SessionId::maybe_invalid(VarInt::from_u32(0))
        );
    }

    #[test]
    fn bi_remote_webtransport_not_first() {
        let mut buffer = Vec::new();
        Frame::new_exercise(VarInt::from_u32(0x21), Cow::Borrowed(b"Payload"))
            .write(&mut buffer)
            .unwrap();
        Frame::new_webtransport(SessionId::maybe_invalid(VarInt::from_u32(0)))
            .write(&mut buffer)
            .unwrap();

        let mut buffer_reader = BufferReader::new(buffer.as_slice());
        let mut stream = Stream::accept_bi().upgrade();
        let frame = stream
            .read_frame_from_buffer(&mut buffer_reader)
            .unwrap()
            .unwrap();

        assert!(matches!(frame.kind(), FrameKind::Exercise(_)));

        let frame = stream.read_frame_from_buffer(&mut buffer_reader);

        assert!(matches!(frame, Err(ErrorCode::Frame)));
    }
}
