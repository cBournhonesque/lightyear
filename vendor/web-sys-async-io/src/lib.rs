#![cfg(target_family = "wasm")]

use wasm_bindgen_futures::JsFuture;

pub mod reader;
mod sys;
pub mod writer;

pub use self::reader::Reader;
pub use self::writer::Writer;

fn js_value_to_io_error(error: wasm_bindgen::JsValue) -> std::io::Error {
    let err: String = js_sys::JSON::stringify(&error)
        .map(Into::into)
        .unwrap_or_default();
    std::io::Error::new(std::io::ErrorKind::Other, err)
}

#[derive(Debug, Default)]
pub enum Op {
    #[default]
    Idle,
    Pending(JsFuture),
}
