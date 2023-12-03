use crate::driver::utils::streamid_q2w;
use crate::driver::utils::varint_q2w;
use crate::driver::utils::varint_w2q;
use crate::error::StreamReadError;
use crate::error::StreamReadExactError;
use crate::error::StreamWriteError;
use std::pin::Pin;
use std::task::ready;
use std::task::Context;
use std::task::Poll;
use tokio::io::ReadBuf;
use wtransport_proto::frame::Frame;
use wtransport_proto::ids::SessionId;
use wtransport_proto::ids::StreamId;
use wtransport_proto::session::SessionRequest;
use wtransport_proto::stream as stream_proto;
use wtransport_proto::stream::Stream as StreamProto;
use wtransport_proto::stream_header::StreamHeader;
use wtransport_proto::stream_header::StreamKind;
use wtransport_proto::varint::VarInt;

pub type ProtoReadError = wtransport_proto::stream::IoReadError;
pub type ProtoWriteError = wtransport_proto::stream::IoWriteError;

#[derive(Debug)]
pub struct AlreadyStop;

#[derive(Debug)]
pub struct QuicSendStream(quinn::SendStream);

impl QuicSendStream {
    #[inline(always)]
    pub async fn write(&mut self, buf: &[u8]) -> Result<usize, StreamWriteError> {
        let written = self.0.write(buf).await?;
        Ok(written)
    }

    #[inline(always)]
    pub async fn write_all(&mut self, buf: &[u8]) -> Result<(), StreamWriteError> {
        self.0.write_all(buf).await?;
        Ok(())
    }

    #[inline(always)]
    pub async fn finish(&mut self) -> Result<(), StreamWriteError> {
        self.0.finish().await?;
        Ok(())
    }

    #[inline(always)]
    pub fn set_priority(&self, priority: i32) {
        let _ = self.0.set_priority(priority);
    }

    #[inline(always)]
    pub fn priority(&self) -> i32 {
        self.0.priority().expect("Stream has been reset")
    }

    pub async fn stopped(&mut self) -> StreamWriteError {
        match self.0.stopped().await {
            Ok(code) => StreamWriteError::Stopped(varint_q2w(code)),
            Err(quinn::StoppedError::ConnectionLost(_)) => StreamWriteError::NotConnected,
            Err(quinn::StoppedError::UnknownStream) => StreamWriteError::QuicProto,
            Err(quinn::StoppedError::ZeroRttRejected) => StreamWriteError::QuicProto,
        }
    }

    #[inline(always)]
    pub fn reset(mut self, error_code: VarInt) {
        self.0
            .reset(varint_w2q(error_code))
            .expect("Stream has been already reset");
    }

    #[inline(always)]
    pub fn id(&self) -> StreamId {
        streamid_q2w(self.0.id())
    }

    #[cfg(feature = "quinn")]
    #[inline(always)]
    pub fn quic_stream(&self) -> &quinn::SendStream {
        &self.0
    }

    #[cfg(feature = "quinn")]
    #[inline(always)]
    pub fn quic_stream_mut(&mut self) -> &mut quinn::SendStream {
        &mut self.0
    }
}

impl wtransport_proto::bytes::AsyncWrite for QuicSendStream {
    #[inline(always)]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        tokio::io::AsyncWrite::poll_write(Pin::new(&mut self.0), cx, buf)
    }
}

impl tokio::io::AsyncWrite for QuicSendStream {
    #[inline(always)]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        tokio::io::AsyncWrite::poll_write(Pin::new(&mut self.0), cx, buf)
    }

    #[inline(always)]
    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        tokio::io::AsyncWrite::poll_flush(Pin::new(&mut self.0), cx)
    }

    #[inline(always)]
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        tokio::io::AsyncWrite::poll_shutdown(Pin::new(&mut self.0), cx)
    }

    #[inline(always)]
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<Result<usize, std::io::Error>> {
        tokio::io::AsyncWrite::poll_write_vectored(Pin::new(&mut self.0), cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        tokio::io::AsyncWrite::is_write_vectored(&self.0)
    }
}

#[derive(Debug)]
pub struct QuicRecvStream(quinn::RecvStream);

