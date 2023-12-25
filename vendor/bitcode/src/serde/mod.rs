use crate::{Buffer, Error, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::fmt::Display;

pub mod de;
pub mod ser;

/// Serializes a `T:` [`Serialize`] into a [`Vec<u8>`].
///
/// **Warning:** The format is incompatible with [`decode`][`crate::decode`] and subject to change between versions.
// #[cfg_attr(doc, doc(cfg(feature = "serde")))]
pub fn serialize<T: ?Sized>(t: &T) -> Result<Vec<u8>>
where
    T: Serialize,
{
    Ok(Buffer::new().serialize(t)?.to_vec())
}

/// Deserializes a [`&[u8]`][`prim@slice`] into an instance of `T:` [`Deserialize`][`serde::Deserialize`].
///
/// **Warning:** The format is incompatible with [`encode`][`crate::encode`] and subject to change between versions.
// #[cfg_attr(doc, doc(cfg(feature = "serde")))]
pub fn deserialize<T>(bytes: &[u8]) -> Result<T>
where
    T: DeserializeOwned,
{
    Buffer::new().deserialize(bytes)
}

impl Buffer {
    /// Serializes a `T:` [`Serialize`] into a [`&[u8]`][`prim@slice`]. Can reuse the buffer's
    /// allocations.
    ///
    /// Even if you call `to_vec` on the [`&[u8]`][`prim@slice`], it's still more efficient than
    /// [`serialize`].
    ///
    /// **Warning:** The format is incompatible with [`decode`][`Buffer::decode`] and subject to change between versions.
    // #[cfg_attr(doc, doc(cfg(feature = "serde")))]
    pub fn serialize<T: ?Sized>(&mut self, t: &T) -> Result<&[u8]>
    where
        T: Serialize,
    {
        ser::serialize_internal(&mut self.0, t)
    }

    /// Deserializes a [`&[u8]`][`prim@slice`] into an instance of `T:` [`Deserialize`][`serde::Deserialize`]. Can reuse
    /// the buffer's allocations.
    ///
    /// **Warning:** The format is incompatible with [`encode`][`Buffer::encode`] and subject to change between versions.
    // #[cfg_attr(doc, doc(cfg(feature = "serde")))]
    pub fn deserialize<T>(&mut self, bytes: &[u8]) -> Result<T>
    where
        T: DeserializeOwned,
    {
        de::deserialize_internal(&mut self.0, bytes)
    }
}

impl serde::ser::Error for Error {
    fn custom<T>(_msg: T) -> Self
    where
        T: Display,
    {
        #[cfg(debug_assertions)]
        return Self(crate::E::Custom(_msg.to_string()));
        #[cfg(not(debug_assertions))]
        Self(())
    }
}

impl serde::de::Error for Error {
    fn custom<T>(_msg: T) -> Self
    where
        T: Display,
    {
        #[cfg(debug_assertions)]
        return Self(crate::E::Custom(_msg.to_string()));
        #[cfg(not(debug_assertions))]
        Self(())
    }
}
