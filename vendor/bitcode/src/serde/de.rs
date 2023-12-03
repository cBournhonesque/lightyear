use crate::buffer::BufferTrait;
use crate::encoding::{Encoding, Fixed, Gamma};
use crate::guard::guard_zst;
use crate::read::Read;
use crate::{Decode, Error, Result, E};
use serde::de::{
    DeserializeOwned, DeserializeSeed, EnumAccess, IntoDeserializer, MapAccess, SeqAccess,
    VariantAccess, Visitor,
};
use serde::Deserializer;
use std::num::NonZeroUsize;

pub fn deserialize_internal<B: BufferTrait, T: DeserializeOwned>(
    buffer: &mut B,
    bytes: &[u8],
) -> Result<T> {
    let (mut reader, context) = buffer.start_read(bytes);
    let decode_result = deserialize_compat(Fixed, &mut reader);
    B::finish_read_with_result(reader, context, decode_result)
}

pub fn deserialize_compat<T: DeserializeOwned>(
    encoding: impl Encoding,
    reader: &mut impl Read,
) -> Result<T> {
    T::deserialize(BitcodeDeserializer { encoding, reader })
}

struct BitcodeDeserializer<'a, C, R> {
    encoding: C,
    reader: &'a mut R,
}

macro_rules! reborrow {
    ($e:expr) => {
        BitcodeDeserializer {
            encoding: $e.encoding,
            reader: &mut *$e.reader,
        }
    }
}

impl<C: Encoding, R: Read> BitcodeDeserializer<'_, C, R> {
    fn read_len(self) -> Result<usize> {
        usize::decode(Gamma, self.reader)
    }

    fn read_variant_index(self) -> Result<u32> {
        u32::decode(Gamma, self.reader).map_err(|e| e.map_invalid("variant index"))
    }
}

macro_rules! impl_de {
    ($name:ident, $visit:ident) => {
        fn $name<V>(self, visitor: V) -> Result<V::Value>
        where
            V: Visitor<'de>,
        {
            visitor.$visit(Decode::decode(self.encoding, self.reader)?)
        }
    };
}

