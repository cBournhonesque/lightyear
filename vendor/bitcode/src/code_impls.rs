use crate::code::{optimized_dec, optimized_enc, Decode, Encode};
use crate::encoding::{Encoding, Fixed, Gamma};
use crate::guard::guard_len;
use crate::nightly::{max, min};
use crate::read::Read;
use crate::write::Write;
use crate::{Result, E};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::hash::{BuildHasher, Hash};
use std::marker::PhantomData;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::num::*;
use std::time::Duration;

macro_rules! impl_enc_const {
    ($v:expr) => {
        const ENCODE_MIN: usize = $v;
        const ENCODE_MAX: usize = $v;
    };
}

macro_rules! impl_enc_size {
    ($t:ty) => {
        impl_enc_const!(std::mem::size_of::<$t>() * u8::BITS as usize);
    };
}

macro_rules! impl_enc_same {
    ($other:ty) => {
        const ENCODE_MIN: usize = <$other>::ENCODE_MIN;
        const ENCODE_MAX: usize = <$other>::ENCODE_MAX;
    };
}

macro_rules! impl_dec_from_enc {
    () => {
        const DECODE_MIN: usize = Self::ENCODE_MIN;
        const DECODE_MAX: usize = Self::ENCODE_MAX;
    };
}

macro_rules! impl_dec_same {
    ($other:ty) => {
        const DECODE_MIN: usize = <$other>::DECODE_MIN;
        const DECODE_MAX: usize = <$other>::DECODE_MAX;
    };
}

impl Encode for bool {
    impl_enc_const!(1);

    #[inline(always)]
    fn encode(&self, _: impl Encoding, writer: &mut impl Write) -> Result<()> {
        writer.write_bit(*self);
        Ok(())
    }
}

impl Decode for bool {
    impl_dec_from_enc!();

    #[inline(always)]
    fn decode(_: impl Encoding, reader: &mut impl Read) -> Result<Self> {
        reader.read_bit()
    }
}

macro_rules! impl_uints {
    ($read:ident, $write:ident, $($int: ty),*) => {
        $(
            impl Encode for $int {
                impl_enc_size!(Self);

                #[inline(always)]
                fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
                    encoding.$write::<{ Self::BITS as usize }>(writer, (*self).into());
                    Ok(())
                }
            }

            impl Decode for $int {
                impl_dec_from_enc!();

                #[inline(always)]
                fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
                    Ok(encoding.$read::<{ Self::BITS as usize }>(reader)? as Self)
                }
            }
        )*
    }
}

macro_rules! impl_ints {
    ($read:ident, $write:ident, $($int: ty => $uint: ty),*) => {
        $(
            impl Encode for $int {
                impl_enc_size!(Self);

                #[inline(always)]
                fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
                    let word = if encoding.zigzag() {
                        zigzag::ZigZagEncode::zigzag_encode(*self).into()
                    } else {
                        (*self as $uint).into()
                    };
                    encoding.$write::<{ Self::BITS as usize }>(writer, word);
                    Ok(())
                }
            }

            impl Decode for $int {
                impl_dec_from_enc!();

                #[inline(always)]
                fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
                    let word = encoding.$read::<{ Self::BITS as usize }>(reader)?;
                    let sint = if encoding.zigzag() {
                        zigzag::ZigZagDecode::zigzag_decode(word as $uint)
                    } else {
                        word as Self
                    };
                    Ok(sint)
                }
            }
        )*
    }
}

impl_uints!(read_u64, write_u64, u8, u16, u32, u64);
impl_ints!(read_u64, write_u64, i8 => u8, i16 => u16, i32 => u32, i64 => u64);
impl_uints!(read_u128, write_u128, u128);
impl_ints!(read_u128, write_u128, i128 => u128);

macro_rules! impl_try_int {
    ($a:ty, $b:ty) => {
        impl Encode for $a {
            impl_enc_size!($b);

            #[inline(always)]
            fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
                (*self as $b).encode(encoding, writer)
            }
        }

        impl Decode for $a {
            impl_dec_from_enc!();

            #[inline(always)]
            fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
                <$b>::decode(encoding, reader)?
                    .try_into()
                    .map_err(|_| E::Invalid(stringify!($a)).e())
            }
        }
    };
}

impl_try_int!(usize, u64);
impl_try_int!(isize, i64);

