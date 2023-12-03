use crate::driver::streams::uniremote::StreamUniRemoteH3;
use crate::driver::DriverError;
use crate::error::StreamReadError;
use crate::error::StreamReadExactError;
use std::future::pending;
use wtransport_proto::error::ErrorCode;
use wtransport_proto::stream_header::StreamKind;

pub struct RemoteQPackEncStream {
    stream: Option<StreamUniRemoteH3>,
    buffer: Box<[u8]>,
}

impl RemoteQPackEncStream {
    pub fn empty() -> Self {
        let buffer = vec![0; 64].into_boxed_slice();

        Self {
            stream: None,
            buffer,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.stream.is_none()
    }

    pub fn set_stream(&mut self, stream: StreamUniRemoteH3) {
        assert!(matches!(stream.kind(), StreamKind::QPackEncoder));
        self.stream = Some(stream);
    }

    pub async fn run(&mut self) -> DriverError {
        let stream = match self.stream.as_mut() {
            Some(stream) => stream,
            None => pending().await,
        };

        loop {
            match stream.stream_mut().read_exact(&mut self.buffer).await {
                Ok(()) => {}
                Err(StreamReadExactError::FinishedEarly) => {
                    return DriverError::Proto(ErrorCode::ClosedCriticalStream);
                }
                Err(StreamReadExactError::Read(StreamReadError::NotConnected)) => {
                    return DriverError::NotConnected;
                }
                Err(StreamReadExactError::Read(StreamReadError::Reset(_))) => {
                    return DriverError::Proto(ErrorCode::ClosedCriticalStream);
                }
                Err(StreamReadExactError::Read(StreamReadError::QuicProto)) => {
                    return DriverError::Proto(ErrorCode::ClosedCriticalStream);
                }
            }
        }
    }
}

pub struct RemoteQPackDecStream {
    stream: Option<StreamUniRemoteH3>,
    buffer: Box<[u8]>,
}

impl RemoteQPackDecStream {
    pub fn empty() -> Self {
        let buffer = vec![0; 64].into_boxed_slice();
        Self {
            stream: None,
            buffer,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.stream.is_none()
    }

    pub fn set_stream(&mut self, stream: StreamUniRemoteH3) {
        assert!(matches!(stream.kind(), StreamKind::QPackDecoder));
        self.stream = Some(stream);
    }

    pub async fn run(&mut self) -> DriverError {
        let stream = match self.stream.as_mut() {
            Some(stream) => stream,
            None => pending().await,
        };

        loop {
            match stream.stream_mut().read_exact(&mut self.buffer).await {
                Ok(()) => {}
                Err(StreamReadExactError::FinishedEarly) => {
                    return DriverError::Proto(ErrorCode::ClosedCriticalStream);
                }
                Err(StreamReadExactError::Read(StreamReadError::NotConnected)) => {
                    return DriverError::NotConnected;
                }
                Err(StreamReadExactError::Read(StreamReadError::Reset(_))) => {
                    return DriverError::Proto(ErrorCode::ClosedCriticalStream);
                }
                Err(StreamReadExactError::Read(StreamReadError::QuicProto)) => {
                    return DriverError::Proto(ErrorCode::ClosedCriticalStream);
                }
            }
        }
    }
}
