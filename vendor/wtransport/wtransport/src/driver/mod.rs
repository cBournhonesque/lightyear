use crate::datagram::Datagram;
use crate::driver::streams::biremote::StreamBiRemoteH3;
use crate::driver::streams::biremote::StreamBiRemoteWT;
use crate::driver::streams::session::StreamSession;
use crate::driver::streams::uniremote::StreamUniRemoteWT;
use crate::driver::streams::Stream;
use crate::driver::utils::bichannel;
use crate::driver::utils::shared_result;
use crate::driver::utils::SendError;
use crate::driver::utils::SharedResultGet;
use crate::driver::utils::SharedResultSet;
use crate::error::SendDatagramError;
use crate::stream::OpeningBiStream;
use crate::stream::OpeningUniStream;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tracing::debug;
use tracing::debug_span;
use tracing::instrument;
use tracing::trace;
use tracing::Instrument;
use utils::BiChannelEndpoint;
use wtransport_proto::error::ErrorCode;
use wtransport_proto::frame::Frame;
use wtransport_proto::ids::SessionId;
use wtransport_proto::session::SessionRequest;
use wtransport_proto::settings::Settings;

#[derive(Copy, Clone, Debug)]
pub enum DriverError {
    Proto(ErrorCode),
    NotConnected,
}

#[derive(Debug)]
pub struct Driver {
    quic_connection: quinn::Connection,
    ready_settings: Mutex<mpsc::Receiver<Settings>>,
    ready_sessions: BiChannelEndpoint<StreamSession>,
    ready_uni_wt_streams: Mutex<mpsc::Receiver<StreamUniRemoteWT>>,
    ready_bi_wt_streams: Mutex<mpsc::Receiver<StreamBiRemoteWT>>,
    ready_datagrams: Mutex<mpsc::Receiver<Datagram>>,
    driver_result: SharedResultGet<DriverError>,
}

impl Driver {
    pub fn init(quic_connection: quinn::Connection) -> Self {
        let ready_settings = mpsc::channel(1);
        let ready_sessions = bichannel(1);
        let ready_uni_wt_streams = mpsc::channel(4);
        let ready_bi_wt_streams = mpsc::channel(1);
        let ready_datagrams = mpsc::channel(1);
        let driver_result = shared_result();

        tokio::spawn(
            worker::Worker::new(
                quic_connection.clone(),
                ready_settings.0,
                ready_sessions.0,
                ready_uni_wt_streams.0,
                ready_bi_wt_streams.0,
                ready_datagrams.0,
                driver_result.0,
            )
            .run()
            .instrument(debug_span!("Driver", quic_id = quic_connection.stable_id())),
        );

        Self {
            quic_connection,
            ready_settings: Mutex::new(ready_settings.1),
            ready_sessions: ready_sessions.1,
            ready_uni_wt_streams: Mutex::new(ready_uni_wt_streams.1),
            ready_bi_wt_streams: Mutex::new(ready_bi_wt_streams.1),
            ready_datagrams: Mutex::new(ready_datagrams.1),
            driver_result: driver_result.1,
        }
    }

    pub async fn accept_settings(&self) -> Result<Settings, DriverError> {
        let mut lock = self.ready_settings.lock().await;

        match lock.recv().await {
            Some(settings) => Ok(settings),
            None => Err(self.result().await),
        }
    }

    pub async fn accept_session(&self) -> Result<StreamSession, DriverError> {
        match self.ready_sessions.recv().await {
            Some(session) => Ok(session),
            None => Err(self.result().await),
        }
    }

    pub async fn open_session(
        &self,
        session_request: SessionRequest,
    ) -> Result<StreamSession, DriverError> {
        let stream = Stream::open_bi(&self.quic_connection)
            .await
            .ok_or(DriverError::NotConnected)?
            .upgrade()
            .into_session(session_request);

        Ok(stream)
    }

    pub async fn register_session(&self, stream_session: StreamSession) -> Result<(), DriverError> {
        match self.ready_sessions.send(stream_session).await {
            Ok(()) => Ok(()),
            Err(SendError) => Err(self.result().await),
        }
    }

    pub async fn accept_uni(
        &self,
        session_id: SessionId,
    ) -> Result<StreamUniRemoteWT, DriverError> {
        let mut lock = self.ready_uni_wt_streams.lock().await;

        loop {
            let stream = match lock.recv().await {
                Some(stream) => stream,
                None => return Err(self.result().await),
            };

            if stream.session_id() == session_id {
                return Ok(stream);
            }

            debug!(
                "Discarding WT stream (stream_id: {}, session_id: {})",
                stream.id(),
                stream.session_id()
            );

            stream
                .into_stream()
                .stop(ErrorCode::BufferedStreamRejected.to_code())
                .expect("Stream not already stopped");
        }
    }

