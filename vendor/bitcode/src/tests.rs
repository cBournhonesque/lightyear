use crate::code::{decode_internal, encode_internal};
use crate::serde::de::deserialize_internal;
use crate::serde::ser::serialize_internal;
use crate::word_buffer::WordBuffer;
use crate::{Buffer, Decode, Encode, E};
use paste::paste;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::fmt::Debug;

#[cfg(not(miri))]
use crate::bit_buffer::BitBuffer;

#[test]
fn test_buffer_with_capacity() {
    assert_eq!(Buffer::with_capacity(0).capacity(), 0);

    let mut buf = Buffer::with_capacity(1016);
    let enough_cap = buf.capacity();
    let bytes = buf.serialize(&"a".repeat(997 + 16)).unwrap();
    assert_eq!(bytes.len(), enough_cap);
    assert_eq!(buf.capacity(), enough_cap);

    let mut buf = Buffer::with_capacity(1016);
    let small_cap = buf.capacity();
    let bytes = buf.serialize(&"a".repeat(997 + 19)).unwrap();
    assert_ne!(bytes.len(), small_cap);
    assert_ne!(buf.capacity(), small_cap);
}

macro_rules! impl_the_same {
    ($ser_trait:ident, $de_trait:ident, $ser:ident, $de:ident) => {
        paste! {
            fn [< the_same_ $ser>] <T: Clone + Debug + PartialEq + $ser_trait + $de_trait>(
                t: T,
                buf: &mut Buffer,
            ) {
                let serialized = {
                    let a = [<$ser _internal>](&mut WordBuffer::default(), &t)
                        .unwrap()
                        .to_vec();
                    let b = buf.$ser(&t).unwrap().to_vec();
                    assert_eq!(a, b);

                    #[cfg(not(miri))]
                    {
                        let c = [<$ser _internal>](&mut BitBuffer::default(), &t)
                            .unwrap()
                            .to_vec();
                        assert_eq!(a, c);
                    }
                    a
                };

                let a: T =
                [<$de _internal>](&mut WordBuffer::default(), &serialized).expect("WordBuffer error");
                let b: T = buf
                    .$de(&serialized)
                    .expect("Buffer::deserialize error");

                assert_eq!(t, a);
                assert_eq!(t, b);

                #[cfg(not(miri))]
                {
                    let c: T =
                        [<$de _internal>](&mut BitBuffer::default(), &serialized).expect("BitBuffer error");
                    assert_eq!(t, c);
                }

                let mut bytes = serialized.clone();
                bytes.push(0);
                #[cfg(not(miri))]
                assert_eq!(
                    [<$de _internal>]::<BitBuffer, T>(&mut Default::default(), &bytes),
                    Err(E::ExpectedEof.e())
                );
                assert_eq!(
                    [<$de _internal>]::<WordBuffer, T>(&mut Default::default(), &bytes),
                    Err(E::ExpectedEof.e())
                );

                let mut bytes = serialized.clone();
                if bytes.pop().is_some() {
                    #[cfg(not(miri))]
                    assert_eq!(
                        [<$de _internal>]::<BitBuffer, T>(&mut Default::default(), &bytes),
                        Err(E::Eof.e())
                    );
                    assert_eq!(
                        [<$de _internal>]::<WordBuffer, T>(&mut Default::default(), &bytes),
                        Err(E::Eof.e())
                    );
                }
            }
        }
    }
}

impl_the_same!(Serialize, DeserializeOwned, serialize, deserialize);
impl_the_same!(Encode, Decode, encode, decode);

fn the_same_once<T: Clone + Debug + PartialEq + Encode + Decode + Serialize + DeserializeOwned>(
    t: T,
) {
    let mut buf = Buffer::new();
    the_same_serialize(t.clone(), &mut buf);
    the_same_encode(t, &mut buf);
}

