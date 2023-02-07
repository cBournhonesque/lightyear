cfg_if! {
    if #[cfg(all(target_arch = "wasm32", feature = "wbindgen"))] {
        mod wasm_bindgen;
        pub use self::wasm_bindgen::timer::Timer;
        pub use self::wasm_bindgen::timestamp::Timestamp;
    }
    else if #[cfg(all(target_arch = "wasm32", feature = "mquad"))] {
        mod miniquad;
        pub use self::miniquad::timer::Timer;
        pub use self::miniquad::timestamp::Timestamp;
    }
    else if #[cfg(not(target_arch = "wasm32"))] {
        mod native;
        pub use native::timer::Timer;
        pub use native::timestamp::Timestamp;
    }
}