    pub async fn accept_bi(&self, session_id: SessionId) -> Result<StreamBiRemoteWT, DriverError> {
        let mut lock = self.ready_bi_wt_streams.lock().await;

        loop {
            let stream = match lock.recv().await {
                Some(stream) => stream,
                None => return Err(self.result().await),
            };

            if stream.session_id() == session_id {
                return Ok(stream);
            }

            debug!(
                "Discarding WT stream (stream_id: {}, session_id: {})",
                stream.id(),
                stream.session_id()
            );

            stream
                .into_stream()
                .1
                .stop(ErrorCode::BufferedStreamRejected.to_code())
                .expect("Stream not already stopped");
        }
    }

    pub async fn receive_datagram(&self, session_id: SessionId) -> Result<Datagram, DriverError> {
        let mut lock = self.ready_datagrams.lock().await;

        loop {
            let datagram = match lock.recv().await {
                Some(datagram) => datagram,
                None => {
                    return Err(self.result().await);
                }
            };

            if datagram.session_id() == session_id {
                return Ok(datagram);
            }

            debug!(
                "Incoming datagram discarded (session_id: {})",
                datagram.session_id()
            );
        }
    }

    pub async fn open_uni(&self, session_id: SessionId) -> Result<OpeningUniStream, DriverError> {
        let quic_stream = Stream::open_uni(&self.quic_connection)
            .await
            .ok_or(DriverError::NotConnected)?;

        Ok(OpeningUniStream::new(session_id, quic_stream))
    }

    pub async fn open_bi(&self, session_id: SessionId) -> Result<OpeningBiStream, DriverError> {
        let quic_stream = Stream::open_bi(&self.quic_connection)
            .await
            .ok_or(DriverError::NotConnected)?;

        Ok(OpeningBiStream::new(session_id, quic_stream))
    }

    pub fn send_datagram(
        &self,
        session_id: SessionId,
        payload: &[u8],
    ) -> Result<(), SendDatagramError> {
        let quic_datagram = Datagram::write(session_id, payload).into_quic_bytes();

        match self.quic_connection.send_datagram(quic_datagram) {
            Ok(()) => Ok(()),
            Err(quinn::SendDatagramError::UnsupportedByPeer) => {
                Err(SendDatagramError::UnsupportedByPeer)
            }
            Err(quinn::SendDatagramError::Disabled) => {
                unreachable!()
            }

            Err(quinn::SendDatagramError::TooLarge) => Err(SendDatagramError::TooLarge),
            Err(quinn::SendDatagramError::ConnectionLost(_)) => {
                Err(SendDatagramError::NotConnected)
            }
        }
    }

    async fn result(&self) -> DriverError {
        match self.driver_result.result().await {
            Some(error) => error,
            None => panic!("Driver worker panic!"),
        }
    }
}

mod worker {
    use super::*;
    use crate::driver::streams::qpack::RemoteQPackDecStream;
    use crate::driver::streams::qpack::RemoteQPackEncStream;
    use crate::driver::streams::settings::LocalSettingsStream;
    use crate::driver::streams::settings::RemoteSettingsStream;
    use crate::driver::streams::uniremote::StreamUniRemoteH3;
    use crate::driver::streams::ProtoReadError;
    use crate::driver::streams::ProtoWriteError;
    use crate::driver::utils::TrySendError;
    use utils::varint_w2q;
    use wtransport_proto::frame::FrameKind;
    use wtransport_proto::headers::Headers;
    use wtransport_proto::session::HeadersParseError;
    use wtransport_proto::stream_header::StreamHeader;
    use wtransport_proto::stream_header::StreamKind;

    pub struct Worker {
        quic_connection: quinn::Connection,
        ready_settings: mpsc::Sender<Settings>,
        ready_sessions: BiChannelEndpoint<StreamSession>,
        ready_uni_wt_streams: mpsc::Sender<StreamUniRemoteWT>,
        ready_bi_wt_streams: mpsc::Sender<StreamBiRemoteWT>,
        ready_datagrams: mpsc::Sender<Datagram>,
        driver_result: SharedResultSet<DriverError>,
        local_settings_stream: LocalSettingsStream,
        remote_settings_stream: RemoteSettingsStream,
        remote_qpack_enc_stream: RemoteQPackEncStream,
        remote_qpack_dec_stream: RemoteQPackDecStream,
        stream_session: Option<StreamSession>,
    }