fn the_same<T: Clone + Debug + PartialEq + Encode + Decode + Serialize + DeserializeOwned>(t: T) {
    the_same_once(t.clone());

    let mut buf = Buffer::new();

    #[cfg(miri)]
    const END: usize = 2;
    #[cfg(not(miri))]
    const END: usize = 65;
    for i in 0..END {
        let input = vec![t.clone(); i];
        the_same_serialize(input.clone(), &mut buf);
        the_same_encode(input, &mut buf);
    }
}

#[test]
fn fuzz1() {
    let bytes = &[64];
    assert!(crate::decode::<Vec<i64>>(bytes).is_err());
    assert!(crate::serde::deserialize::<Vec<i64>>(bytes).is_err());
}

#[test]
fn fuzz2() {
    let bytes = &[0, 0, 0, 1];
    assert!(crate::decode::<Vec<u8>>(bytes).is_err());
    assert!(crate::serde::deserialize::<Vec<u8>>(bytes).is_err());
}

#[test]
fn fuzz3() {
    use bitvec::prelude::*;

    #[rustfmt::skip]
    let bits = bitvec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let mut bits2 = BitVec::<u8, Lsb0>::new();
    bits2.extend_from_bitslice(&bits);
    let bytes = bits2.as_raw_slice();

    assert!(crate::decode::<Vec<()>>(bytes).is_err());
    assert!(crate::serde::deserialize::<Vec<()>>(bytes).is_err());
}

#[test]
fn test_reddit() {
    #[derive(Serialize)]
    #[allow(dead_code)]
    enum Variant {
        Three = 3,
        Zero = 0,
        Two = 2,
        One = 1,
    }

    assert_eq!(crate::serde::serialize(&Variant::Three).unwrap().len(), 1);
}

#[test]
fn test_zst_vec() {
    for i in (0..100).step_by(9) {
        the_same(vec![(); i]);
    }
}

#[test]
fn test_long_string() {
    the_same("abcde".repeat(25))
}

