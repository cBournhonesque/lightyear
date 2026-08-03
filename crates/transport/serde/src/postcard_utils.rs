//! Postcard adapters for Lightyear's reusable byte buffers.

use crate::reader::Reader;
use crate::writer::Writer;
use postcard::ser_flavors::Flavor;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Serializes directly into an existing [`Writer`] allocation.
pub(crate) fn to_writer<T: Serialize + ?Sized>(
    value: &T,
    writer: &mut Writer,
) -> postcard::Result<()> {
    postcard::serialize_with_flavor(value, WriterFlavor(writer))
}

struct WriterFlavor<'a>(&'a mut Writer);

impl Flavor for WriterFlavor<'_> {
    type Output = ();

    #[inline]
    fn try_push(&mut self, data: u8) -> postcard::Result<()> {
        self.0.extend_from_slice(&[data]);
        Ok(())
    }

    #[inline]
    fn try_extend(&mut self, data: &[u8]) -> postcard::Result<()> {
        self.0.extend_from_slice(data);
        Ok(())
    }

    #[inline]
    fn finalize(self) -> postcard::Result<Self::Output> {
        Ok(())
    }
}

/// Deserializes from the unread part of a [`Reader`] and advances its cursor.
pub(crate) fn from_reader<T: DeserializeOwned>(reader: &mut Reader) -> postcard::Result<T> {
    let position = usize::try_from(reader.position())
        .map_err(|_| postcard::Error::DeserializeUnexpectedEnd)?;
    let (value, consumed) = {
        let remaining = reader
            .as_ref()
            .get(position..)
            .ok_or(postcard::Error::DeserializeUnexpectedEnd)?;
        let initial_len = remaining.len();
        let (value, remainder) = postcard::take_from_bytes(remaining)?;
        (value, initial_len - remainder.len())
    };
    reader.set_position((position + consumed) as u64);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct Message {
        sequence: u64,
        text: String,
        values: Vec<i16>,
    }

    #[test]
    fn reuses_writer_and_advances_reader() {
        let first = Message {
            sequence: 1,
            text: String::from("first"),
            values: vec![-1, 2, 300],
        };
        let second = Message {
            sequence: 2,
            text: String::from("second"),
            values: vec![4, 5, 6],
        };
        let mut writer = Writer::with_capacity(1);

        to_writer(&first, &mut writer).unwrap();
        let first_len = writer.len();
        to_writer(&second, &mut writer).unwrap();

        assert!(writer.len() > first_len);
        let mut reader = Reader::from(writer.to_bytes());
        assert_eq!(from_reader::<Message>(&mut reader).unwrap(), first);
        assert_eq!(reader.position() as usize, first_len);
        assert_eq!(from_reader::<Message>(&mut reader).unwrap(), second);
        assert_eq!(reader.remaining(), 0);
    }
}