    impl Worker {
        pub fn new(
            quic_connection: quinn::Connection,
            ready_settings: mpsc::Sender<Settings>,
            ready_sessions: BiChannelEndpoint<StreamSession>,
            ready_uni_wt_streams: mpsc::Sender<StreamUniRemoteWT>,
            ready_bi_wt_streams: mpsc::Sender<StreamBiRemoteWT>,
            ready_datagrams: mpsc::Sender<Datagram>,
            driver_result: SharedResultSet<DriverError>,
        ) -> Self {
            Self {
                quic_connection,
                ready_settings,
                ready_sessions,
                ready_uni_wt_streams,
                ready_bi_wt_streams,
                ready_datagrams,
                driver_result,
                local_settings_stream: LocalSettingsStream::empty(),
                remote_settings_stream: RemoteSettingsStream::empty(),
                remote_qpack_enc_stream: RemoteQPackEncStream::empty(),
                remote_qpack_dec_stream: RemoteQPackDecStream::empty(),
                stream_session: None,
            }
        }

        pub async fn run(mut self) {
            debug!("Started");

            let error = self
                .run_impl()
                .await
                .expect_err("Worker must return an error");

            debug!("Ended with error: {:?}", error);

            if let DriverError::Proto(error_code) = &error {
                self.quic_connection
                    .close(varint_w2q(error_code.to_code()), b"");
            }

            self.driver_result.set(error);
        }

        async fn run_impl(&mut self) -> Result<(), DriverError> {
            let mut remote_settings_watcher = self.remote_settings_stream.subscribe();
            let mut ready_uni_h3_streams = mpsc::channel(4);
            let mut ready_bi_h3_streams = mpsc::channel(1);

            self.open_and_send_settings().await?;

            loop {
                tokio::select! {
                    result = Self::accept_uni(&self.quic_connection,
                                              &ready_uni_h3_streams.0,
                                              &self.ready_uni_wt_streams) => {
                        result?;
                    }

                    result = Self::accept_bi(&self.quic_connection,
                                             &ready_bi_h3_streams.0,
                                             &self.ready_bi_wt_streams) => {
                        result?;
                    }

                    result = Self::accept_datagram(&self.quic_connection,
                                                   &self.ready_datagrams) => {
                        result?;
                    }

                    uni_h3_stream = ready_uni_h3_streams.1.recv() => {
                        let uni_h3_stream = uni_h3_stream.expect("Sender cannot be dropped")?;
                        self.handle_uni_h3_stream(uni_h3_stream)?;
                    }

                    bi_h3_stream = ready_bi_h3_streams.1.recv() => {
                        let (bi_h3_stream, first_frame) = bi_h3_stream.expect("Sender cannot be dropped")?;
                        self.handle_bi_h3_stream(bi_h3_stream, first_frame)?;
                    }


                    settings = remote_settings_watcher.accept_settings() => {
                        let settings = settings.expect("Channel cannot be dropped");
                        self.handle_remote_settings(settings)?;
                    }

                    stream_session = self.ready_sessions.recv() => {
                        match stream_session {
                            Some(stream_session) => {
                                if self.stream_session.is_none() {
                                    self.stream_session = Some(stream_session);
                                }
                            }
                            None => return Err(DriverError::NotConnected),
                        };
                    }

                    error = Self::run_control_streams(&mut self.local_settings_stream,
                                                      &mut self.remote_settings_stream,
                                                      &mut self.remote_qpack_enc_stream,
                                                      &mut self.remote_qpack_dec_stream,
                                                      &mut self.stream_session) => {
                        return Err(error);
                    }

                    () = self.driver_result.closed() => {
                        return Err(DriverError::NotConnected);
                    }
                }
            }
        }

        async fn open_and_send_settings(&mut self) -> Result<(), DriverError> {
            assert!(self.local_settings_stream.is_empty());

            let stream = match Stream::open_uni(&self.quic_connection)
                .await
                .ok_or(DriverError::NotConnected)?
                .upgrade(StreamHeader::new_control())
                .await
            {
                Ok(h3_stream) => h3_stream,
                Err(ProtoWriteError::NotConnected) => return Err(DriverError::NotConnected),
                Err(ProtoWriteError::Stopped) => {
                    return Err(DriverError::Proto(ErrorCode::ClosedCriticalStream));
                }
            };

            self.local_settings_stream.set_stream(stream);
            self.local_settings_stream.send_settings().await
        }

