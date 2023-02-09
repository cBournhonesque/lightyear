use super::{
    error::Error,
    reader_writer::{BitReader, BitWrite},
};
use ::serde::Deserialize;
use ::serde::de::{
    self, DeserializeSeed, EnumAccess, IntoDeserializer, MapAccess, SeqAccess,
    VariantAccess, Visitor,
};
use crate::{Serde, UnsignedInteger, UnsignedVariableInteger};


pub struct Deserializer<'de, 'b> {
    pub(crate) reader: &'de mut BitReader<'b>,
}


impl<'de, 'a> ::serde::de::Deserializer<'de> for &'a mut Deserializer<'de, '_> {
    type Error = Error;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        panic!("deserialize any is not supported");
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_bool(bool::de(self.reader)?)
    }

    fn deserialize_i8<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_i8(i8::de(self.reader)?)
    }

    fn deserialize_i16<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_i16(i16::de(self.reader)?)
    }

    fn deserialize_i32<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_i32(i32::de(self.reader)?)
    }

    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_i64(i64::de(self.reader)?)
    }

    fn deserialize_u8<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_u8(u8::de(self.reader)?)
    }

    fn deserialize_u16<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_u16(u16::de(self.reader)?)
    }

    fn deserialize_u32<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_u32(u32::de(self.reader)?)
    }

    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_u64(u64::de(self.reader)?)
    }

    fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_f32(f32::de(self.reader)?)
    }

    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_f64(f64::de(self.reader)?)
    }

    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_char(char::de(self.reader)?)
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_str(<&str>::de(self.reader)?)
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_string(String::de(self.reader)?)
    }

    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
         panic!("cant")
        // visitor.visit_bytes(<&[u8]>::de(self.reader)?)
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_byte_buf(Vec::<u8>::de(self.reader)?)
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        let is_some = bool::de(self.reader)?;
        if is_some {
            visitor.visit_some(self)
        } else {
            visitor.visit_none()
        }
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V>(self, name: &'static str, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V>(self, name: &'static str, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        let length = UnsignedVariableInteger::<5>::de(self.reader)?;
        self.deserialize_tuple(length.get() as usize, visitor)
    }

    fn deserialize_tuple<V>(mut self, len: usize, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de>,
    {
        visitor.visit_seq(LenAccess {
            deserializer: &mut self,
            len,
        })
    }

    fn deserialize_tuple_struct<V>(self, name: &'static str, len: usize, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        self.deserialize_tuple(len, visitor)
    }

    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        let length = UnsignedVariableInteger::<5>::de(self.reader)?;
        self.deserialize_tuple(length.get() as usize, visitor)
    }

    fn deserialize_struct<V>(self, name: &'static str, fields: &'static [&'static str], visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        self.deserialize_tuple(fields.len(), visitor)
    }

    fn deserialize_enum<V>(self, name: &'static str, variants: &'static [&'static str], visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        visitor.visit_enum(Enum::new(self))
    }

    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        Err(Error)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, Self::Error> where V: Visitor<'de> {
        Err(Error)
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

struct Enum<'a, 'de: 'a, 'b> {
    de: &'a mut Deserializer<'de, 'b>
}

impl<'a, 'de, 'b> Enum<'a, 'de, 'b> {
    fn new(de: &'a mut Deserializer<'de, 'b>) -> Self {
        Enum { de }
    }
}

impl<'de, 'a, 'b> EnumAccess<'de> for Enum<'a, 'de, 'b> {
    type Error = Error;
    type Variant = Self;

    fn variant_seed<V>(mut self, seed: V) -> Result<(V::Value, Self::Variant), Self::Error>
        where
            V: DeserializeSeed<'de>,
    {
        let index = UnsignedInteger::<2u8>::de(self.de.reader)?.get() as u16;
        let val = seed.deserialize(index.into_deserializer())?;
        Ok((val, self))
    }
}

impl<'de, 'a, 'b> VariantAccess<'de> for Enum<'a, 'de, 'b> {
    type Error = Error;

    fn unit_variant(self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn newtype_variant_seed<T>(self, seed: T) -> Result<T::Value, Self::Error>
        where
            T: DeserializeSeed<'de>,
    {
        DeserializeSeed::deserialize(seed, self.de)
    }

    fn tuple_variant<V>(self, len: usize, visitor: V) -> Result<V::Value, Self::Error>
        where
            V: Visitor<'de>,
    {
        ::serde::de::Deserializer::deserialize_tuple(self.de, len, visitor)
    }

    fn struct_variant<V>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
        where
            V: Visitor<'de>,
    {
        ::serde::de::Deserializer::deserialize_tuple(self.de, fields.len(), visitor)
    }
}

/// Access for variable size sequence.
/// The length of the sequence tells us when the sequence ends
struct LenAccess<'a, 'de: 'a, 'b> {
    deserializer: &'a mut Deserializer<'de, 'b>,
    len: usize,
}

impl<'a, 'de, 'b> SeqAccess<'de> for LenAccess<'a, 'de, 'b> {
    type Error = Error;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>, Error>
        where
            T: DeserializeSeed<'de>,
    {
        if self.len > 0 {
            self.len -= 1;
            let value = DeserializeSeed::deserialize(
                seed,
                    &mut *self.deserializer
            )?;
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.len)
    }
}

impl<'a, 'de, 'b> MapAccess<'de> for LenAccess<'a, 'de, 'b> {
    type Error = Error;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>, Error>
        where K: DeserializeSeed<'de>,
    {
        if self.len > 0 {
            self.len -= 1;
            let key = DeserializeSeed::deserialize(
                seed,
                    &mut *self.deserializer
            )?;
            Ok(Some(key))
        } else {
            Ok(None)
        }
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value, Error>
        where
            V: DeserializeSeed<'de>,
    {
        let value = DeserializeSeed::deserialize(
            seed,
                &mut *self.deserializer
        )?;
        Ok(value)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.len)
    }
}