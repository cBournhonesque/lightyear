use xwt_core::prelude::*;

#[derive(Debug, thiserror::Error)]
pub enum Error<Endpoint>
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

pub async fn run<Endpoint>(endpoint: Endpoint) -> Result<(), Error<Endpoint>>
where
    Endpoint: xwt_core::EndpointConnect + std::fmt::Debug,
    Endpoint::Connecting: std::fmt::Debug,
    EndpointConnectConnectionFor<Endpoint>: xwt_core::OpenBiStream + std::fmt::Debug,
{
    let connection = crate::utils::connect(endpoint, "https://echo.webtransport.day")
        .await
        .map_err(Error::Connect)?;

    let (mut send_stream, mut recv_stream) = crate::utils::open_bi(connection)
        .await
        .map_err(Error::Open)?;

    let mut to_write = &b"hello"[..];
    loop {
        let written = send_stream.write(to_write).await.map_err(Error::Send)?;
        to_write = &to_write[written..];
        if to_write.is_empty() {
            break;
        }
    }

    let mut read_buf = vec![0u8; 2];

    let Some(read) = recv_stream
        .read(&mut read_buf[..])
        .await
        .map_err(Error::Recv)?
    else {
        return Err(Error::NoResponse);
    };
    read_buf.truncate(read);

    if read_buf != b"he" {
        return Err(Error::BadData(read_buf));
    }

    let Some(read) = recv_stream
        .read(&mut read_buf[..])
        .await
        .map_err(Error::Recv)?
    else {
        return Err(Error::NoResponse);
    };
    read_buf.truncate(read);

    if read_buf != b"ll" {
        return Err(Error::BadData(read_buf));
    }

    let Some(read) = recv_stream
        .read(&mut read_buf[..])
        .await
        .map_err(Error::Recv)?
    else {
        return Err(Error::NoResponse);
    };
    read_buf.truncate(read);

    if read_buf != b"o" {
        return Err(Error::BadData(read_buf));
    }

    Ok(())
}
