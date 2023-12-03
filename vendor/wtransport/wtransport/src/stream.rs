use crate::driver::streams::bilocal::StreamBiLocalQuic;
use crate::driver::streams::unilocal::StreamUniLocalQuic;
use crate::driver::streams::ProtoWriteError;
use crate::driver::streams::QuicRecvStream;
use crate::driver::streams::QuicSendStream;
use crate::error::StreamOpeningError;
use crate::error::StreamReadError;
use crate::error::StreamReadExactError;
use crate::error::StreamWriteError;
use std::future::Future;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use tokio::io::ReadBuf;
use wtransport_proto::ids::SessionId;
use wtransport_proto::ids::StreamId;
use wtransport_proto::stream_header::StreamHeader;
use wtransport_proto::varint::VarInt;

/// A stream that can only be used to send data.
#[derive(Debug)]
pub struct SendStream(QuicSendStream);

impl SendStream {
    #[inline(always)]
    pub(crate) fn new(stream: QuicSendStream) -> Self {
        Self(stream)
    }

    /// Writes bytes to the stream.
    ///
    /// On success, returns the number of bytes written.
    /// Congestion and flow control may cause this to be shorter than `buf.len()`,
    /// indicating that only a prefix of `buf` was written.
    #[inline(always)]
    pub async fn write(&mut self, buf: &[u8]) -> Result<usize, StreamWriteError> {
        self.0.write(buf).await
    }

    /// Convenience method to write an entire buffer to the stream.
    #[inline(always)]
    pub async fn write_all(&mut self, buf: &[u8]) -> Result<(), StreamWriteError> {
        self.0.write_all(buf).await
    }

    /// Shut down the stream gracefully.
    ///
    /// No new data may be written after calling this method. Completes when the peer has
    /// acknowledged all sent data, retransmitting data as needed.
    #[inline(always)]
    pub async fn finish(&mut self) -> Result<(), StreamWriteError> {
        self.0.finish().await
    }

    /// Returns the [`StreamId`] associated.
    #[inline(always)]
    pub fn id(&self) -> StreamId {
        self.0.id()
    }

    /// Sets the priority of the send stream.
    ///
    /// Every send stream has an initial priority of 0. Locally buffered data from streams with
    /// higher priority will be transmitted before data from streams with lower priority. Changing
    /// the priority of a stream with pending data may only take effect after that data has been
    /// transmitted. Using many different priority levels per connection may have a negative
    /// impact on performance.
    #[inline(always)]
    pub fn set_priority(&self, priority: i32) {
        self.0.set_priority(priority);
    }

    /// Gets the priority of the send stream.
    #[inline(always)]
    pub fn priority(&self) -> i32 {
        self.0.priority()
    }

    /// Closes the send stream immediately.
    ///
    /// No new data can be written after calling this method. Locally buffered data is dropped, and
    /// previously transmitted data will no longer be retransmitted if lost. If an attempt has
    /// already been made to finish the stream, the peer may still receive all written data.
    #[inline(always)]
    pub fn reset(self, error_code: VarInt) {
        self.0.reset(error_code);
    }

    /// Awaits for the stream to be stopped by the peer.
    ///
    /// If the stream is stopped the error code will be stored in [`StreamWriteError::Stopped`].
    #[inline(always)]
    pub async fn stopped(mut self) -> StreamWriteError {
        self.0.stopped().await
    }

    /// Returns a reference to the underlying QUIC stream.
    #[cfg(feature = "quinn")]
    #[cfg_attr(docsrs, doc(cfg(feature = "quinn")))]
    #[inline(always)]
    pub fn quic_stream(&self) -> &quinn::SendStream {
        self.0.quic_stream()
    }

    /// Returns a mutable reference to the underlying QUIC stream.
    #[cfg(feature = "quinn")]
    #[cfg_attr(docsrs, doc(cfg(feature = "quinn")))]
    #[inline(always)]
    pub fn quic_stream_mut(&mut self) -> &mut quinn::SendStream {
        self.0.quic_stream_mut()
    }
}

/// A stream that can only be used to receive data.
#[derive(Debug)]
pub struct RecvStream(QuicRecvStream);

impl RecvStream {
    #[inline(always)]
    pub(crate) fn new(stream: QuicRecvStream) -> Self {
        Self(stream)
    }

