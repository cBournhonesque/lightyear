use crate::varint::VarInt;
use std::fmt;
use std::str::FromStr;

/// QUIC stream id.
#[derive(Copy, Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StreamId(VarInt);

impl StreamId {
    /// The largest stream id.
    pub const MAX: StreamId = StreamId(VarInt::MAX);

    /// New stream id.
    #[inline(always)]
    pub const fn new(varint: VarInt) -> Self {
        Self(varint)
    }

    /// Checks whether a stream is bi-directional or not.
    #[inline(always)]
    pub const fn is_bidirectional(self) -> bool {
        self.0.into_inner() & 0x2 == 0
    }

    /// Checks whether a stream is client-initiated or not.
    #[inline(always)]
    pub const fn is_client_initiated(self) -> bool {
        self.0.into_inner() & 0x1 == 0
    }

    /// Checks whether a stream is locally initiated or not.
    #[inline(always)]
    pub const fn is_local(self, is_server: bool) -> bool {
        (self.0.into_inner() & 0x1) == (is_server as u64)
    }

    /// Returns the integer value as `u64`.
    #[inline(always)]
    pub const fn into_u64(self) -> u64 {
        self.0.into_inner()
    }

    /// Returns the stream id as [`VarInt`] value.
    #[inline(always)]
    pub const fn into_varint(self) -> VarInt {
        self.0
    }
}

impl From<StreamId> for VarInt {
    #[inline(always)]
    fn from(stream_id: StreamId) -> Self {
        stream_id.0
    }
}

impl fmt::Debug for StreamId {
    #[inline(always)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Display for StreamId {
    #[inline(always)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Error for invalid Session ID value.
#[derive(Debug)]
pub struct InvalidSessionId;

/// A WebTransport session id.
///
/// Internally, it corresponds to a *bidirectional* *client-initiated* QUIC stream,
/// that is, a webtransport *session stream*.
#[derive(Copy, Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionId(StreamId);

impl SessionId {
    /// Returns the integer value as `u64`.
    #[inline(always)]
    pub const fn into_u64(self) -> u64 {
        self.0.into_u64()
    }

    /// Returns the session id as [`VarInt`] value.
    #[inline(always)]
    pub const fn into_varint(self) -> VarInt {
        self.0.into_varint()
    }

    /// Returns the corresponding session QUIC stream.
    #[inline(always)]
    pub const fn session_stream(self) -> StreamId {
        self.0
    }

    /// Tries to create a session id from its session stream.
    ///
    /// `stream_id` must be *bidirectional* and *client-initiated*, otherwise
    /// an [`Err`] is returned.
    pub fn try_from_session_stream(stream_id: StreamId) -> Result<Self, InvalidSessionId> {
        if stream_id.is_bidirectional() && stream_id.is_client_initiated() {
            Ok(Self(stream_id))
        } else {
            Err(InvalidSessionId)
        }
    }

    /// Creates a session id without checking session stream properties.
    ///
    /// # Safety
    ///
    /// `stream_id` must be *bidirectional* and *client-initiated*.
    #[inline(always)]
    pub const unsafe fn from_session_stream_unchecked(stream_id: StreamId) -> Self {
        debug_assert!(stream_id.is_bidirectional() && stream_id.is_client_initiated());
        Self(stream_id)
    }

    #[inline(always)]
    pub(crate) fn try_from_varint(varint: VarInt) -> Result<Self, InvalidSessionId> {
        Self::try_from_session_stream(StreamId::new(varint))
    }

    #[cfg(test)]
    pub(crate) fn maybe_invalid(varint: VarInt) -> Self {
        Self(StreamId::new(varint))
    }
}

impl fmt::Debug for SessionId {
    #[inline(always)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Display for SessionId {
    #[inline(always)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Error for invalid Quarter Stream ID value (too large).
#[derive(Debug)]
pub struct InvalidQStreamId;

/// HTTP3 Quarter Stream ID.
#[derive(Copy, Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct QStreamId(VarInt);

impl QStreamId {
    /// The largest quarter stream id.
    // SAFETY: value is less than max varint
    pub const MAX: QStreamId =
        unsafe { Self(VarInt::from_u64_unchecked(1_152_921_504_606_846_975)) };

