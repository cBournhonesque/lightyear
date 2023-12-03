// Exports for derive macro. #[doc(hidden)] because not stable between versions.

pub use crate::code::*;
pub use crate::encoding::*;
pub use crate::nightly::{max, min};
pub use crate::read::Read;
pub use crate::register_buffer::*;
pub use crate::write::Write;
pub use crate::Error;

#[cfg(any(test, feature = "serde"))]
pub use crate::serde::de::deserialize_compat;
#[cfg(any(test, feature = "serde"))]
pub use crate::serde::ser::serialize_compat;
#[cfg(any(test, feature = "serde"))]
pub use serde::{de::DeserializeOwned, Serialize};

// TODO only define once.
pub type Result<T> = std::result::Result<T, Error>;

pub fn invalid_variant() -> Error {
    crate::E::Invalid("enum variant").e()
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use crate::{Decode, Encode};
    use serde::{Deserialize, Serialize};
    use std::marker::PhantomData;

    #[derive(Debug, Default, PartialEq, Encode, Decode)]
    #[bitcode(recursive)]
    struct Recursive {
        a: Option<Box<Recursive>>,
        b: Option<Box<Self>>,
        c: Vec<Self>,
    }

    #[test]
    fn test_recursive() {
        // If these functions aren't called, Rust hides some kinds of compile errors.
        crate::encode(&Recursive::default()).unwrap();
        let _ = crate::decode::<Recursive>(&[]);
    }

    trait ParamTrait {
        type One;
        type Two: Encode + Decode;
        type Three;
        type Four;
    }

    struct Param;

    #[derive(Serialize, Deserialize)]
    struct SerdeU32(u32);

    impl ParamTrait for Param {
        type One = i8;
        type Two = u16;
        type Three = SerdeU32;
        type Four = &'static str;
    }

    #[derive(Encode, Decode)]
    #[bitcode_hint(gamma)]
    struct UsesParamTrait<A: ParamTrait, B: ParamTrait> {
        #[bitcode(bound_type = "B::One")]
        a: Vec<B::One>,
        #[bitcode(bound_type = "A::One")] // Make sure redundant bound_type works.
        b: A::One,
        c: Vec<A::Two>, // Always Encode + Decode so no bound_type needed.
        #[bitcode(with_serde, bound_type = "(A::Three, B::Three)")]
        d: Vec<(A::Three, B::Three)>,
        e: PhantomData<A::Four>,
    }

    #[test]
    fn test_uses_param_trait() {
        type T = UsesParamTrait<Param, Param>;
        let t: T = UsesParamTrait {
            a: vec![1, 2, 3],
            b: 1,
            c: vec![1, 2, 3],
            d: vec![(SerdeU32(1), SerdeU32(2))],
            e: PhantomData,
        };

        let encoded = crate::encode(&t).unwrap();
        let _ = crate::decode::<T>(&encoded).unwrap();
    }

    #[derive(Debug, PartialEq, Encode, Decode)]
    struct Empty;

    #[derive(Debug, PartialEq, Encode, Decode)]
    struct Tuple(usize, u8);

    #[derive(Debug, PartialEq, Encode, Decode)]
    struct Generic<T>(usize, T);

    #[derive(Debug, PartialEq, Encode, Decode)]
    struct FooInner {
        foo: u8,
        #[bitcode_hint(gamma)]
        bar: usize,
        baz: String,
    }

    #[derive(Debug, PartialEq, Encode, Decode)]
    #[allow(unused)]
    enum Foo {
        #[bitcode_hint(frequency = 100)]
        A,
        #[bitcode_hint(frequency = 10)]
        B(String),
        C {
            #[bitcode_hint(gamma)]
            baz: usize,
            qux: f32,
        },
        #[bitcode_hint(fixed)]
        Foo(FooInner, #[bitcode_hint(gamma)] i64),
        #[bitcode_hint(expected_range = "0..10")]
        Tuple(Tuple),
        Empty(Empty),
    }

    #[derive(Encode, Decode)]
    enum Never {}

    #[derive(Copy, Clone, Debug, PartialEq, Encode, Decode)]
    enum Xyz {
        #[bitcode_hint(frequency = 2)]
        X,
        Y,
        Z,
    }

    #[test]
    fn test_encode_x() {
        let v = [Xyz::X; 16];
        let encoded = crate::encode(&v).unwrap();
        assert_eq!(encoded.len(), 2);

        let decoded: [Xyz; 16] = crate::decode(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn test_encode_y() {
        let v = [Xyz::Y; 16];
        let encoded = crate::encode(&v).unwrap();
        assert_eq!(encoded.len(), 4);

        let decoded: [Xyz; 16] = crate::decode(&encoded).unwrap();
        assert_eq!(v, decoded);
    }
}
