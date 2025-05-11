use lightyear_link::{RecvPayload, SendPayload};
use {
    super::{ServerError, SessionResponse, ToConnected, ToConnecting, ToOpen},
    crate::{
        session::{SessionBackend, SessionError, SessionMeta},
        WebTransportRuntime,
    },
    aeronet_io::{connection::Disconnected, packet::RecvPacket, server::Closed},
    bevy_ecs::prelude::*,
    bytes::Bytes,
    futures::{
        channel::{mpsc, oneshot},
        never::Never,
        SinkExt,
    },
    tracing::{debug, debug_span, Instrument},
    wtransport::{
        endpoint::{IncomingSession, SessionRequest}, Endpoint,
        ServerConfig,
    },
    xwt_core::prelude::*,
};

pub async fn start(
    config: ServerConfig,
    send_next: oneshot::Sender<ToOpen>,
) -> Result<Never, Closed> {
    debug!("Spawning backend task to open server");

    let endpoint = Endpoint::server(config).map_err(SessionError::CreateEndpoint)?;
    debug!("Created endpoint");

    let (send_connecting, recv_connecting) = mpsc::channel(1);

    let local_addr = endpoint.local_addr().map_err(SessionError::GetLocalAddr)?;
    let next = ToOpen {
        local_addr,
        recv_connecting,
    };
    send_next
        .send(next)
        .map_err(|_| SessionError::FrontendClosed)?;

    debug!("Starting server loop");
    loop {
        let session = endpoint.accept().await;
        debug!("Accepting new session");

        WebTransportRuntime::spawn({
            let send_connecting = send_connecting.clone();
            async move {
                if let Err(err) = accept_session(session, send_connecting).await {
                    debug!("Failed to accept session: {err:?}");
                }
            }
        });
    }
}

async fn accept_session(
    session: IncomingSession,
    mut send_connecting: mpsc::Sender<ToConnecting>,
) -> Result<(), Closed> {
    let request = session.await.map_err(ServerError::AwaitSessionRequest)?;

    let (send_session_entity, recv_session_entity) = oneshot::channel::<Entity>();
    let (send_session_response, recv_session_response) = oneshot::channel::<SessionResponse>();
    let (send_dc, recv_dc) = oneshot::channel::<Disconnected>();
    let (send_next, recv_next) = oneshot::channel::<ToConnected>();
    send_connecting
        .send(ToConnecting {
            authority: request.authority().to_owned(),
            path: request.path().to_owned(),
            origin: request.origin().map(ToOwned::to_owned),
            user_agent: request.user_agent().map(ToOwned::to_owned),
            headers: request.headers().clone(),
            send_session_entity,
            send_session_response,
            recv_dc,
            recv_next,
        })
        .await
        .map_err(|_| SessionError::FrontendClosed)?;
    let session = recv_session_entity
        .await
        .map_err(|_| SessionError::FrontendClosed)?;

    let Err(dc_reason) = handle_session(request, recv_session_response, send_next)
        .instrument(debug_span!("session", %session))
        .await;
    _ = send_dc.send(dc_reason);
    Ok(())
}

async fn handle_session(
    request: SessionRequest,
    recv_session_response: oneshot::Receiver<SessionResponse>,
    send_connected: oneshot::Sender<ToConnected>,
) -> Result<Never, Disconnected> {
    debug!(
        "New session request from {}{}",
        request.authority(),
        request.path()
    );

    let session_response = recv_session_response
        .await
        .map_err(|_| SessionError::FrontendClosed)?;
    debug!("Frontend responded to this session request with {session_response:?}");

    let conn = match session_response {
        SessionResponse::Accepted => request.accept(),
        SessionResponse::Forbidden => {
            request.forbidden().await;
            return Err(ServerError::Rejected.into());
        }
        SessionResponse::NotFound => {
            request.not_found().await;
            return Err(ServerError::Rejected.into());
        }
    }
    .await
    .map(xwt_wtransport::Connection)
    .map_err(ServerError::AcceptSessionRequest)?;
    debug!("Connected");

    let (send_meta, recv_meta) = mpsc::channel::<SessionMeta>(1);
    let (send_packet_b2f, recv_packet_b2f) = mpsc::unbounded::<RecvPayload>();
    let (send_packet_f2b, recv_packet_f2b) = mpsc::unbounded::<SendPayload>();
    let (send_user_dc, recv_user_dc) = oneshot::channel::<String>();
    let next = ToConnected {
        initial_peer_addr: conn.0.remote_address(),
        initial_rtt: conn.0.rtt(),
        initial_mtu: conn
            .max_datagram_size()
            .ok_or(SessionError::DatagramsNotSupported)?,
        recv_meta,
        recv_packet_b2f,
        send_packet_f2b,
        send_user_dc,
    };
    let backend = SessionBackend {
        conn,
        send_meta,
        send_packet_b2f,
        recv_packet_f2b,
        recv_user_dc,
    };
    send_connected
        .send(next)
        .map_err(|_| SessionError::FrontendClosed)?;

    debug!("Starting session loop");
    Err(backend.start().await)
}