    /// Creates a quarter stream id from its corresponding [`SessionId`]
    #[inline(always)]
    pub const fn from_session_id(session_id: SessionId) -> Self {
        let value = session_id.into_u64() >> 2;
        debug_assert!(value <= Self::MAX.into_u64());

        // SAFETY: after bitwise operation from stream id, result is surely a varint
        let varint = unsafe { VarInt::from_u64_unchecked(value) };

        Self(varint)
    }

    /// Returns its corresponding [`StreamId`].
    ///
    /// This is a *client-initiated* *bidirectional* stream.
    #[inline(always)]
    pub const fn into_stream_id(self) -> StreamId {
        // SAFETY: Quarter Stream ID origin from a valid Stream ID
        let varint = unsafe {
            debug_assert!(self.0.into_inner() << 2 <= VarInt::MAX.into_inner());
            VarInt::from_u64_unchecked(self.0.into_inner() << 2)
        };

        StreamId::new(varint)
    }

    /// Returns its corresponding [`SessionId`].
    #[inline(always)]
    pub const fn into_session_id(self) -> SessionId {
        let stream_id = self.into_stream_id();

        // SAFETY: corresponding stream for qstream is bidirectional and client-initiated
        unsafe {
            debug_assert!(stream_id.is_bidirectional() && stream_id.is_client_initiated());
            SessionId::from_session_stream_unchecked(stream_id)
        }
    }

    /// Returns the integer value as `u64`.
    #[inline(always)]
    pub const fn into_u64(self) -> u64 {
        self.0.into_inner()
    }

    /// Returns the quarter stream id as [`VarInt`] value.
    #[inline(always)]
    pub const fn into_varint(self) -> VarInt {
        self.0
    }

    pub(crate) fn try_from_varint(varint: VarInt) -> Result<Self, InvalidQStreamId> {
        if varint <= Self::MAX.into_varint() {
            Ok(Self(varint))
        } else {
            Err(InvalidQStreamId)
        }
    }

    #[cfg(test)]
    pub(crate) fn maybe_invalid(varint: VarInt) -> QStreamId {
        Self(varint)
    }
}

impl fmt::Debug for QStreamId {
    #[inline(always)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Display for QStreamId {
    #[inline(always)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Error for invalid HTTP status code.
#[derive(Debug)]
pub struct InvalidStatusCode;

/// HTTP status code (rfc9110).
#[derive(Default, Copy, Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StatusCode(u16);

impl StatusCode {
    /// The largest code.
    pub const MAX: Self = Self(599);

    /// The smallest code.
    pub const MIN: Self = Self(100);

    /// HTTP 200 OK status code.
    pub const OK: Self = Self(200);

    /// HTTP 403 Forbidden status code.
    pub const FORBIDDEN: Self = Self(403);

    /// HTTP 404 Not Found status code.
    pub const NOT_FOUND: Self = Self(404);

    /// Tries to construct from `u32`.
    #[inline(always)]
    pub fn try_from_u32(value: u32) -> Result<Self, InvalidStatusCode> {
        value.try_into()
    }

    /// Extracts the integer value as `u16`.
    #[inline(always)]
    pub fn into_inner(self) -> u16 {
        self.0
    }