macro_rules! impl_float {
    ($a:ty, $write:ident, $read:ident) => {
        impl Encode for $a {
            impl_enc_size!($a);

            #[inline(always)]
            fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
                encoding.$write(writer, *self);
                Ok(())
            }
        }

        impl Decode for $a {
            impl_dec_from_enc!();

            #[inline(always)]
            fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
                encoding.$read(reader)
            }
        }
    };
}

impl_float!(f32, write_f32, read_f32);
impl_float!(f64, write_f64, read_f64);

// Subtracts 1 in encode and adds one in decode (so gamma is smaller).
macro_rules! impl_non_zero {
    ($($a:ty),*) => {
        $(
            impl Encode for $a {
                impl_enc_size!($a);

                #[inline(always)]
                fn encode(&self, _: impl Encoding, writer: &mut impl Write) -> Result<()> {
                    (self.get() - 1).encode(Fixed, writer)
                }
            }

            impl Decode for $a {
                impl_dec_from_enc!();

                #[inline(always)]
                fn decode(_: impl Encoding, reader: &mut impl Read) -> Result<Self> {
                    let v = Decode::decode(Fixed, reader)?;
                    let _ = Self::new(v); // Type inference.
                    Self::new(v.wrapping_add(1)).ok_or_else(|| E::Invalid("non zero").e())
                }
            }
        )*
    };
}

impl_non_zero!(NonZeroU8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroUsize);
impl_non_zero!(NonZeroI8, NonZeroI16, NonZeroI32, NonZeroI64, NonZeroIsize);

impl Encode for char {
    impl_enc_const!(21);

    #[inline(always)]
    fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
        encoding.write_u64::<21>(writer, *self as u64);
        Ok(())
    }
}

impl Decode for char {
    impl_dec_from_enc!();

    #[inline(always)]
    fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
        let bits = encoding.read_u64::<21>(reader)? as u32;
        char::from_u32(bits).ok_or_else(|| E::Invalid("char").e())
    }
}

impl<T: Encode> Encode for Option<T> {
    const ENCODE_MIN: usize = 1;
    const ENCODE_MAX: usize = T::ENCODE_MAX.saturating_add(1);

    fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
        if let Some(t) = self {
            fn encode_some<T: Encode>(
                t: &T,
                encoding: impl Encoding,
                writer: &mut impl Write,
            ) -> Result<()> {
                optimized_enc!(encoding, writer);
                enc!(true, bool);
                enc!(t, T);
                end_enc!();
                Ok(())
            }
            encode_some(t, encoding, writer)
        } else {
            writer.write_false();
            Ok(())
        }
    }
}

impl<T: Decode> Decode for Option<T> {
    const DECODE_MIN: usize = 1;
    const DECODE_MAX: usize = T::DECODE_MAX.saturating_add(1);

    fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
        optimized_dec!(encoding, reader);
        dec!(v, bool);
        if v {
            dec!(t, T);
            end_dec!();
            Ok(Some(t))
        } else {
            end_dec!();
            Ok(None)
        }
    }
}

macro_rules! impl_either {
    ($typ: path, $a: ident, $a_t:ty, $b:ident, $b_t: ty $(,$($generic: ident);*)*) => {
        impl $(<$($generic: Encode),*>)* Encode for $typ {
            const ENCODE_MIN: usize = 1 + min(<$a_t>::ENCODE_MIN, <$b_t>::ENCODE_MIN);
            const ENCODE_MAX: usize = max(<$a_t>::ENCODE_MAX, <$b_t>::ENCODE_MAX).saturating_add(1);

            fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
                match self {
                    Self::$a(a) => {
                        writer.write_false();
                        optimized_enc!(encoding, writer);
                        enc!(a, $a_t);
                        end_enc!();
                        Ok(())
                    },
                    Self::$b(b) => {
                        optimized_enc!(encoding, writer);
                        enc!(true, bool);
                        enc!(b, $b_t);
                        end_enc!();
                        Ok(())
                    },
                }
            }
        }

        impl $(<$($generic: Decode),*>)* Decode for $typ {
            const DECODE_MIN: usize = 1 + min(<$a_t>::DECODE_MIN, <$b_t>::DECODE_MIN);
            const DECODE_MAX: usize = max(<$a_t>::DECODE_MAX, <$b_t>::DECODE_MAX).saturating_add(1);

            fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
                optimized_dec!(encoding, reader);
                dec!(v, bool);
                Ok(if v {
                    dec!(b, $b_t);
                    end_dec!();
                    Self::$b(b)
                } else {
                    dec!(a, $a_t);
                    end_dec!();
                    Self::$a(a)
                })
            }
        }
    }
}