impl QuicRecvStream {
    #[inline(always)]
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, StreamReadError> {
        match self.0.read(buf).await? {
            Some(read) => Ok(Some(read)),
            None => Ok(None),
        }
    }

    #[inline(always)]
    pub async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), StreamReadExactError> {
        self.0
            .read_exact(buf)
            .await
            .map_err(|quic_error| match quic_error {
                quinn::ReadExactError::FinishedEarly => StreamReadExactError::FinishedEarly,
                quinn::ReadExactError::ReadError(read) => StreamReadExactError::Read(read.into()),
            })
    }

    #[inline(always)]
    pub fn stop(&mut self, error_code: VarInt) -> Result<(), AlreadyStop> {
        self.0.stop(varint_w2q(error_code)).map_err(|_| AlreadyStop)
    }

    #[inline(always)]
    pub fn id(&self) -> StreamId {
        streamid_q2w(self.0.id())
    }

    #[cfg(feature = "quinn")]
    #[inline(always)]
    pub fn quic_stream(&self) -> &quinn::RecvStream {
        &self.0
    }

    #[cfg(feature = "quinn")]
    #[inline(always)]
    pub fn quic_stream_mut(&mut self) -> &mut quinn::RecvStream {
        &mut self.0
    }
}

impl wtransport_proto::bytes::AsyncRead for QuicRecvStream {
    #[inline(always)]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut buffer = ReadBuf::new(buf);

        match ready!(tokio::io::AsyncRead::poll_read(
            Pin::new(&mut self.0),
            cx,
            &mut buffer
        )) {
            Ok(()) => Poll::Ready(Ok(buffer.filled().len())),
            Err(io_error) => Poll::Ready(Err(io_error)),
        }
    }
}

impl tokio::io::AsyncRead for QuicRecvStream {
    #[inline(always)]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        tokio::io::AsyncRead::poll_read(Pin::new(&mut self.0), cx, buf)
    }
}

#[derive(Debug)]
pub struct Stream<S, P> {
    stream: S,
    proto: P,
}

pub mod biremote {
    use super::*;

    pub type StreamBiRemoteQuic =
        Stream<(QuicSendStream, QuicRecvStream), stream_proto::biremote::StreamBiRemoteQuic>;

    pub type StreamBiRemoteH3 =
        Stream<(QuicSendStream, QuicRecvStream), stream_proto::biremote::StreamBiRemoteH3>;

    pub type StreamBiRemoteWT =
        Stream<(QuicSendStream, QuicRecvStream), stream_proto::biremote::StreamBiRemoteWT>;

    impl StreamBiRemoteQuic {
        pub async fn accept_bi(quic_connection: &quinn::Connection) -> Option<Self> {
            let stream = quic_connection.accept_bi().await.ok()?;
            Some(Self {
                stream: (QuicSendStream(stream.0), QuicRecvStream(stream.1)),
                proto: StreamProto::accept_bi(),
            })
        }

        pub fn upgrade(self) -> StreamBiRemoteH3 {
            StreamBiRemoteH3 {
                stream: self.stream,
                proto: self.proto.upgrade(),
            }
        }

        #[inline(always)]
        pub fn id(&self) -> StreamId {
            self.stream.0.id()
        }
    }

    impl StreamBiRemoteH3 {
        pub async fn read_frame<'a>(&mut self) -> Result<Frame<'a>, ProtoReadError> {
            self.proto.read_frame_async(&mut self.stream.1).await
        }

        pub fn stop(&mut self, error_code: VarInt) -> Result<(), AlreadyStop> {
            self.stream.1.stop(error_code)
        }

        pub fn upgrade(self, session_id: SessionId) -> StreamBiRemoteWT {
            StreamBiRemoteWT {
                stream: self.stream,
                proto: self.proto.upgrade(session_id),
            }
        }

        pub fn id(&self) -> StreamId {
            self.stream.0.id()
        }

        pub fn into_session(self, session_request: SessionRequest) -> session::StreamSession {
            session::StreamSession {
                stream: self.stream,
                proto: self.proto.into_session(session_request),
            }
        }
    }

    impl StreamBiRemoteWT {
        #[inline(always)]
        pub fn session_id(&self) -> SessionId {
            self.proto.session_id()
        }

