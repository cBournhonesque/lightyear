use crate::error::ErrorCode;
use crate::frame::Frame;
use crate::frame::FrameKind;
use crate::ids::StreamId;
use ls_qpack::decoder::Decoder;
use ls_qpack::decoder::DecoderOutput;
use ls_qpack::encoder::Encoder;
use ls_qpack::errors::DecoderError;
use std::borrow::Cow;
use std::collections::HashMap;

/// HTTP3 headers from the request or response.
#[derive(Debug)]
pub struct Headers(HashMap<String, String>);

impl Headers {
    /// Constructs the headers from a HTTP3 [`Frame`].
    ///
    /// # Panics
    ///
    /// Panics if `frame` is not type [`FrameKind::Headers`].
    pub fn with_frame(frame: &Frame, stream_id: StreamId) -> Result<Self, ErrorCode> {
        assert!(matches!(frame.kind(), FrameKind::Headers));

        let mut decoder = Decoder::new(0, 0);

        match decoder
            .decode(stream_id.into(), frame.payload())
            .map_err(|DecoderError| ErrorCode::Decompression)?
        {
            DecoderOutput::Done(headers) => Ok(headers
                .into_iter()
                .map(|h| (h.name().to_string(), h.value().to_string()))
                .collect()),
            DecoderOutput::BlockedStream => unreachable!("Dynamic table is not allowed"),
        }
    }

    /// Generates a [`Frame`] with these headers.
    pub fn generate_frame(&self, stream_id: StreamId) -> Frame<'static> {
        let mut encoder = Encoder::new();

        let (enc_headers, enc_stream) = encoder
            .encode_all(stream_id.into(), self.0.iter())
            .expect("Static encoding is not expected to fail")
            .take();

        debug_assert_eq!(enc_stream.len(), 0);

        Frame::new_headers(Cow::Owned(enc_headers.to_vec()))
    }

    /// Returns a reference to the value associated with the key.
    #[inline(always)]
    pub fn get<K>(&self, key: K) -> Option<&str>
    where
        K: AsRef<str>,
    {
        self.0.get(key.as_ref()).map(|s| s.as_str())
    }

    /// Inserts a field (key, value) in the headers.
    ///
    /// If the headers did have this key present, the value is updated.
    #[inline(always)]
    pub fn insert<K, V>(&mut self, key: K, value: V)
    where
        K: ToString,
        V: ToString,
    {
        self.0.insert(key.to_string(), value.to_string());
    }
}

impl<K, V> FromIterator<(K, V)> for Headers
where
    K: ToString,
    V: ToString,
{
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        Self(
            iter.into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }
}

impl AsRef<HashMap<String, String>> for Headers {
    fn as_ref(&self) -> &HashMap<String, String> {
        &self.0
    }
}

impl From<StreamId> for ls_qpack::StreamId {
    #[inline(always)]
    fn from(value: StreamId) -> Self {
        ls_qpack::StreamId::new(value.into_u64())
    }
}
