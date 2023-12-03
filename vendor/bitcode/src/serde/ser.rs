use crate::buffer::BufferTrait;
use crate::encoding::{Encoding, Fixed, Gamma};
use crate::write::Write;
use crate::{Encode, Error, Result, E};
use serde::ser::{
    SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant,
};
use serde::{Serialize, Serializer};

pub fn serialize_internal<'a>(
    buffer: &'a mut impl BufferTrait,
    t: &(impl Serialize + ?Sized),
) -> Result<&'a [u8]> {
    let mut writer = buffer.start_write();
    serialize_compat(t, Fixed, &mut writer)?;
    Ok(buffer.finish_write(writer))
}

pub fn serialize_compat(
    t: &(impl Serialize + ?Sized),
    encoding: impl Encoding,
    writer: &mut impl Write,
) -> Result<()> {
    t.serialize(BitcodeSerializer { encoding, writer })
}

pub struct BitcodeSerializer<'a, C, W> {
    encoding: C,
    writer: &'a mut W,
}

macro_rules! reborrow {
    ($e:expr) => {
        BitcodeSerializer {
            encoding: $e.encoding,
            writer: &mut *$e.writer,
        }
    }
}

impl<C: Encoding, W: Write> BitcodeSerializer<'_, C, W> {
    fn write_len(self, len: usize) -> Result<()> {
        len.encode(Gamma, self.writer)
    }

    fn write_variant_index(self, variant_index: u32) -> Result<()> {
        variant_index.encode(Gamma, self.writer)
    }
}

macro_rules! impl_ser {
    ($name:ident, $a:ty) => {
        #[inline(always)]
        fn $name(self, v: $a) -> Result<Self::Ok> {
            v.encode(self.encoding, self.writer)
        }
    };
}

impl<C: Encoding, W: Write> Serializer for BitcodeSerializer<'_, C, W> {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    impl_ser!(serialize_bool, bool);
    impl_ser!(serialize_i8, i8);
    impl_ser!(serialize_i16, i16);
    impl_ser!(serialize_i32, i32);
    impl_ser!(serialize_i64, i64);
    impl_ser!(serialize_i128, i128);
    impl_ser!(serialize_u8, u8);
    impl_ser!(serialize_u16, u16);
    impl_ser!(serialize_u32, u32);
    impl_ser!(serialize_u64, u64);
    impl_ser!(serialize_u128, u128);
    impl_ser!(serialize_f32, f32);
    impl_ser!(serialize_f64, f64);
    impl_ser!(serialize_char, char);
    impl_ser!(serialize_str, &str);

    fn serialize_bytes(self, v: &[u8]) -> Result<Self::Ok> {
        reborrow!(self).write_len(v.len())?;
        self.writer.write_bytes(v);
        Ok(())
    }

    #[inline(always)]
    fn serialize_none(self) -> Result<Self::Ok> {
        self.writer.write_false();
        Ok(())
    }

    #[inline(always)]
    fn serialize_some<T: ?Sized>(self, value: &T) -> Result<Self::Ok>
    where
        T: Serialize,
    {
        self.writer.write_bit(true);
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok> {
        Ok(())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok> {
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
    ) -> Result<Self::Ok> {
        self.write_variant_index(variant_index)
    }

    fn serialize_newtype_struct<T: ?Sized>(self, _name: &'static str, value: &T) -> Result<Self::Ok>
    where
        T: Serialize,
    {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized>(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok>
    where
        T: Serialize,
    {
        reborrow!(self).write_variant_index(variant_index)?;
        value.serialize(self)
    }

    #[inline(always)]
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq> {
        let len = len.expect("sequence must have len");
        reborrow!(self).write_len(len)?;
        Ok(self)
    }

    #[inline(always)]
    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple> {
        Ok(self)
    }

    #[inline(always)]
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct> {
        Ok(self)
    }

    #[inline(always)]
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant> {
        reborrow!(self).write_variant_index(variant_index)?;
        Ok(self)
    }

    #[inline(always)]
    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap> {
        let len = len.expect("sequence must have len");
        reborrow!(self).write_len(len)?;
        Ok(self)
    }

    #[inline(always)]
    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeStruct> {
        Ok(self)
    }

    #[inline(always)]
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant> {
        reborrow!(self).write_variant_index(variant_index)?;
        Ok(self)
    }

    #[inline(always)]
    fn is_human_readable(&self) -> bool {
        false
    }
}

macro_rules! ok_error_end {
    () => {
        type Ok = ();
        type Error = Error;
        fn end(self) -> Result<Self::Ok> {
            Ok(())
        }
    };
}

macro_rules! impl_seq {
    ($tr:ty, $fun:ident) => {
        impl<C: Encoding, W: Write> $tr for BitcodeSerializer<'_, C, W> {
            ok_error_end!();
            fn $fun<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
                value.serialize(reborrow!(self))
            }
        }
    };
}
impl_seq!(SerializeSeq, serialize_element);
impl_seq!(SerializeTuple, serialize_element);
impl_seq!(SerializeTupleStruct, serialize_field);
impl_seq!(SerializeTupleVariant, serialize_field);

macro_rules! impl_struct {
    ($tr:ty) => {
        impl<C: Encoding, W: Write> $tr for BitcodeSerializer<'_, C, W> {
            ok_error_end!();
            fn serialize_field<T: ?Sized>(&mut self, _key: &'static str, value: &T) -> Result<()>
            where
                T: Serialize,
            {
                value.serialize(reborrow!(self))
            }

            fn skip_field(&mut self, _key: &'static str) -> Result<()> {
                Err(E::NotSupported("skip_field").e())
            }
        }
    };
}
impl_struct!(SerializeStruct);
impl_struct!(SerializeStructVariant);

impl<C: Encoding, W: Write> SerializeMap for BitcodeSerializer<'_, C, W> {
    ok_error_end!();
    fn serialize_key<T: ?Sized>(&mut self, key: &T) -> Result<()>
    where
        T: Serialize,
    {
        key.serialize(reborrow!(self))
    }

    fn serialize_value<T: ?Sized>(&mut self, value: &T) -> Result<()>
    where
        T: Serialize,
    {
        value.serialize(reborrow!(self))
    }
}