        #[inline(always)]
        pub fn id(&self) -> StreamId {
            self.stream.0.id()
        }

        #[inline(always)]
        pub fn into_stream(self) -> (QuicSendStream, QuicRecvStream) {
            self.stream
        }
    }
}

pub mod bilocal {
    use super::*;

    pub type StreamBiLocalQuic =
        Stream<(QuicSendStream, QuicRecvStream), stream_proto::bilocal::StreamBiLocalQuic>;

    pub type StreamBiLocalH3 =
        Stream<(QuicSendStream, QuicRecvStream), stream_proto::bilocal::StreamBiLocalH3>;

    pub type StreamBiLocalWT =
        Stream<(QuicSendStream, QuicRecvStream), stream_proto::bilocal::StreamBiLocalWT>;

    impl StreamBiLocalQuic {
        pub async fn open_bi(quic_connection: &quinn::Connection) -> Option<Self> {
            let stream = quic_connection.open_bi().await.ok()?;
            Some(Self {
                stream: (QuicSendStream(stream.0), QuicRecvStream(stream.1)),
                proto: StreamProto::open_bi(),
            })
        }

        pub fn upgrade(self) -> StreamBiLocalH3 {
            StreamBiLocalH3 {
                stream: self.stream,
                proto: self.proto.upgrade(),
            }
        }
    }

    impl StreamBiLocalH3 {
        pub async fn upgrade(
            mut self,
            session_id: SessionId,
        ) -> Result<StreamBiLocalWT, ProtoWriteError> {
            let proto = self
                .proto
                .upgrade_async(session_id, &mut self.stream.0)
                .await?;

            Ok(StreamBiLocalWT {
                stream: self.stream,
                proto,
            })
        }

        pub fn into_session(self, session_request: SessionRequest) -> session::StreamSession {
            session::StreamSession {
                stream: self.stream,
                proto: self.proto.into_session(session_request),
            }
        }
    }

    impl StreamBiLocalWT {
        pub fn into_stream(self) -> (QuicSendStream, QuicRecvStream) {
            self.stream
        }
    }
}

pub mod uniremote {
    use super::*;

    pub type StreamUniRemoteQuic =
        Stream<QuicRecvStream, stream_proto::uniremote::StreamUniRemoteQuic>;

    pub type StreamUniRemoteH3 = Stream<QuicRecvStream, stream_proto::uniremote::StreamUniRemoteH3>;

    pub type StreamUniRemoteWT = Stream<QuicRecvStream, stream_proto::uniremote::StreamUniRemoteWT>;

    impl StreamUniRemoteQuic {
        pub async fn accept_uni(quic_connection: &quinn::Connection) -> Option<Self> {
            let stream = quic_connection.accept_uni().await.ok()?;
            Some(Self {
                stream: QuicRecvStream(stream),
                proto: StreamProto::accept_uni(),
            })
        }

        pub async fn upgrade(mut self) -> Result<StreamUniRemoteH3, ProtoReadError> {
            let proto = self.proto.upgrade_async(&mut self.stream).await?;
            Ok(StreamUniRemoteH3 {
                stream: self.stream,
                proto,
            })
        }

        #[inline(always)]
        pub fn id(&self) -> StreamId {
            self.stream.id()
        }
    }

    impl StreamUniRemoteH3 {
        pub async fn read_frame<'a>(&mut self) -> Result<Frame<'a>, ProtoReadError> {
            self.proto.read_frame_async(&mut self.stream).await
        }

        pub fn kind(&self) -> StreamKind {
            self.proto.kind()
        }

        pub fn upgrade(self) -> StreamUniRemoteWT {
            StreamUniRemoteWT {
                stream: self.stream,
                proto: self.proto.upgrade(),
            }
        }

        pub fn stream_mut(&mut self) -> &mut QuicRecvStream {
            &mut self.stream
        }
    }

    impl StreamUniRemoteWT {
        #[inline(always)]
        pub fn session_id(&self) -> SessionId {
            self.proto.session_id()
        }

        #[inline(always)]
        pub fn id(&self) -> StreamId {
            self.stream.id()
        }

        #[inline(always)]
        pub fn into_stream(self) -> QuicRecvStream {
            self.stream
        }
    }
}

