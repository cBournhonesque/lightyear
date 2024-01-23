#![cfg(not(target_family = "wasm"))]

use xwt_core::async_trait;

mod types;

pub use self::types::*;

#[async_trait]
impl xwt_core::traits::EndpointConnect for Endpoint<wtransport::endpoint::endpoint_side::Client> {
    type Connecting = xwt_core::utils::dummy::Connecting<Connection>;
    type Error = wtransport::error::ConnectingError;

    async fn connect(&self, url: &str) -> Result<Self::Connecting, Self::Error> {
        let connecting = self.0.connect(url).await.map(Connection)?;
        Ok(xwt_core::utils::dummy::Connecting(connecting))
    }
}

#[async_trait]
impl xwt_core::traits::EndpointAccept for Endpoint<wtransport::endpoint::endpoint_side::Server> {
    type Accepting = IncomingSession;
    type Error = std::convert::Infallible;

    async fn accept(&self) -> Result<Option<Self::Accepting>, Self::Error> {
        let incoming_session = self.0.accept().await;
        let incoming_session = IncomingSession(incoming_session);
        Ok(Some(incoming_session))
    }
}

#[async_trait]
impl xwt_core::traits::Accepting for IncomingSession {
    type Request = SessionRequest;
    type Error = wtransport::error::ConnectionError;

    async fn wait_accept(self) -> Result<Self::Request, Self::Error> {
        self.0.await.map(SessionRequest)
    }
}

#[async_trait]
impl xwt_core::traits::Request for SessionRequest {
    type Connection = Connection;
    type OkError = wtransport::error::ConnectionError;
    type CloseError = std::convert::Infallible;

    async fn ok(self) -> Result<Self::Connection, Self::OkError> {
        self.0.accept().await.map(Connection)
    }

    async fn close(self, status: u16) -> Result<(), Self::CloseError> {
        debug_assert!(
            status == 404,
            "wtransport driver only supports closing requests with 404 status code"
        );
        self.0.not_found().await;
        Ok(())
    }
}

impl xwt_core::traits::Streams for Connection {
    type SendStream = SendStream;
    type RecvStream = RecvStream;
}

fn map_streams(
    streams: (wtransport::SendStream, wtransport::RecvStream),
) -> (SendStream, RecvStream) {
    let (send, recv) = streams;
    (SendStream(send), RecvStream(recv))
}

#[async_trait]
impl xwt_core::traits::OpeningBiStream for OpeningBiStream {
    type Streams = Connection;
    type Error = wtransport::error::StreamOpeningError;

    async fn wait_bi(self) -> Result<xwt_core::traits::BiStreamsFor<Self::Streams>, Self::Error> {
        self.0.await.map(map_streams)
    }
}

#[async_trait]
impl xwt_core::traits::OpenBiStream for Connection {
    type Opening = OpeningBiStream;
    type Error = wtransport::error::ConnectionError;

    async fn open_bi(&self) -> Result<Self::Opening, Self::Error> {
        self.0.open_bi().await.map(OpeningBiStream)
    }
}

#[async_trait]
impl xwt_core::traits::AcceptBiStream for Connection {
    type Error = wtransport::error::ConnectionError;

    async fn accept_bi(&self) -> Result<xwt_core::traits::BiStreamsFor<Self>, Self::Error> {
        self.0.accept_bi().await.map(map_streams)
    }
}

#[async_trait]
impl xwt_core::traits::OpeningUniStream for OpeningUniStream {
    type Streams = Connection;
    type Error = wtransport::error::StreamOpeningError;

    async fn wait_uni(
        self,
    ) -> Result<<Self::Streams as xwt_core::traits::Streams>::SendStream, Self::Error> {
        self.0.await.map(SendStream)
    }
}

#[async_trait]
impl xwt_core::traits::OpenUniStream for Connection {
    type Opening = OpeningUniStream;
    type Error = wtransport::error::ConnectionError;

    async fn open_uni(&self) -> Result<Self::Opening, Self::Error> {
        self.0.open_uni().await.map(OpeningUniStream)
    }
}

#[async_trait]
impl xwt_core::traits::AcceptUniStream for Connection {
    type Error = wtransport::error::ConnectionError;

    async fn accept_uni(&self) -> Result<Self::RecvStream, Self::Error> {
        self.0.accept_uni().await.map(RecvStream)
    }
}

#[async_trait]
impl xwt_core::io::Read for RecvStream {
    type Error = wtransport::error::StreamReadError;

    async fn read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error> {
        self.0.read(buf).await
    }
}

#[async_trait]
impl xwt_core::io::Write for SendStream {
    type Error = wtransport::error::StreamWriteError;

    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.0.write(buf).await
    }
}

#[async_trait]
impl xwt_core::io::WriteChunk<xwt_core::io::chunk::U8> for SendStream {
    type Error = wtransport::error::StreamWriteError;

    async fn write_chunk(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
        self.0.write_all(buf).await
    }
}

#[async_trait]
impl xwt_core::datagram::Receive for Connection {
    type Datagram = Datagram;
    type Error = wtransport::error::ConnectionError;

    async fn receive_datagram(&self) -> Result<Self::Datagram, Self::Error> {
        self.0.receive_datagram().await.map(Datagram)
    }
}

impl AsRef<[u8]> for Datagram {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[async_trait]
impl xwt_core::datagram::Send for Connection {
    type Error = wtransport::error::SendDatagramError;

    async fn send_datagram<D>(&self, payload: D) -> Result<(), Self::Error>
    where
        D: Send + AsRef<[u8]>,
    {
        self.0.send_datagram(payload)
    }
}