impl<'de, C: Encoding, R: Read> Deserializer<'de> for BitcodeDeserializer<'_, C, R> {
    type Error = Error;

    fn deserialize_any<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        Err(E::NotSupported("deserialize_any").e())
    }

    impl_de!(deserialize_bool, visit_bool);
    impl_de!(deserialize_i8, visit_i8);
    impl_de!(deserialize_i16, visit_i16);
    impl_de!(deserialize_i32, visit_i32);
    impl_de!(deserialize_i64, visit_i64);
    impl_de!(deserialize_i128, visit_i128);
    impl_de!(deserialize_u8, visit_u8);
    impl_de!(deserialize_u16, visit_u16);
    impl_de!(deserialize_u32, visit_u32);
    impl_de!(deserialize_u64, visit_u64);
    impl_de!(deserialize_u128, visit_u128);
    impl_de!(deserialize_f32, visit_f32);
    impl_de!(deserialize_f64, visit_f64);
    impl_de!(deserialize_char, visit_char);
    impl_de!(deserialize_string, visit_string);

    #[inline(always)] // Makes #[bitcode(with_serde)] ArrayString much faster.
    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_str(self.encoding.read_str(self.reader)?)
    }

    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let len = reborrow!(self).read_len()?;
        let bytes = if let Some(len) = NonZeroUsize::new(len) {
            self.reader.read_bytes(len)?
        } else {
            &[]
        };

        visitor.visit_bytes(bytes)
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        if self.reader.read_bit()? {
            visitor.visit_some(self)
        } else {
            visitor.visit_none()
        }
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V>(self, _name: &'static str, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V>(self, _name: &'static str, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        let len = reborrow!(self).read_len()?;
        self.deserialize_tuple(len, visitor)
    }

    // based on https://github.com/bincode-org/bincode/blob/c44b5e364e7084cdbabf9f94b63a3c7f32b8fb68/src/de/mod.rs#L293-L330
    fn deserialize_tuple<V>(self, len: usize, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        struct Access<'a, E, R> {
            deserializer: BitcodeDeserializer<'a, E, R>,
            len: usize,
        }

        impl<'de, C: Encoding, R: Read> SeqAccess<'de> for Access<'_, C, R> {
            type Error = Error;

            fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>>
            where
                T: DeserializeSeed<'de>,
            {
                guard_zst::<T::Value>(self.len)?;
                if self.len > 0 {
                    self.len -= 1;
                    let value = DeserializeSeed::deserialize(seed, reborrow!(self.deserializer))?;
                    Ok(Some(value))
                } else {
                    Ok(None)
                }
            }

            fn size_hint(&self) -> Option<usize> {
                Some(self.len)
            }
        }

        visitor.visit_seq(Access {
            deserializer: self,
            len,
        })
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_tuple(len, visitor)
    }

    // based on https://github.com/bincode-org/bincode/blob/c44b5e364e7084cdbabf9f94b63a3c7f32b8fb68/src/de/mod.rs#L353-L400
    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        struct Access<'a, E, R> {
            deserializer: BitcodeDeserializer<'a, E, R>,
            len: usize,
        }

        impl<'de, C: Encoding, R: Read> MapAccess<'de> for Access<'_, C, R> {
            type Error = Error;

            fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>>
            where
                K: DeserializeSeed<'de>,
            {
                guard_zst::<K::Value>(self.len)?;
                if self.len > 0 {
                    self.len -= 1;
                    let key = DeserializeSeed::deserialize(seed, reborrow!(self.deserializer))?;
                    Ok(Some(key))
                } else {
                    Ok(None)
                }
            }

            fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value>
            where
                V: DeserializeSeed<'de>,
            {
                let value = DeserializeSeed::deserialize(seed, reborrow!(self.deserializer))?;
                Ok(value)
            }

            fn size_hint(&self) -> Option<usize> {
                Some(self.len)
            }
        }

        let len = reborrow!(self).read_len()?;
        visitor.visit_map(Access {
            deserializer: self,
            len,
        })
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_tuple(fields.len(), visitor)
    }

    // based on https://github.com/bincode-org/bincode/blob/c44b5e364e7084cdbabf9f94b63a3c7f32b8fb68/src/de/mod.rs#L263-L291
    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        impl<'a, 'de, C: Encoding, R: Read> EnumAccess<'de> for BitcodeDeserializer<'a, C, R> {
            type Error = Error;
            type Variant = BitcodeDeserializer<'a, C, R>;

            fn variant_seed<V>(self, seed: V) -> Result<(V::Value, Self::Variant)>
            where
                V: DeserializeSeed<'de>,
            {
                let idx = reborrow!(self).read_variant_index()?;
                let val: Result<_> = seed.deserialize(idx.into_deserializer());
                Ok((val?, reborrow!(self)))
            }
        }

        visitor.visit_enum(self)
    }

    fn deserialize_identifier<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        Err(E::NotSupported("deserialize_identifier").e())
    }

    fn deserialize_ignored_any<V>(self, _visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        Err(E::NotSupported("deserialize_ignored_any").e())
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

// based on https://github.com/bincode-org/bincode/blob/c44b5e364e7084cdbabf9f94b63a3c7f32b8fb68/src/de/mod.rs#L461-L492
impl<'de, C: Encoding, R: Read> VariantAccess<'de> for BitcodeDeserializer<'_, C, R> {
    type Error = Error;

    fn unit_variant(self) -> Result<()> {
        Ok(())
    }

    fn newtype_variant_seed<T>(self, seed: T) -> Result<T::Value>
    where
        T: DeserializeSeed<'de>,
    {
        DeserializeSeed::deserialize(seed, self)
    }

    fn tuple_variant<V>(self, len: usize, visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        Deserializer::deserialize_tuple(self, len, visitor)
    }

    fn struct_variant<V>(self, fields: &'static [&'static str], visitor: V) -> Result<V::Value>
    where
        V: Visitor<'de>,
    {
        Deserializer::deserialize_tuple(self, fields.len(), visitor)
    }
}