pub mod unilocal {
    use super::*;

    pub type StreamUniLocalQuic =
        Stream<QuicSendStream, stream_proto::unilocal::StreamUniLocalQuic>;

    pub type StreamUniLocalH3 = Stream<QuicSendStream, stream_proto::unilocal::StreamUniLocalH3>;

    pub type StreamUniLocalWT = Stream<QuicSendStream, stream_proto::unilocal::StreamUniLocalWT>;

    impl StreamUniLocalQuic {
        pub async fn open_uni(quic_connection: &quinn::Connection) -> Option<Self> {
            let stream = quic_connection.open_uni().await.ok()?;
            Some(Self {
                stream: QuicSendStream(stream),
                proto: StreamProto::open_uni(),
            })
        }

        pub async fn upgrade(
            mut self,
            stream_header: StreamHeader,
        ) -> Result<StreamUniLocalH3, ProtoWriteError> {
            let proto = self
                .proto
                .upgrade_async(stream_header, &mut self.stream)
                .await?;

            Ok(StreamUniLocalH3 {
                stream: self.stream,
                proto,
            })
        }
    }

    impl StreamUniLocalH3 {
        pub async fn write_frame<'a>(&mut self, frame: Frame<'a>) -> Result<(), ProtoWriteError> {
            self.proto.write_frame_async(frame, &mut self.stream).await
        }

        pub fn kind(&self) -> StreamKind {
            self.proto.kind()
        }

        pub async fn stopped(&mut self) -> StreamWriteError {
            self.stream.stopped().await
        }

        pub fn upgrade(self) -> StreamUniLocalWT {
            StreamUniLocalWT {
                stream: self.stream,
                proto: self.proto.upgrade(),
            }
        }
    }

    impl StreamUniLocalWT {
        pub fn into_stream(self) -> QuicSendStream {
            self.stream
        }
    }
}

pub mod session {
    use super::*;

    pub type StreamSession =
        Stream<(QuicSendStream, QuicRecvStream), stream_proto::session::StreamSession>;

    impl StreamSession {
        pub async fn read_frame<'a>(&mut self) -> Result<Frame<'a>, ProtoReadError> {
            self.proto.read_frame_async(&mut self.stream.1).await
        }

        pub async fn write_frame<'a>(&mut self, frame: Frame<'a>) -> Result<(), ProtoWriteError> {
            self.proto
                .write_frame_async(frame, &mut self.stream.0)
                .await
        }

        pub fn stop(&mut self, error_code: VarInt) -> Result<(), AlreadyStop> {
            self.stream.1.stop(error_code)
        }

        pub fn id(&self) -> StreamId {
            self.stream.0.id()
        }

        pub fn session_id(&self) -> SessionId {
            SessionId::try_from_session_stream(self.id()).expect("Session stream must be valid")
        }

        pub fn request(&self) -> &SessionRequest {
            self.proto.request()
        }

        pub async fn finish(mut self) {
            let _ = self.stream.0.finish().await;
        }
    }
}

impl From<quinn::WriteError> for StreamWriteError {
    fn from(error: quinn::WriteError) -> Self {
        match error {
            quinn::WriteError::Stopped(code) => StreamWriteError::Stopped(varint_q2w(code)),
            quinn::WriteError::ConnectionLost(_) => StreamWriteError::NotConnected,
            quinn::WriteError::UnknownStream => StreamWriteError::QuicProto,
            quinn::WriteError::ZeroRttRejected => StreamWriteError::QuicProto,
        }
    }
}

impl From<quinn::ReadError> for StreamReadError {
    fn from(error: quinn::ReadError) -> Self {
        match error {
            quinn::ReadError::Reset(code) => StreamReadError::Reset(varint_q2w(code)),
            quinn::ReadError::ConnectionLost(_) => StreamReadError::NotConnected,
            quinn::ReadError::UnknownStream => StreamReadError::QuicProto,
            quinn::ReadError::IllegalOrderedRead => StreamReadError::QuicProto,
            quinn::ReadError::ZeroRttRejected => StreamReadError::QuicProto,
        }
    }
}

pub mod qpack;
pub mod settings;