#[test]
fn test_array_string() {
    use arrayvec::ArrayString;

    // Serialize one field has with serde.
    #[derive(Clone, Debug, PartialEq, Encode, Decode, Serialize, Deserialize)]
    struct MyStruct1<const N: usize> {
        #[bitcode_hint(ascii_lowercase)]
        #[bitcode(with_serde)]
        inner: ArrayString<N>,
        #[bitcode_hint(gamma)]
        foo: i32,
    }

    for i in 0..=20 {
        let short = MyStruct1 {
            inner: ArrayString::<20>::from(&"a".repeat(i)).unwrap(),
            foo: 5,
        };
        the_same_once(short);
    }

    // not ascii_lowercase
    let short = MyStruct1 {
        inner: ArrayString::<5>::from(&"A".repeat(5)).unwrap(),
        foo: 5,
    };
    the_same_once(short);

    // Serialize whole struct with serde.
    #[derive(Clone, Debug, PartialEq, Encode, Decode, Serialize, Deserialize)]
    #[bitcode_hint(ascii)]
    #[bitcode(with_serde)]
    struct MyStruct2<const N: usize> {
        inner: ArrayString<N>,
    }

    let long = MyStruct2 {
        inner: ArrayString::<150>::from(&"abcde".repeat(30)).unwrap(),
    };
    the_same_once(long);

    // Serialize whole variant with serde.
    #[derive(Clone, Debug, PartialEq, Encode, Decode, Serialize, Deserialize)]
    enum MyEnum<const N: usize> {
        #[bitcode(with_serde)]
        A(ArrayString<N>),
    }

    let medium = MyEnum::A(ArrayString::<25>::from(&"abcde".repeat(5)).unwrap());
    the_same_once(medium);
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_zst() {
    use crate::guard::ZST_LIMIT;
    fn is_ok<T: Serialize + DeserializeOwned + Encode + Decode>(v: Vec<T>) -> bool {
        let ser = crate::serialize(&v).unwrap();
        let a = crate::deserialize::<Vec<T>>(&ser).is_ok();
        let b = crate::decode::<Vec<T>>(&ser).is_ok();
        assert_eq!(a, b);
        b
    }
    assert!(is_ok(vec![0u8; ZST_LIMIT]));
    assert!(is_ok(vec![0u8; ZST_LIMIT]));
    assert!(!is_ok(vec![(); ZST_LIMIT + 1]));
    assert!(is_ok(vec![0u8; ZST_LIMIT + 1]));
}

#[test]
fn test_chars() {
    #[cfg(not(miri))]
    const STEP: usize = char::MAX as usize / 1000;

    #[cfg(miri)]
    const STEP: usize = char::MAX as usize / 100;

    let chars = (0..=char::MAX as u32)
        .step_by(STEP)
        .filter_map(char::from_u32)
        .collect::<Vec<_>>();
    the_same_once(chars);
}

#[test]
fn test_char1() {
    let c = char::from_u32(11141).unwrap();
    the_same(c)
}

#[test]
fn test_expected_range() {
    #[derive(PartialEq, Debug, Clone, Serialize, Deserialize, Encode, Decode)]
    struct LargeU64(#[bitcode_hint(expected_range = "10..1000000000")] u64);

    let mut i = 0;
    loop {
        the_same_once(LargeU64(i));
        if let Some(new) = i.checked_add(1).and_then(|i| i.checked_mul(2)) {
            i = new;
        } else {
            break;
        }
    }
}

#[test]
fn test_weird_tuple() {
    let value = (1u8, Option::<()>::None);
    println!(
        "{} {:?}",
        <(u8, Option<()>)>::DECODE_MIN,
        crate::encode(&value).unwrap()
    );
    the_same(value);
}

#[test]
fn test_gamma_bytes() {
    #[derive(Encode, Decode, Serialize, Deserialize, PartialEq, Debug, Clone)]
    #[bitcode_hint(gamma)]
    struct Bytes(Vec<u8>);

    the_same_once(Bytes(vec![0u8; 20]));
    the_same_once(Bytes(vec![255u8; 20]));
}

#[test]
fn test_name_conflict() {
    mod decode {
        #[allow(unused_imports)]
        use musli::Decode;

        #[derive(bitcode::Decode)]
        struct Struct {
            #[allow(unused)]
            field: u64,
        }
    }

    mod encode {
        #[allow(unused_imports)]
        use musli::Encode;

        #[derive(bitcode::Encode)]
        struct Struct {
            #[allow(unused)]
            field: u64,
        }
    }
}

#[test]
fn test_c_string() {
    the_same_once(CString::new(vec![]).unwrap());
    the_same_once(CString::new((1..=255).collect::<Vec<_>>()).unwrap());

    let bytes = vec![1, 2, 3, 255, 0];
    let c_str = CStr::from_bytes_with_nul(&bytes).unwrap();
    let encoded = crate::encode(c_str).unwrap();
    let decoded = crate::decode::<CString>(&encoded).unwrap();
    assert_eq!(decoded.as_c_str(), c_str)
}

#[test]
fn test_numbers_extra() {
    macro_rules! test {
        ($t:ident) => {
            the_same(5 as $t);
            the_same($t::MAX - 5);
            the_same($t::MAX);
        };
    }

    test!(u64);
    test!(u128);

    macro_rules! test_signed {
        ($t:ident) => {
            test!($t);
            the_same(-5 as $t);
            the_same($t::MIN);
            the_same($t::MIN + 5);
        };
    }

    test_signed!(i64);
    test_signed!(i128);
}

// Everything below this comment was derived from bincode:
// https://github.com/bincode-org/bincode/blob/v1.x/tests/test.rs

#[test]
fn test_numbers() {
    // unsigned positive
    the_same(5u8);
    the_same(5u16);
    the_same(5u32);
    the_same(5u64);
    the_same(5usize);
    // signed positive
    the_same(5i8);
    the_same(5i16);
    the_same(5i32);
    the_same(5i64);
    the_same(5isize);
    // signed negative
    the_same(-5i8);
    the_same(-5i16);
    the_same(-5i32);
    the_same(-5i64);
    the_same(-5isize);
    // floating
    the_same(-100f32);
    the_same(0f32);
    the_same(5f32);
    the_same(-100f64);
    the_same(5f64);
}

#[test]
fn test_string() {
    the_same("".to_string());
    the_same("a".to_string());
}

#[test]
fn test_tuple() {
    the_same((1isize,));
    the_same((1isize, 2isize, 3isize));
    the_same((1isize, "foo".to_string(), ()));
}

#[test]
fn test_basic_struct() {
    #[derive(Encode, Decode, Serialize, Deserialize, PartialEq, Debug, Clone)]
    struct Easy {
        x: isize,
        s: String,
        y: usize,
    }
    the_same(Easy {
        x: -4,
        s: "foo".to_string(),
        y: 10,
    });
}

#[test]
fn test_nested_struct() {
    #[derive(Encode, Decode, Serialize, Deserialize, PartialEq, Debug, Clone)]
    struct Easy {
        x: isize,
        s: String,
        y: usize,
    }
    #[derive(Encode, Decode, Serialize, Deserialize, PartialEq, Debug, Clone)]
    struct Nest {
        f: Easy,
        b: usize,
        s: Easy,
    }

    the_same(Nest {
        f: Easy {
            x: -1,
            s: "foo".to_string(),
            y: 20,
        },
        b: 100,
        s: Easy {
            x: -100,
            s: "bar".to_string(),
            y: 20,
        },
    });
}

#[test]
fn test_struct_newtype() {
    #[derive(Encode, Decode, Serialize, Deserialize, PartialEq, Debug, Clone)]
    struct NewtypeStr(usize);

    the_same(NewtypeStr(5));
}

#[test]
fn test_struct_tuple() {
    #[derive(Encode, Decode, Serialize, Deserialize, PartialEq, Debug, Clone)]
    struct TubStr(usize, String, f32);

    the_same(TubStr(5, "hello".to_string(), 3.2));
}

#[test]
fn test_option() {
    the_same(Some(5usize));
    the_same(Some("foo bar".to_string()));
    the_same(None::<usize>);
}

#[test]
fn test_enum() {
    #[derive(Encode, Decode, Serialize, Deserialize, PartialEq, Debug, Clone)]
    enum TestEnum {
        NoArg,
        OneArg(usize),
        Args(usize, usize),
        AnotherNoArg,
        StructLike { x: usize, y: f32 },
    }
    the_same(TestEnum::NoArg);
    the_same(TestEnum::OneArg(4));
    the_same(TestEnum::Args(4, 5));
    the_same(TestEnum::AnotherNoArg);
    the_same(TestEnum::StructLike {
        x: 4,
        y: std::f32::consts::PI,
    });
    the_same(vec![
        TestEnum::NoArg,
        TestEnum::OneArg(5),
        TestEnum::AnotherNoArg,
        TestEnum::StructLike { x: 4, y: 1.4 },
    ]);
}

#[test]
fn test_vec() {
    let v: Vec<u8> = vec![];
    the_same(v);
    the_same(vec![1u64]);
    the_same(vec![1u64, 2, 3, 4, 5, 6]);
}

#[test]
fn test_map() {
    let mut m = HashMap::new();
    m.insert(4u64, "foo".to_string());
    m.insert(0u64, "bar".to_string());
    the_same(m);
}

#[test]
fn test_bool() {
    the_same(true);
    the_same(false);
}

#[test]
fn test_unicode() {
    the_same("å".to_string());
    the_same("aåååååååa".to_string());
}

#[test]
fn test_fixed_size_array() {
    the_same([24u32; 32]);
    the_same([1u64, 2, 3, 4, 5, 6, 7, 8]);
    the_same([0u8; 19]);
}

#[test]
fn expected_range_bug() {
    #[derive(Encode, Decode, Serialize, Deserialize, PartialEq, Debug, Clone)]
    pub struct UVec2 {
        x: u16,
        y: u16,
    }

    #[derive(Encode, Decode, Serialize, Deserialize, PartialEq, Debug, Clone)]
    pub struct Wrapper(#[bitcode_hint(expected_range = "0..31")] UVec2);

    let val = Wrapper(UVec2 { x: 500, y: 512 });
    the_same(val);
}
