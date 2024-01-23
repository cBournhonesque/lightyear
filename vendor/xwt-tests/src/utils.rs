use xwt_core::prelude::*;

pub async fn connect<Endpoint>(
    endpoint: Endpoint,
    url: &str,
) -> Result<EndpointConnectConnectionFor<Endpoint>, xwt_error::Connect<Endpoint>>
where
    Endpoint: xwt_core::EndpointConnect,
{
    let connecting = endpoint
        .connect(url)
        .await
        .map_err(xwt_error::Connect::Connect)?;

    let connection = connecting
        .wait_connect()
        .await
        .map_err(xwt_error::Connect::Connecting)?;

    Ok(connection)
}

pub async fn ok_accepting<Accepting>(
    accepting: Accepting,
) -> Result<AcceptingConnectionFor<Accepting>, xwt_error::Accepting<Accepting>>
where
    Accepting: xwt_core::Accepting,
    AcceptingConnectionFor<Accepting>: xwt_core::Connection,
{
    let request = accepting
        .wait_accept()
        .await
        .map_err(xwt_error::Accepting::Accepting)?;

    let connection = request
        .ok()
        .await
        .map_err(xwt_error::Accepting::RequestOk)?;

    Ok(connection)
}

pub async fn open_bi<Connection>(
    connection: Connection,
) -> Result<BiStreamsFor<Connection>, xwt_error::OpenBi<Connection>>
where
    Connection: xwt_core::OpenBiStream,
{
    let opening = connection
        .open_bi()
        .await
        .map_err(xwt_error::OpenBi::Open)?;
    let streams = opening
        .wait_bi()
        .await
        .map_err(xwt_error::OpenBi::Opening)?;

    Ok(streams)
}

pub async fn open_uni<Connection>(
    connection: Connection,
) -> Result<SendUniStreamFor<Connection>, xwt_error::OpenUni<Connection>>
where
    Connection: xwt_core::OpenUniStream,
{
    let opening = connection
        .open_uni()
        .await
        .map_err(xwt_error::OpenUni::Open)?;
    let stream = opening
        .wait_uni()
        .await
        .map_err(xwt_error::OpenUni::Opening)?;

    Ok(stream)
}