impl_either!(std::result::Result<T, E>, Ok, T, Err, E, T ; E);

macro_rules! impl_wrapper {
    ($(::$ptr: ident)*) => {
        impl<T: Encode> Encode for $(::$ptr)*<T> {
            impl_enc_same!(T);

            fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
                T::encode(&self.0, encoding, writer)
            }
        }

        impl<T: Decode> Decode for $(::$ptr)*<T> {
            impl_dec_same!(T);

            fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
                Ok(Self(T::decode(encoding, reader)?))
            }
        }
    }
}

impl_wrapper!(::std::num::Wrapping);
impl_wrapper!(::std::cmp::Reverse);

macro_rules! impl_smart_ptr {
    ($(::$ptr: ident)*) => {
        impl<T: Encode + ?Sized> Encode for $(::$ptr)*<T> {
            impl_enc_same!(T);

            fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
                T::encode(self, encoding, writer)
            }
        }

        impl<T: Decode> Decode for $(::$ptr)*<T> {
            impl_dec_same!(T);

            fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
                Ok(T::decode(encoding, reader)?.into())
            }
        }

        impl<T: Decode> Decode for $(::$ptr)*<[T]> {
            impl_dec_same!(Vec<T>);

            fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
                Ok(Vec::<T>::decode(encoding, reader)?.into()) // TODO avoid Vec<T> allocation for Rc<[T]> and Arc<[T]>.
            }
        }

        impl Decode for $(::$ptr)*<str> {
            impl_dec_same!(String);

            fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
                Ok(String::decode(encoding, reader)?.into())
            }
        }
    }
}

impl_smart_ptr!(::std::boxed::Box);
impl_smart_ptr!(::std::rc::Rc);
impl_smart_ptr!(::std::sync::Arc);

// Writes multiple elements per flush.
#[cfg_attr(not(debug_assertions), inline(always))]
fn encode_elements<T: Encode>(
    elements: &[T],
    encoding: impl Encoding,
    writer: &mut impl Write,
) -> Result<()> {
    if T::ENCODE_MAX == 0 {
        return Ok(()); // Nothing to serialize.
    }
    let chunk_size = 64 / T::ENCODE_MAX;

    if chunk_size > 1 && encoding.is_fixed() {
        let mut buf = crate::register_buffer::RegisterWriter::new(writer);

        let chunks = elements.chunks_exact(chunk_size);
        let remainder = chunks.remainder();

        for chunk in chunks {
            for t in chunk {
                t.encode(encoding, &mut buf.inner)?;
            }
            buf.flush();
        }

        if !remainder.is_empty() {
            for t in remainder {
                t.encode(encoding, &mut buf.inner)?;
            }
            buf.flush();
        }
    } else {
        for t in elements.iter() {
            t.encode(encoding, writer)?
        }
    }
    Ok(())
}

// Reads multiple elements per flush.
#[cfg_attr(not(debug_assertions), inline(always))]
fn decode_elements<T: Decode>(
    len: usize,
    encoding: impl Encoding,
    reader: &mut impl Read,
) -> Result<Vec<T>> {
    let chunk_size = if encoding.is_fixed() && T::DECODE_MAX != 0 {
        64 / T::DECODE_MAX
    } else {
        1
    };

    if chunk_size >= 2 {
        let chunks = len / chunk_size;
        let remainder = len % chunk_size;

        let mut ret = Vec::with_capacity(len);
        let mut buf = crate::register_buffer::RegisterReader::new(reader);

        for _ in 0..chunks {
            buf.refill()?;
            let r = &mut buf.inner;

            // This avoids checking if allocation is needed for every item for chunks divisible by 8.
            // Adding more impls for other sizes slows down this case for some reason.
            if chunk_size % 8 == 0 {
                for _ in 0..chunk_size / 8 {
                    ret.extend([
                        T::decode(encoding, r)?,
                        T::decode(encoding, r)?,
                        T::decode(encoding, r)?,
                        T::decode(encoding, r)?,
                        T::decode(encoding, r)?,
                        T::decode(encoding, r)?,
                        T::decode(encoding, r)?,
                        T::decode(encoding, r)?,
                    ])
                }
            } else {
                for _ in 0..chunk_size {
                    ret.push(T::decode(encoding, r)?)
                }
            }
        }

        buf.refill()?;
        for _ in 0..remainder {
            ret.push(T::decode(encoding, &mut buf.inner)?);
        }
        buf.advance_reader();

        Ok(ret)
    } else {
        // This is faster than extend for some reason.
        let mut vec = Vec::with_capacity(len);
        for _ in 0..len {
            // Avoid generating allocation logic in push (we've allocated enough capacity).
            if vec.len() == vec.capacity() {
                panic!();
            }
            vec.push(T::decode(encoding, reader)?);
        }
        Ok(vec)
    }
}