        async fn accept_uni(
            quic_connection: &quinn::Connection,
            ready_uni_h3_streams: &mpsc::Sender<Result<StreamUniRemoteH3, DriverError>>,
            ready_uni_wt_streams: &mpsc::Sender<StreamUniRemoteWT>,
        ) -> Result<(), DriverError> {
            trace!("H3 uni queue capacity: {}", ready_uni_h3_streams.capacity());
            let h3_slot = ready_uni_h3_streams
                .clone()
                .reserve_owned()
                .await
                .expect("Receiver cannot be dropped");

            let wt_slot = match ready_uni_wt_streams.clone().reserve_owned().await {
                Ok(wt_slot) => wt_slot,
                Err(mpsc::error::SendError(_)) => return Err(DriverError::NotConnected),
            };

            let stream_quic = Stream::accept_uni(quic_connection)
                .await
                .ok_or(DriverError::NotConnected)?;

            let stream_id = stream_quic.id();
            debug!("New incoming uni stream ({})", stream_id);

            tokio::spawn(
                async move {
                    let stream_h3 = match stream_quic.upgrade().await {
                        Ok(stream_h3) => stream_h3,
                        Err(ProtoReadError::H3(error_code)) => {
                            h3_slot.send(Err(DriverError::Proto(error_code)));
                            return;
                        }
                        Err(ProtoReadError::IO(_)) => {
                            return;
                        }
                    };

                    let stream_kind = stream_h3.kind();
                    debug!("Type: {:?}", stream_kind);

                    if matches!(stream_kind, StreamKind::WebTransport) {
                        let stream_wt = stream_h3.upgrade();
                        wt_slot.send(stream_wt);
                    } else {
                        h3_slot.send(Ok(stream_h3));
                    }
                }
                .instrument(debug_span!("Stream", "id={}", stream_id)),
            );

            Ok(())
        }

