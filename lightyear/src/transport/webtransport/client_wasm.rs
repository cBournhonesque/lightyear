#![cfg(target_family = "wasm")]
//! WebTransport client implementation.
use super::MTU;
use crate::transport::{PacketReceiver, PacketSender, Transport};
use std::net::SocketAddr;
use std::collections::VecDeque;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tracing::{debug, error, info, trace};
use web_sys::{
    js_sys::{Array, Uint8Array},
    WebTransportHash,
};

use wasm_bindgen::{convert::IntoWasmAbi, prelude::*};

use base64::prelude::{Engine as _, BASE64_STANDARD};

// this could be a module maybe?
#[wasm_bindgen(inline_js = "export class WebTransportWrapper {
    constructor(url, serverCertificateHashes, readyCB, receiveCB, closedCB) {
        this.transport = new WebTransport(url, {
            serverCertificateHashes
        });

        this.transport.ready.then(async () => {
            this.writer = this.transport.datagrams.writable.getWriter();

            for(const buffer of this.sendQueue) this.write(buffer);
            
            readyCB(); // we are ready to send

            const reader = this.transport.datagrams.readable.getReader();

            while(true) {
                const { value, done } = await reader.read();
                if(done) break;
                // value is assumed to be of type Uint8Array
                // if not read from, the unbounded channel on the wasm side will grow infinitely!
                receiveCB(value);
            }
        });

        this.transport.closed.then(e => {
            this.writer = undefined;

            closedCB(e.closeCode, e.reason);
        });

        this.sendQueue = [];
    }

    /**
     * Closes the webtransport connection (the closed callback still proceeds as usual though!)
     * @param {number?} closeCode 
     * @param {string?} reason 
     */
    close(closeCode, reason) {
        this.transport.close({
            closeCode,
            reason
        });
    }

    /**
     * Sends a datagram (there are no additional check to save performance!)
     * @param {Uint8Array} buffer 
     */
    write(buffer) {
        // buffer is assumed to be of type Uint8Array
        if(this.writer) this.writer.write(buffer);
        else this.sendQueue.push(buffer);
    }
}")]
extern "C" {
    type WebTransportWrapper;

    #[wasm_bindgen(constructor)]
    fn new(
        url: String,
        server_certificate_hashes: Array,
        ready: &Closure<dyn FnMut()>,
        recv: &Closure<dyn FnMut(Uint8Array)>,
        closed: &Closure<dyn FnMut(Option<usize>, Option<String>)>,
    ) -> WebTransportWrapper;

    #[wasm_bindgen(method)]
    fn close(this: &WebTransportWrapper, code: Option<usize>, reason: Option<String>);

    #[wasm_bindgen(method)]
    fn write(this: &WebTransportWrapper, buffer: Uint8Array);
}

impl PacketSender for WebTransportWrapper {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> std::io::Result<()> {
        let arr = Uint8Array::new_with_length(payload.len().try_into().unwrap());
        arr.copy_from(payload);
        self.write(arr);
        Ok(())
    }
}

unsafe impl Send for WebTransportWrapper {}
unsafe impl Sync for WebTransportWrapper {}

#[derive(Debug)]
pub struct PacketQueue {
    server_addr: SocketAddr,
    pub queue: UnboundedReceiver<Vec<u8>>,
    buffer: [u8; MTU],
}

impl PacketReceiver for PacketQueue {
    fn recv(&mut self) -> std::io::Result<Option<(&mut [u8], SocketAddr)>> {
        if let Ok(packet) = self.queue.try_recv() {
            let data = packet.as_slice();
            self.buffer[..data.len()].copy_from_slice(data);
            Ok(Some((&mut self.buffer[..data.len()], self.server_addr)))
        } else {
            Ok(None)
        }
    }
}

/// WebTransport client socket
pub struct WebTransportClientSocket {
    client_addr: SocketAddr,
    server_addr: SocketAddr,
    certificate_digest: String,
}

impl WebTransportClientSocket {
    pub fn new(
        client_addr: SocketAddr,
        server_addr: SocketAddr,
        certificate_digest: String,
    ) -> Self {
        Self {
            client_addr,
            server_addr,
            certificate_digest,
        }
    }
}

impl Transport for WebTransportClientSocket {
    fn local_addr(&self) -> SocketAddr {
        self.client_addr
    }

    fn listen(self) -> (Box<dyn PacketSender>, Box<dyn PacketReceiver>) {
        let client_addr = self.client_addr;
        let server_addr = self.server_addr;

        let server_url = format!("https://{}", server_addr);
        info!(
            "Starting client webtransport task with server url: {}",
            &server_url
        );

        let hashes = Array::new();
        let certificate_digests = [&self.certificate_digest]
            .into_iter()
            .map(|x| ring::test::from_hex(x).unwrap())
            .collect::<Vec<_>>();
        for hash in certificate_digests.into_iter() {
            let digest = Uint8Array::from(hash.as_slice());
            let mut jshash = WebTransportHash::new();
            jshash.algorithm("sha-256").value(&digest);
            hashes.push(&jshash);
        }

        let (receiver, queue) = unbounded_channel::<Vec<u8>>();

        let mut is_ready = false;
        let mut is_closed = false;
        let queue = PacketQueue {
            server_addr,
            queue,
            buffer: [0; MTU],
        };

        let recv =
            Closure::wrap(
                Box::new(move |buffer: Uint8Array| _ = receiver.send(buffer.to_vec()))
                    as Box<dyn FnMut(Uint8Array)>,
            );
        let ready = Closure::wrap(Box::new(move || is_ready = true) as Box<dyn FnMut()>);
        let closed = Closure::wrap(
            Box::new(move |code: Option<usize>, reason: Option<String>| is_closed = true)
                as Box<dyn FnMut(Option<usize>, Option<String>)>,
        );

        let transport = WebTransportWrapper::new(server_url, hashes, &ready, &recv, &closed);

        recv.forget();
        ready.forget();
        closed.forget();

        (Box::new(transport), Box::new(queue))
    }
}