impl<const N: usize, T: Encode> Encode for [T; N] {
    const ENCODE_MIN: usize = T::ENCODE_MIN * N;
    const ENCODE_MAX: usize = T::ENCODE_MAX.saturating_mul(N);

    fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
        encode_elements(self, encoding, writer)
    }
}

impl<const N: usize, T: Decode> Decode for [T; N] {
    const DECODE_MIN: usize = T::DECODE_MIN * N;
    const DECODE_MAX: usize = T::DECODE_MAX.saturating_mul(N);

    fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
        // TODO find a safe way to decode an array without allocating.
        // Maybe use ArrayVec, but that would require another dep.
        Ok(decode_elements(N, encoding, reader)?
            .try_into()
            .ok()
            .unwrap())
    }
}

// Blocked TODO: https://github.com/rust-lang/rust/issues/37653
//
// Implement faster encoding of &[u8] or more generally any &[bytemuck::Pod] that encodes the same.
impl<T: Encode> Encode for [T] {
    const ENCODE_MIN: usize = 1;
    // [()] max bits is 127 (gamma of u64::MAX - 1).
    const ENCODE_MAX: usize = (T::ENCODE_MAX.saturating_mul(usize::MAX)).saturating_add(127);

    fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
        self.len().encode(Gamma, writer)?;
        encode_elements(self, encoding, writer)
    }
}

impl<T: Encode> Encode for Vec<T> {
    impl_enc_same!([T]);

    fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
        self.as_slice().encode(encoding, writer)
    }
}

// Blocked TODO: https://github.com/rust-lang/rust/issues/37653
//
// Implement faster decoding of Vec<u8> or more generally any Vec<bytemuck::Pod> that encodes the same.
impl<T: Decode> Decode for Vec<T> {
    const DECODE_MIN: usize = 1;
    // Vec<()> max bits is 127 (gamma of u64::MAX - 1).
    const DECODE_MAX: usize = (T::DECODE_MAX.saturating_mul(usize::MAX)).saturating_add(127);

    fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
        let len = usize::decode(Gamma, reader)?;
        guard_len::<T>(len, encoding, reader)?;
        decode_elements(len, encoding, reader)
    }
}

macro_rules! impl_iter_encode {
    ($item:ty) => {
        impl_enc_same!([$item]);
        fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
            self.len().encode(Gamma, writer)?;
            for t in self {
                t.encode(encoding, writer)?;
            }
            Ok(())
        }
    };
}

macro_rules! impl_collection {
    ($collection: ident $(,$bound: ident)*) => {
        impl<T: Encode $(+ $bound)*> Encode for std::collections::$collection<T> {
            impl_iter_encode!(T);
        }

        impl<T: Decode $(+ $bound)*> Decode for std::collections::$collection<T> {
            impl_dec_same!(Vec<T>);

            fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
                let len = usize::decode(Gamma, reader)?;
                guard_len::<T>(len, encoding, reader)?;

                (0..len).map(|_| T::decode(encoding, reader)).collect()
            }
        }
    }
}

impl_collection!(BTreeSet, Ord);
impl_collection!(LinkedList);

// Some collections can be efficiently created from a Vec such as BinaryHeap/VecDeque.
macro_rules! impl_collection_decode_from_vec {
    ($collection: ident $(,$bound: ident)*) => {
        impl<T: Decode $(+ $bound)*> Decode for std::collections::$collection<T> {
            impl_dec_same!(Vec<T>);

            fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
                Ok(Vec::decode(encoding, reader)?.into())
            }
        }
    }
}

impl<T: Encode> Encode for std::collections::VecDeque<T> {
    impl_enc_same!([T]);

    fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
        self.len().encode(Gamma, writer)?;
        let (a, b) = self.as_slices();
        encode_elements(a, encoding, writer)?;
        encode_elements(b, encoding, writer)
    }
}
impl_collection_decode_from_vec!(VecDeque);