        async fn accept_bi(
            quic_connection: &quinn::Connection,
            ready_bi_h3_streams: &mpsc::Sender<
                Result<(StreamBiRemoteH3, Frame<'static>), DriverError>,
            >,
            ready_bi_wt_streams: &mpsc::Sender<StreamBiRemoteWT>,
        ) -> Result<(), DriverError> {
            trace!("H3 bi queue capacity: {}", ready_bi_h3_streams.capacity());
            let h3_slot = ready_bi_h3_streams
                .clone()
                .reserve_owned()
                .await
                .expect("Receiver cannot be dropped");

            let wt_slot = match ready_bi_wt_streams.clone().reserve_owned().await {
                Ok(wt_slot) => wt_slot,
                Err(mpsc::error::SendError(_)) => return Err(DriverError::NotConnected),
            };

            let stream_quic = Stream::accept_bi(quic_connection)
                .await
                .ok_or(DriverError::NotConnected)?;

            let stream_id = stream_quic.id();
            debug!("New incoming bi stream ({})", stream_id);

            tokio::spawn(
                async move {
                    let mut stream_h3 = stream_quic.upgrade();

                    let frame = match stream_h3.read_frame().await {
                        Ok(frame) => frame,
                        Err(ProtoReadError::H3(error_code)) => {
                            h3_slot.send(Err(DriverError::Proto(error_code)));
                            return;
                        }
                        Err(ProtoReadError::IO(_)) => {
                            return;
                        }
                    };

                    debug!("First frame kind: {:?}", frame.kind());

                    match frame.session_id() {
                        Some(session_id) => {
                            let stream_wt = stream_h3.upgrade(session_id);
                            wt_slot.send(stream_wt);
                        }
                        None => {
                            h3_slot.send(Ok((stream_h3, frame)));
                        }
                    }
                }
                .instrument(debug_span!("Stream", "id={}", stream_id)),
            );

            Ok(())
        }

        async fn accept_datagram(
            quic_connection: &quinn::Connection,
            ready_datagrams: &mpsc::Sender<Datagram>,
        ) -> Result<(), DriverError> {
            let slot = match ready_datagrams.reserve().await {
                Ok(slot) => slot,
                Err(mpsc::error::SendError(_)) => return Err(DriverError::NotConnected),
            };

            let quic_dgram = match quic_connection.read_datagram().await {
                Ok(quic_dgram) => quic_dgram,
                Err(_) => return Err(DriverError::NotConnected),
            };

            let datagram = match Datagram::read(quic_dgram) {
                Ok(datagram) => datagram,
                Err(error_code) => return Err(DriverError::Proto(error_code)),
            };

            debug!(
                "New incoming datagram (session_id: {})",
                datagram.session_id()
            );

            slot.send(datagram);

            Ok(())
        }

        fn handle_uni_h3_stream(&mut self, stream: StreamUniRemoteH3) -> Result<(), DriverError> {
            match stream.kind() {
                StreamKind::Control => {
                    if !self.remote_settings_stream.is_empty() {
                        return Err(DriverError::Proto(ErrorCode::StreamCreation));
                    }

                    self.remote_settings_stream.set_stream(stream);
                }
                StreamKind::QPackEncoder => {
                    if !self.remote_qpack_enc_stream.is_empty() {
                        return Err(DriverError::Proto(ErrorCode::StreamCreation));
                    }

                    self.remote_qpack_enc_stream.set_stream(stream);
                }
                StreamKind::QPackDecoder => {
                    if !self.remote_qpack_dec_stream.is_empty() {
                        return Err(DriverError::Proto(ErrorCode::StreamCreation));
                    }

                    self.remote_qpack_dec_stream.set_stream(stream);
                }
                StreamKind::WebTransport => unreachable!(),
                StreamKind::Exercise(_) => {}
            }

            Ok(())
        }

        #[instrument(skip_all, name = "Stream", fields(id = %stream.id()))]
        fn handle_bi_h3_stream(
            &mut self,
            mut stream: StreamBiRemoteH3,
            first_frame: Frame<'static>,
        ) -> Result<(), DriverError> {
            match first_frame.kind() {
                FrameKind::Data => {
                    return Err(DriverError::Proto(ErrorCode::FrameUnexpected));
                }
                FrameKind::Headers => {
                    let headers = match Headers::with_frame(&first_frame, stream.id()) {
                        Ok(headers) => headers,
                        Err(error_code) => return Err(DriverError::Proto(error_code)),
                    };

                    debug!("Headers: {:?}", headers);

                    let stream_session = match SessionRequest::try_from(headers) {
                        Ok(session_request) => stream.into_session(session_request),
                        Err(HeadersParseError::MethodNotConnect) => {
                            stream
                                .stop(ErrorCode::RequestRejected.to_code())
                                .expect("Stream not already stopped");
                            return Ok(());
                        }
                        // TODO(biagio): we might have more granularity with errors
                        Err(_) => {
                            stream
                                .stop(ErrorCode::Message.to_code())
                                .expect("Stream not already stopped");
                            return Ok(());
                        }
                    };

                    match self.ready_sessions.try_send(stream_session) {
                        Ok(()) => {}
                        Err(TrySendError::Full(mut stream)) => {
                            debug!("Discarding session request: sessions queue is full");
                            stream
                                .stop(ErrorCode::RequestRejected.to_code())
                                .expect("Stream not already stopped");
                        }
                        Err(TrySendError::Closed(_)) => return Err(DriverError::NotConnected),
                    }
                }
                FrameKind::Settings => {
                    return Err(DriverError::Proto(ErrorCode::FrameUnexpected));
                }
                FrameKind::WebTransport => unreachable!(),
                FrameKind::Exercise(_) => {}
            }

            Ok(())
        }

        async fn run_control_streams(
            local_settings: &mut LocalSettingsStream,
            remote_settings: &mut RemoteSettingsStream,
            remote_qpack_enc: &mut RemoteQPackEncStream,
            remote_qpack_dec: &mut RemoteQPackDecStream,
            _stream_session: &mut Option<StreamSession>,
        ) -> DriverError {
            // TODO(biagio): run stream_session
            tokio::select! {
                error = local_settings.run() => error,
                error = remote_settings.run() => error,
                error = remote_qpack_enc.run() => error,
                error = remote_qpack_dec.run() => error,
            }
        }

        fn handle_remote_settings(&mut self, settings: Settings) -> Result<(), DriverError> {
            debug!("Received: {:?}", settings);

            match self.ready_settings.try_send(settings) {
                Ok(()) => Ok(()),
                Err(mpsc::error::TrySendError::Closed(_)) => Err(DriverError::NotConnected),
                Err(mpsc::error::TrySendError::Full(_)) => {
                    unreachable!("No more than 1 setting frame can be processed")
                }
            }
        }
    }
}

pub(crate) mod streams;
pub(crate) mod utils;
