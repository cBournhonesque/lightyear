pub mod read_small_buf;

use xwt_core::prelude::*;

#[derive(Debug, thiserror::Error)]
pub enum EchoError<Endpoint>
where
    Endpoint: xwt_core::EndpointConnect + std::fmt::Debug,
    Endpoint::Connecting: std::fmt::Debug,
    EndpointConnectConnectionFor<Endpoint>: xwt_core::OpenBiStream + std::fmt::Debug,
{
    Connect(xwt_error::Connect<Endpoint>),
    Open(xwt_error::OpenBi<EndpointConnectConnectionFor<Endpoint>>),
    Send(WriteErrorFor<SendStreamFor<EndpointConnectConnectionFor<Endpoint>>>),
    Recv(ReadErrorFor<RecvStreamFor<EndpointConnectConnectionFor<Endpoint>>>),
    NoResponse,
    BadData(Vec<u8>),
}

pub async fn echo<Endpoint>(endpoint: Endpoint) -> Result<(), EchoError<Endpoint>>
where
    Endpoint: xwt_core::EndpointConnect + std::fmt::Debug,
    Endpoint::Connecting: std::fmt::Debug,
    EndpointConnectConnectionFor<Endpoint>: xwt_core::OpenBiStream + std::fmt::Debug,
{
    let connection = crate::utils::connect(endpoint, "https://echo.webtransport.day")
        .await
        .map_err(EchoError::Connect)?;

    let (mut send_stream, mut recv_stream) = crate::utils::open_bi(connection)
        .await
        .map_err(EchoError::Open)?;

    let mut to_write = &b"hello"[..];
    loop {
        let written = send_stream.write(to_write).await.map_err(EchoError::Send)?;
        to_write = &to_write[written..];
        if to_write.is_empty() {
            break;
        }
    }

    let mut read_buf = vec![0u8; 1024];

    let Some(read) = recv_stream
        .read(&mut read_buf[..])
        .await
        .map_err(EchoError::Recv)?
    else {
        return Err(EchoError::NoResponse);
    };
    read_buf.truncate(read);

    if read_buf != b"hello" {
        return Err(EchoError::BadData(read_buf));
    }

    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum EchoChunksError<Endpoint, WriteChunk, ReadChunk>
where
    Endpoint: xwt_core::EndpointConnect + std::fmt::Debug,
    Endpoint::Connecting: std::fmt::Debug,
    EndpointConnectConnectionFor<Endpoint>: xwt_core::OpenBiStream + std::fmt::Debug,

    WriteChunk: xwt_core::WriteableChunk + std::fmt::Debug,
    ReadChunk: xwt_core::ReadableChunk + std::fmt::Debug,

    SendStreamFor<EndpointConnectConnectionFor<Endpoint>>: xwt_core::WriteChunk<WriteChunk>,
    RecvStreamFor<EndpointConnectConnectionFor<Endpoint>>: xwt_core::ReadChunk<ReadChunk>,
{
    Connect(xwt_error::Connect<Endpoint>),
    Open(xwt_error::OpenBi<EndpointConnectConnectionFor<Endpoint>>),
    Send(WriteChunkErrorFor<SendStreamFor<EndpointConnectConnectionFor<Endpoint>>, WriteChunk>),
    Recv(ReadChunkErrorFor<RecvStreamFor<EndpointConnectConnectionFor<Endpoint>>, ReadChunk>),
    NoResponse,
    BadData(Vec<u8>),
}

pub async fn echo_chunks<Endpoint, WriteChunk, ReadChunk>(
    endpoint: Endpoint,
) -> Result<(), EchoChunksError<Endpoint, WriteChunk, ReadChunk>>
where
    Endpoint: xwt_core::EndpointConnect + std::fmt::Debug,
    Endpoint::Connecting: std::fmt::Debug,
    EndpointConnectConnectionFor<Endpoint>: xwt_core::OpenBiStream + std::fmt::Debug,

    WriteChunk: xwt_core::WriteableChunk + std::fmt::Debug,
    ReadChunk: xwt_core::ReadableChunk + std::fmt::Debug,

    <WriteChunk as xwt_core::WriteableChunk>::Data<'static>: From<&'static [u8]>,
    for<'a> <ReadChunk as xwt_core::ReadableChunk>::Data<'a>: AsRef<[u8]>,

    SendStreamFor<EndpointConnectConnectionFor<Endpoint>>: xwt_core::WriteChunk<WriteChunk>,
    RecvStreamFor<EndpointConnectConnectionFor<Endpoint>>: xwt_core::ReadChunk<ReadChunk>,
{
    let connection = crate::utils::connect(endpoint, "https://echo.webtransport.day")
        .await
        .map_err(EchoChunksError::Connect)?;

    let (mut send_stream, mut recv_stream) = crate::utils::open_bi(connection)
        .await
        .map_err(EchoChunksError::Open)?;

    let write_data: WriteChunk::Data<'static> = (&b"hello"[..]).into();
    send_stream
        .write_chunk(write_data)
        .await
        .map_err(EchoChunksError::Send)?;

    let maybe_read_chunk = recv_stream
        .read_chunk(1024, false)
        .await
        .map_err(EchoChunksError::Recv)?;
    let Some(read_chunk) = maybe_read_chunk else {
        return Err(EchoChunksError::NoResponse);
    };
    let read_data = read_chunk.data.as_ref();

    if read_data != b"hello" {
        return Err(EchoChunksError::BadData(read_data.into()));
    }

    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum EchoDatagrmsError<Endpoint>
where
    Endpoint: xwt_core::EndpointConnect + std::fmt::Debug,
    Endpoint::Connecting: std::fmt::Debug,
    EndpointConnectConnectionFor<Endpoint>: xwt_core::datagram::Datagrams + std::fmt::Debug,
    ReceiveDatagramFor<EndpointConnectConnectionFor<Endpoint>>: std::fmt::Debug,
{
    Connect(xwt_error::Connect<Endpoint>),
    Send(SendErrorFor<EndpointConnectConnectionFor<Endpoint>>),
    Recv(ReceiveErrorFor<EndpointConnectConnectionFor<Endpoint>>),
    BadData(ReceiveDatagramFor<EndpointConnectConnectionFor<Endpoint>>),
}

pub async fn echo_datagrams<Endpoint>(endpoint: Endpoint) -> Result<(), EchoDatagrmsError<Endpoint>>
where
    Endpoint: xwt_core::EndpointConnect + std::fmt::Debug,
    Endpoint::Connecting: std::fmt::Debug,
    EndpointConnectConnectionFor<Endpoint>: xwt_core::datagram::Datagrams + std::fmt::Debug,
    ReceiveDatagramFor<EndpointConnectConnectionFor<Endpoint>>: std::fmt::Debug,
{
    let connection = crate::utils::connect(endpoint, "https://echo.webtransport.day")
        .await
        .map_err(EchoDatagrmsError::Connect)?;

    connection
        .send_datagram(&b"hello"[..])
        .await
        .map_err(EchoDatagrmsError::Send)?;

    let read = connection
        .receive_datagram()
        .await
        .map_err(EchoDatagrmsError::Recv)?;

    if read.as_ref() != b"hello" {
        return Err(EchoDatagrmsError::BadData(read));
    }

    Ok(())
}