impl<T: Encode + Ord> Encode for std::collections::BinaryHeap<T> {
    // TODO optimize with encode_elements(binary_heap.as_slice(), ..) once it's stable.
    impl_iter_encode!(T);
}
impl_collection_decode_from_vec!(BinaryHeap, Ord);

impl Encode for str {
    impl_enc_same!([u8]);

    #[inline(always)]
    fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
        encoding.write_str(writer, self);
        Ok(())
    }
}

impl Encode for String {
    impl_enc_same!(str);

    #[inline(always)]
    fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
        self.as_str().encode(encoding, writer)
    }
}

impl Decode for String {
    impl_dec_from_enc!();

    #[inline(always)]
    fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
        Ok(encoding.read_str(reader)?.to_owned())
    }
}

impl Encode for CStr {
    impl_enc_same!(str);

    #[inline(always)]
    fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
        encoding.write_byte_str(writer, self.to_bytes());
        Ok(())
    }
}

impl Encode for CString {
    impl_enc_same!(CStr);

    #[inline(always)]
    fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
        self.as_c_str().encode(encoding, writer)
    }
}

impl Decode for CString {
    impl_dec_from_enc!();

    #[inline(always)]
    fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
        CString::new(encoding.read_byte_str(reader)?).map_err(|_| E::Invalid("CString").e())
    }
}

impl<K: Encode, V: Encode> Encode for BTreeMap<K, V> {
    impl_iter_encode!((K, V));
}

impl<K: Decode + Ord, V: Decode> Decode for BTreeMap<K, V> {
    impl_dec_same!(Vec<(K, V)>);

    fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
        let len = usize::decode(Gamma, reader)?;
        guard_len::<(K, V)>(len, encoding, reader)?;

        // Collect is faster than insert for BTreeMap since it can add the items in bulk once it
        // ensures they are sorted.
        (0..len)
            .map(|_| <(K, V)>::decode(encoding, reader))
            .collect()
    }
}

impl<K: Encode, V: Encode, S> Encode for HashMap<K, V, S> {
    impl_iter_encode!((K, V));
}

impl<K: Decode + Hash + Eq, V: Decode, S: BuildHasher + Default> Decode for HashMap<K, V, S> {
    impl_dec_same!(Vec<(K, V)>);

    fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
        let len = usize::decode(Gamma, reader)?;
        guard_len::<(K, V)>(len, encoding, reader)?;

        // Insert is faster than collect for HashMap since it only reserves size_hint / 2 in collect.
        let mut map = Self::with_capacity_and_hasher(len, Default::default());
        for _ in 0..len {
            let (k, v) = <(K, V)>::decode(encoding, reader)?;
            map.insert(k, v);
        }
        Ok(map)
    }
}

impl<T: Encode, S> Encode for HashSet<T, S> {
    impl_iter_encode!(T);
}

impl<T: Decode + Hash + Eq, S: BuildHasher + Default> Decode for HashSet<T, S> {
    impl_dec_same!(Vec<T>);

    fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
        let len = usize::decode(Gamma, reader)?;
        guard_len::<T>(len, encoding, reader)?;

        // Insert is faster than collect for HashSet since it only reserves size_hint / 2 in collect.
        let mut set = Self::with_capacity_and_hasher(len, Default::default());
        for _ in 0..len {
            set.insert(T::decode(encoding, reader)?);
        }
        Ok(set)
    }
}

macro_rules! impl_ipvx_addr {
    ($addr:ident, $bytes:expr, $int:ty) => {
        impl Encode for $addr {
            impl_enc_const!($bytes * u8::BITS as usize);

            #[inline(always)]
            fn encode(&self, _: impl Encoding, writer: &mut impl Write) -> Result<()> {
                <$int>::from_le_bytes(self.octets()).encode(Fixed, writer)
            }
        }

        impl Decode for $addr {
            impl_dec_from_enc!();

            #[inline(always)]
            fn decode(_: impl Encoding, reader: &mut impl Read) -> Result<Self> {
                Ok(Self::from(
                    <$int as Decode>::decode(Fixed, reader)?.to_le_bytes(),
                ))
            }
        }
    };
}

impl_ipvx_addr!(Ipv4Addr, 4, u32);
impl_ipvx_addr!(Ipv6Addr, 16, u128);
impl_either!(IpAddr, V4, Ipv4Addr, V6, Ipv6Addr);

