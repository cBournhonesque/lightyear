use crate::varint::VarInt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;

/// HTTP3 protocol errors.
#[derive(Clone, Copy)]
pub enum ErrorCode {
    /// H3_DATAGRAM_ERROR.
    Datagram,

    /// H3_NO_ERROR.
    NoError,

    /// H3_STREAM_CREATION_ERROR.
    StreamCreation,

    /// H3_CLOSED_CRITICAL_STREAM.
    ClosedCriticalStream,

    /// H3_FRAME_UNEXPECTED.
    FrameUnexpected,

    /// H3_FRAME_ERROR.
    Frame,

    /// H3_EXCESSIVE_LOAD.
    ExcessiveLoad,

    /// H3_ID_ERROR.
    Id,

    /// H3_SETTINGS_ERROR.
    Settings,

    /// H3_MISSING_SETTINGS.
    MissingSettings,

    /// H3_REQUEST_REJECTED.
    RequestRejected,

    /// H3_MESSAGE_ERROR.
    Message,

    /// QPACK_DECOMPRESSION_FAILED.
    Decompression,

    /// WEBTRANSPORT_BUFFERED_STREAM_REJECTED.
    BufferedStreamRejected,

    /// WEBTRANSPORT_SESSION_GONE.
    SessionGone,
}

impl ErrorCode {
    /// Returns the integer representation (code) of the error.
    pub fn to_code(self) -> VarInt {
        match self {
            ErrorCode::Datagram => h3_error_codes::H3_DATAGRAM_ERROR,
            ErrorCode::NoError => h3_error_codes::H3_NO_ERROR,
            ErrorCode::StreamCreation => h3_error_codes::H3_STREAM_CREATION_ERROR,
            ErrorCode::ClosedCriticalStream => h3_error_codes::H3_CLOSED_CRITICAL_STREAM,
            ErrorCode::FrameUnexpected => h3_error_codes::H3_FRAME_UNEXPECTED,
            ErrorCode::Frame => h3_error_codes::H3_FRAME_ERROR,
            ErrorCode::ExcessiveLoad => h3_error_codes::H3_EXCESSIVE_LOAD,
            ErrorCode::Id => h3_error_codes::H3_ID_ERROR,
            ErrorCode::Settings => h3_error_codes::H3_SETTINGS_ERROR,
            ErrorCode::MissingSettings => h3_error_codes::H3_MISSING_SETTINGS,
            ErrorCode::RequestRejected => h3_error_codes::H3_REQUEST_REJECTED,
            ErrorCode::Message => h3_error_codes::H3_MESSAGE_ERROR,
            ErrorCode::Decompression => qpack_error_codes::QPACK_DECOMPRESSION_FAILED,
            ErrorCode::BufferedStreamRejected => {
                wt_error_codes::WEBTRANSPORT_BUFFERED_STREAM_REJECTED
            }
            ErrorCode::SessionGone => wt_error_codes::WEBTRANSPORT_SESSION_GONE,
        }
    }
}

impl Debug for ErrorCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code: {})", self, self.to_code())
    }
}

impl Display for ErrorCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorCode::Datagram => write!(f, "DatagramError"),
            ErrorCode::NoError => write!(f, "NoError"),
            ErrorCode::StreamCreation => write!(f, "StreamCreationError"),
            ErrorCode::ClosedCriticalStream => write!(f, "ClosedCriticalStreamError"),
            ErrorCode::FrameUnexpected => write!(f, "FrameUnexpectedError"),
            ErrorCode::Frame => write!(f, "FrameError"),
            ErrorCode::ExcessiveLoad => write!(f, "ExcessiveLoad"),
            ErrorCode::Id => write!(f, "IdError"),
            ErrorCode::Settings => write!(f, "SettingsError"),
            ErrorCode::MissingSettings => write!(f, "MissingSettingsError"),
            ErrorCode::RequestRejected => write!(f, "RequestRejectedError"),
            ErrorCode::Message => write!(f, "MessageError"),
            ErrorCode::Decompression => write!(f, "DecompressionError"),
            ErrorCode::BufferedStreamRejected => write!(f, "BufferedStreamRejected"),
            ErrorCode::SessionGone => write!(f, "SessionGone"),
        }
    }
}

impl std::error::Error for ErrorCode {}

mod h3_error_codes {
    use crate::varint::VarInt;

    pub const H3_DATAGRAM_ERROR: VarInt = VarInt::from_u32(0x33);
    pub const H3_NO_ERROR: VarInt = VarInt::from_u32(0x0100);
    pub const H3_STREAM_CREATION_ERROR: VarInt = VarInt::from_u32(0x0103);
    pub const H3_CLOSED_CRITICAL_STREAM: VarInt = VarInt::from_u32(0x0104);
    pub const H3_FRAME_UNEXPECTED: VarInt = VarInt::from_u32(0x0105);
    pub const H3_FRAME_ERROR: VarInt = VarInt::from_u32(0x0106);
    pub const H3_EXCESSIVE_LOAD: VarInt = VarInt::from_u32(0x0107);
    pub const H3_ID_ERROR: VarInt = VarInt::from_u32(0x0108);
    pub const H3_SETTINGS_ERROR: VarInt = VarInt::from_u32(0x0109);
    pub const H3_MISSING_SETTINGS: VarInt = VarInt::from_u32(0x010a);
    pub const H3_REQUEST_REJECTED: VarInt = VarInt::from_u32(0x010b);
    pub const H3_MESSAGE_ERROR: VarInt = VarInt::from_u32(0x010e);
}

mod qpack_error_codes {
    use crate::varint::VarInt;

    pub const QPACK_DECOMPRESSION_FAILED: VarInt = VarInt::from_u32(0x0200);
}

mod wt_error_codes {
    use crate::varint::VarInt;

    pub const WEBTRANSPORT_BUFFERED_STREAM_REJECTED: VarInt = VarInt::from_u32(0x3994_bd84);
    pub const WEBTRANSPORT_SESSION_GONE: VarInt = VarInt::from_u32(0x170d_7b68);
}