    /// Returns true if the status code is 2xx.
    #[inline(always)]
    pub fn is_successful(self) -> bool {
        (200..300).contains(&self.0)
    }
}

impl TryFrom<u8> for StatusCode {
    type Error = InvalidStatusCode;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if u16::from(value) >= Self::MIN.0 && u16::from(value) <= Self::MAX.0 {
            Ok(Self(u16::from(value)))
        } else {
            Err(InvalidStatusCode)
        }
    }
}

impl TryFrom<u16> for StatusCode {
    type Error = InvalidStatusCode;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        if (Self::MIN.0..=Self::MAX.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(InvalidStatusCode)
        }
    }
}

impl TryFrom<u32> for StatusCode {
    type Error = InvalidStatusCode;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if value >= u32::from(Self::MIN.0) && value <= u32::from(Self::MAX.0) {
            Ok(Self(value as u16))
        } else {
            Err(InvalidStatusCode)
        }
    }
}

impl TryFrom<u64> for StatusCode {
    type Error = InvalidStatusCode;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value >= u64::from(Self::MIN.0) && value <= u64::from(Self::MAX.0) {
            Ok(Self(value as u16))
        } else {
            Err(InvalidStatusCode)
        }
    }
}

impl FromStr for StatusCode {
    type Err = InvalidStatusCode;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse().map_err(|_| InvalidStatusCode)?))
    }
}

impl fmt::Debug for StatusCode {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Display for StatusCode {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use utils::stream_types;
    use utils::StreamType;

    use super::*;

    #[test]
    fn stream_properties() {
        for (id, stream_type) in stream_types(1024) {
            let stream_id = StreamId::new(id);

            match stream_type {
                StreamType::ClientBi => {
                    assert!(stream_id.is_bidirectional());
                    assert!(stream_id.is_client_initiated());
                    assert!(stream_id.is_local(false));
                    assert!(!stream_id.is_local(true));
                }
                StreamType::ServerBi => {
                    assert!(stream_id.is_bidirectional());
                    assert!(!stream_id.is_client_initiated());
                    assert!(!stream_id.is_local(false));
                    assert!(stream_id.is_local(true));
                }
                StreamType::ClientUni => {
                    assert!(!stream_id.is_bidirectional());
                    assert!(stream_id.is_client_initiated());
                    assert!(stream_id.is_local(false));
                    assert!(!stream_id.is_local(true));
                }
                StreamType::ServerUni => {
                    assert!(!stream_id.is_bidirectional());
                    assert!(!stream_id.is_client_initiated());
                    assert!(!stream_id.is_local(false));
                    assert!(stream_id.is_local(true));
                }
            }
        }
    }

    #[test]
    fn session_id() {
        for (id, stream_type) in stream_types(1024) {
            if let StreamType::ClientBi = stream_type {
                assert!(SessionId::try_from_varint(id).is_ok());
                assert!(SessionId::try_from_session_stream(StreamId::new(id)).is_ok());
            } else {
                assert!(SessionId::try_from_varint(id).is_err());
                assert!(SessionId::try_from_session_stream(StreamId::new(id)).is_err());
            }
        }
    }

    #[test]
    fn qstream_id() {
        for (quarter, id) in stream_types(1024)
            .filter(|(_id, r#type)| matches!(r#type, StreamType::ClientBi))
            .map(|(id, _type)| id)
            .enumerate()
        {
            let session_id = SessionId::try_from_varint(id).unwrap();
            let qstream_id = QStreamId::from_session_id(session_id);

            assert_eq!(qstream_id.into_stream_id(), session_id.session_stream());
            assert_eq!(qstream_id.into_session_id(), session_id);
            assert_eq!(qstream_id.into_u64(), quarter as u64);
        }
    }

    mod utils {
        use super::*;

        #[derive(Copy, Clone, Debug)]
        pub enum StreamType {
            ClientBi,
            ServerBi,
            ClientUni,
            ServerUni,
        }

        pub fn stream_types(max_id: u32) -> impl Iterator<Item = (VarInt, StreamType)> {
            [
                StreamType::ClientBi,
                StreamType::ServerBi,
                StreamType::ClientUni,
                StreamType::ServerUni,
            ]
            .into_iter()
            .cycle()
            .enumerate()
            .map(|(index, r#type)| (VarInt::from_u32(index as u32), r#type))
            .take(max_id as usize)
        }
    }
}