macro_rules! impl_socket_addr_vx {
    ($addr:ident, $ip_addr:ident, $bytes:expr $(,$extra: expr)*) => {
        impl Encode for $addr {
            impl_enc_const!(($bytes) * u8::BITS as usize);

            fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
                optimized_enc!(encoding, writer);
                enc!(self.ip(), $ip_addr);
                enc!(self.port(), u16);
                end_enc!();
                Ok(())
            }
        }

        impl Decode for $addr {
            impl_dec_from_enc!();

            fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
                optimized_dec!(encoding, reader);
                dec!(ip, $ip_addr);
                dec!(port, u16);
                end_dec!();
                Ok(Self::new(
                    ip,
                    port
                    $(,$extra)*
                ))
            }
        }
    }
}

impl_socket_addr_vx!(SocketAddrV4, Ipv4Addr, 4 + 2);
impl_socket_addr_vx!(SocketAddrV6, Ipv6Addr, 16 + 2, 0, 0);
impl_either!(SocketAddr, V4, SocketAddrV4, V6, SocketAddrV6);

impl Encode for Duration {
    impl_enc_const!(94); // 64 bits seconds + 30 bits nanoseconds

    fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
        encoding.write_u128::<{ Self::ENCODE_MAX }>(writer, self.as_nanos());
        Ok(())
    }
}

impl Decode for Duration {
    impl_dec_from_enc!();

    fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
        let nanos = encoding.read_u128::<{ Self::DECODE_MAX }>(reader)?;

        // Manual implementation of Duration::from_nanos since it takes a u64 instead of a u128.
        const NANOS_PER_SEC: u128 = Duration::new(1, 0).as_nanos();
        let secs = (nanos / NANOS_PER_SEC)
            .try_into()
            .map_err(|_| E::Invalid("Duration").e())?;
        Ok(Duration::new(secs, (nanos % NANOS_PER_SEC) as u32))
    }
}

impl<T> Encode for PhantomData<T> {
    impl_enc_const!(0);

    fn encode(&self, _: impl Encoding, _: &mut impl Write) -> Result<()> {
        Ok(())
    }
}

impl<T> Decode for PhantomData<T> {
    impl_dec_from_enc!();

    fn decode(_: impl Encoding, _: &mut impl Read) -> Result<Self> {
        Ok(PhantomData)
    }
}

// TODO maybe Atomic*, Bound, Cell, Range, RangeInclusive, SystemTime.

// Allows `&str` and `&[T]` to implement encode.
impl<'a, T: Encode + ?Sized> Encode for &'a T {
    impl_enc_same!(T);

    fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
        T::encode(self, encoding, writer)
    }
}

impl Encode for () {
    impl_enc_const!(0);

    fn encode(&self, _: impl Encoding, _: &mut impl Write) -> Result<()> {
        Ok(())
    }
}

impl Decode for () {
    impl_dec_from_enc!();

    fn decode(_: impl Encoding, _: &mut impl Read) -> Result<Self> {
        Ok(())
    }
}

macro_rules! impl_tuples {
    ($($len:expr => ($($n:tt $name:ident)+))+) => {
        $(
            impl<$($name),+> Encode for ($($name,)+)
            where
                $($name: Encode,)+
            {
                const ENCODE_MIN: usize = $(<$name>::ENCODE_MIN +)+ 0;
                const ENCODE_MAX: usize = 0usize $(.saturating_add(<$name>::ENCODE_MAX))+;

                #[cfg_attr(not(debug_assertions), inline(always))]
                fn encode(&self, encoding: impl Encoding, writer: &mut impl Write) -> Result<()> {
                    optimized_enc!(encoding, writer);
                    $(
                        enc!(self.$n, $name);
                    )+
                    end_enc!();
                    Ok(())
                }
            }

            impl<$($name),+> Decode for ($($name,)+)
            where
                $($name: Decode,)+
            {
                const DECODE_MIN: usize = $(<$name>::DECODE_MIN +)+ 0;
                const DECODE_MAX: usize = 0usize $(.saturating_add(<$name>::DECODE_MAX))+;

                #[allow(non_snake_case)]
                #[cfg_attr(not(debug_assertions), inline(always))]
                fn decode(encoding: impl Encoding, reader: &mut impl Read) -> Result<Self> {
                    optimized_dec!(encoding, reader);
                    $(
                        dec!($name, $name);
                    )+
                    end_dec!();
                    Ok(($($name,)+))
                }
            }
        )+
    }
}