    /// Read data contiguously from the stream.
    ///
    /// On success, returns the number of bytes read into `buf`.
    #[inline(always)]
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, StreamReadError> {
        self.0.read(buf).await
    }

    /// Reads an exact number of bytes contiguously from the stream.
    ///
    /// If the stream terminates before the entire length has been read, it
    /// returns [`StreamReadExactError::FinishedEarly`].
    pub async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), StreamReadExactError> {
        self.0.read_exact(buf).await
    }

    /// Stops accepting data on the stream.
    ///
    /// Discards unread data and notifies the peer to stop transmitting.
    pub fn stop(mut self, error_code: VarInt) {
        let _ = self.0.stop(error_code);
    }

    /// Returns the [`StreamId`] associated.
    #[inline(always)]
    pub fn id(&self) -> StreamId {
        self.0.id()
    }

    /// Returns a reference to the underlying QUIC stream.
    #[cfg(feature = "quinn")]
    #[cfg_attr(docsrs, doc(cfg(feature = "quinn")))]
    #[inline(always)]
    pub fn quic_stream(&self) -> &quinn::RecvStream {
        self.0.quic_stream()
    }

    /// Returns a mutable reference to the underlying QUIC stream.
    #[cfg(feature = "quinn")]
    #[cfg_attr(docsrs, doc(cfg(feature = "quinn")))]
    #[inline(always)]
    pub fn quic_stream_mut(&mut self) -> &mut quinn::RecvStream {
        self.0.quic_stream_mut()
    }
}

impl tokio::io::AsyncWrite for SendStream {
    #[inline(always)]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        tokio::io::AsyncWrite::poll_write(Pin::new(&mut self.0), cx, buf)
    }

    #[inline(always)]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        tokio::io::AsyncWrite::poll_flush(Pin::new(&mut self.0), cx)
    }

    #[inline(always)]
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
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

    #[inline(always)]
    fn is_write_vectored(&self) -> bool {
        tokio::io::AsyncWrite::is_write_vectored(&self.0)
    }
}

impl tokio::io::AsyncRead for RecvStream {
    #[inline(always)]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        tokio::io::AsyncRead::poll_read(Pin::new(&mut self.0), cx, buf)
    }
}

type DynFutureUniStream = dyn Future<Output = Result<SendStream, StreamOpeningError>> + Send + Sync;

/// [`Future`] for an in-progress opening unidirectional stream.
///
/// See [`Connection::open_uni`](crate::Connection::open_uni).
pub struct OpeningUniStream(Pin<Box<DynFutureUniStream>>);

impl OpeningUniStream {
    pub(crate) fn new(session_id: SessionId, quic_stream: StreamUniLocalQuic) -> Self {
        Self(Box::pin(async move {
            match quic_stream
                .upgrade(StreamHeader::new_webtransport(session_id))
                .await
            {
                Ok(stream) => Ok(SendStream(stream.upgrade().into_stream())),
                Err(ProtoWriteError::NotConnected) => Err(StreamOpeningError::NotConnected),
                Err(ProtoWriteError::Stopped) => Err(StreamOpeningError::Refused),
            }
        }))
    }
}

impl Future for OpeningUniStream {
    type Output = Result<SendStream, StreamOpeningError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Future::poll(self.0.as_mut(), cx)
    }
}

type DynFutureBiStream =
    dyn Future<Output = Result<(SendStream, RecvStream), StreamOpeningError>> + Send + Sync;

/// [`Future`] for an in-progress opening bidirectional stream.
///
/// See [`Connection::open_bi`](crate::Connection::open_bi).
pub struct OpeningBiStream(Pin<Box<DynFutureBiStream>>);

impl OpeningBiStream {
    pub(crate) fn new(session_id: SessionId, quic_stream: StreamBiLocalQuic) -> Self {
        Self(Box::pin(async move {
            match quic_stream.upgrade().upgrade(session_id).await {
                Ok(stream) => {
                    let stream = stream.into_stream();
                    Ok((SendStream::new(stream.0), RecvStream::new(stream.1)))
                }
                Err(ProtoWriteError::NotConnected) => Err(StreamOpeningError::NotConnected),
                Err(ProtoWriteError::Stopped) => Err(StreamOpeningError::Refused),
            }
        }))
    }
}

impl Future for OpeningBiStream {
    type Output = Result<(SendStream, RecvStream), StreamOpeningError>;

    #[inline(always)]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Future::poll(self.0.as_mut(), cx)
    }
}
