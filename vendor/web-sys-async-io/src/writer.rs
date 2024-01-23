use std::{
    future::Future,
    pin::Pin,
    task::{ready, Poll},
};

use wasm_bindgen_futures::JsFuture;

use crate::Op;

#[derive(Debug)]
pub struct Writer {
    pub inner: web_sys::WritableStreamDefaultWriter,
    pub op: Op,
}

impl Writer {
    pub fn new(inner: web_sys::WritableStreamDefaultWriter) -> Self {
        Self {
            inner,
            op: Op::default(),
        }
    }
}

impl tokio::io::AsyncWrite for Writer {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        match self.op {
            Op::Pending(ref mut fut) => {
                let result = ready!(Pin::new(fut).poll(cx));
                self.op = Op::Idle;
                Poll::Ready(
                    result
                        .map(|_| buf.len())
                        .map_err(super::js_value_to_io_error),
                )
            }
            Op::Idle => {
                let chunk = js_sys::Uint8Array::from(buf);
                let fut = JsFuture::from(self.inner.write_with_chunk(chunk.as_ref()));
                self.op = Op::Pending(fut);
                self.poll_write(cx, buf)
            }
        }
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        match self.op {
            Op::Pending(ref mut fut) => {
                let result = ready!(Pin::new(fut).poll(cx));
                self.op = Op::Idle;
                Poll::Ready(result.map(|_| ()).map_err(super::js_value_to_io_error))
            }
            Op::Idle => {
                let fut = JsFuture::from(self.inner.ready());
                self.op = Op::Pending(fut);
                self.poll_flush(cx)
            }
        }
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        match self.op {
            Op::Pending(ref mut fut) => {
                let result = ready!(Pin::new(fut).poll(cx));
                self.op = Op::Idle;
                Poll::Ready(result.map(|_| ()).map_err(super::js_value_to_io_error))
            }
            Op::Idle => {
                let fut = JsFuture::from(self.inner.close());
                self.op = Op::Pending(fut);
                self.poll_shutdown(cx)
            }
        }
    }
}