impl_tuples! {
    1 => (0 T0)
    2 => (0 T0 1 T1)
    3 => (0 T0 1 T1 2 T2)
    4 => (0 T0 1 T1 2 T2 3 T3)
    5 => (0 T0 1 T1 2 T2 3 T3 4 T4)
    6 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5)
    7 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6)
    8 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7)
    9 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8)
    10 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9)
    11 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10)
    12 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11)
    13 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11 12 T12)
    14 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11 12 T12 13 T13)
    15 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11 12 T12 13 T13 14 T14)
    16 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11 12 T12 13 T13 14 T14 15 T15)
}

#[cfg(all(test, not(miri)))]
mod tests {
    use paste::paste;
    use std::net::*;
    use std::time::Duration;
    use test::{black_box, Bencher};

    macro_rules! bench {
        ($name:ident, $t:ty, $v:expr) => {
            paste! {
                #[bench]
                fn [<bench_ $name _encode>](b: &mut Bencher) {
                    let mut buffer = crate::Buffer::new();
                    let v = vec![$v; 1000];
                    let _ = buffer.encode(&v).unwrap();

                    b.iter(|| {
                        let v = black_box(v.as_slice());
                        let bytes = buffer.encode(v).unwrap();
                        black_box(bytes);
                    })
                }

                #[bench]
                fn [<bench_ $name _decode>](b: &mut Bencher) {
                    let mut buffer = crate::Buffer::new();
                    let v = vec![$v; 1000];

                    let bytes = buffer.encode(&v).unwrap().to_vec();
                    let decoded: Vec<$t> = buffer.decode(&bytes).unwrap();
                    assert_eq!(v, decoded);

                    b.iter(|| {
                        let bytes = black_box(bytes.as_slice());
                        black_box(buffer.decode::<Vec<$t>>(bytes).unwrap())
                    })
                }
            }
        };
    }

    bench!(char, char, 'a'); // TODO bench on random chars.
    bench!(duration, Duration, Duration::new(123, 456));
    bench!(ipv4_addr, Ipv4Addr, Ipv4Addr::from([1, 2, 3, 4]));
    bench!(ipv6_addr, Ipv6Addr, Ipv6Addr::from([4; 16]));
    bench!(
        socket_addr_v4,
        SocketAddrV4,
        SocketAddrV4::new(Ipv4Addr::from([1, 2, 3, 4]), 1234)
    );
    bench!(
        socket_addr_v6,
        SocketAddrV6,
        SocketAddrV6::new(Ipv6Addr::from([4; 16]), 1234, 0, 0)
    );

    macro_rules! bench_map_or_set {
        ($name:ident, $t:ty, $f:expr) => {
            paste! {
                #[bench]
                fn [<bench_ $name _encode>](b: &mut Bencher) {
                    let mut buffer = crate::Buffer::new();
                    let v = $t::from_iter((0u16..1000).map($f));
                    let _ = buffer.encode(&v).unwrap();

                    b.iter(|| {
                        let v = black_box(&v);
                        let bytes = buffer.encode(v).unwrap();
                        black_box(bytes);
                    })
                }

                #[bench]
                fn [<bench_ $name _decode>](b: &mut Bencher) {
                    let mut buffer = crate::Buffer::new();
                    let v = $t::from_iter((0u16..1000).map($f));

                    let bytes = buffer.encode(&v).unwrap().to_vec();
                    let decoded: $t = buffer.decode(&bytes).unwrap();
                    assert_eq!(v, decoded);

                    b.iter(|| {
                        let bytes = black_box(bytes.as_slice());
                        black_box(buffer.decode::<$t>(bytes).unwrap())
                    })
                }
            }
        };
    }

    macro_rules! bench_map {
        ($name:ident, $t:ident) => {
            bench_map_or_set!($name, std::collections::$t::<u16, u16>, |v| (v, v));
        };
    }
    bench_map!(btree_map, BTreeMap);
    bench_map!(hash_map, HashMap);

    macro_rules! bench_set {
        ($name:ident, $t:ident) => {
            bench_map_or_set!($name, std::collections::$t::<u16>, |v| v);
        };
    }
    bench_set!(btree_set, BTreeSet);
    bench_set!(hash_set, HashSet);
}
