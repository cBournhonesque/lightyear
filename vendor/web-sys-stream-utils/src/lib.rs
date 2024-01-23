#![cfg(target_family = "wasm")]

pub mod sys;

pub fn get_reader(
    readable_stream: impl Into<web_sys::ReadableStream>,
) -> web_sys::ReadableStreamDefaultReader {
    let readable_stream = readable_stream.into();
    let reader: wasm_bindgen::JsValue = readable_stream.get_reader().into();
    reader.into()
}

pub fn get_reader_byob(
    readable_stream: impl Into<web_sys::ReadableStream>,
) -> web_sys::ReadableStreamByobReader {
    let readable_stream = readable_stream.into();
    let mut options = web_sys::ReadableStreamGetReaderOptions::new();
    options.mode(web_sys::ReadableStreamReaderMode::Byob);
    let reader: wasm_bindgen::JsValue = readable_stream.get_reader_with_options(&options).into();
    reader.into()
}

pub fn get_writer(
    writable_stream: web_sys::WritableStream,
) -> web_sys::WritableStreamDefaultWriter {
    writable_stream.get_writer().unwrap()
}

pub async fn read(
    reader: &web_sys::ReadableStreamDefaultReader,
) -> Result<Option<Vec<u8>>, wasm_bindgen::JsValue> {
    let fut = wasm_bindgen_futures::JsFuture::from(reader.read());
    let result = fut.await?;
    let result: crate::sys::ReadableStreamDefaultReaderValue = result.into();
    let value = result.value();

    let Some(js_buf) = value else {
        if result.is_done() {
            return Ok(None);
        }
        unreachable!("no value and we are also not done, this should be impossible");
    };

    let vec: Vec<u8> = js_buf.to_vec();

    tracing::info!("{} {}", vec.len(), js_buf.length());

    Ok(Some(vec))
}

pub async fn read_byob(
    reader: &web_sys::ReadableStreamByobReader,
    buf: js_sys::Uint8Array,
) -> Result<Option<js_sys::Uint8Array>, wasm_bindgen::JsValue> {
    // This may look odd, as we are seemingly needlessly forcing the user of
    // the API to do an extra copy due to the use of
    // `read_with_array_buffer_view` and taking a [`js_sys::ArrayBuffer`]
    // as an argument.
    // In reality, accepting a `&mut [u8]` and using `read_with_u8_array` makes
    // it very tempting to pass in just a regular rust-allocated slice, which
    // in turn would pass the whole wasm module memory as a buffer to
    // the reader, effectively breaking everything.
    // This `read_with_u8_array` should actually be marked `unsafe`.
    // As it stand right now, the code still is actually not safe still, but
    // at the very least there is this guard-rail of taking
    // an `js_sys::ArrayBuffer`, which should suggest that user of this API
    // has to allocate a new `Uint8Array`.
    let fut = wasm_bindgen_futures::JsFuture::from(reader.read_with_array_buffer_view(&buf));
    let result = fut.await?;
    let result: crate::sys::ReadableStreamByobReaderValue = result.into();
    let value = result.value();

    let Some(js_buf) = value else {
        if result.is_done() {
            return Ok(None);
        }
        unreachable!("no value and we are also not done, this should be impossible");
    };

    Ok(Some(js_buf))
}

pub async fn write<T>(
    writer: &web_sys::WritableStreamDefaultWriter,
    buf: T,
) -> Result<(), wasm_bindgen::JsValue>
where
    js_sys::Uint8Array: From<T>,
{
    let chunk = js_sys::Uint8Array::from(buf);
    let fut = wasm_bindgen_futures::JsFuture::from(writer.write_with_chunk(chunk.as_ref()));
    fut.await?;
    Ok(())
}
