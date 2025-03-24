//! Zstd compression

use crate::connection::netcode::MAX_PKT_BUF_SIZE;
use crate::transport::error::{Error, Result};
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use std::net::SocketAddr;

pub(crate) mod compression {
    use super::*;
    use crate::transport::middleware::PacketSenderWrapper;
    use crate::transport::PacketSender;
    use zstd::bulk::Compressor;

    pub(crate) struct ZstdCompressor {
        result: Vec<u8>,
        compressor: Compressor<'static>,
    }

    impl ZstdCompressor {
        pub fn new(level: i32) -> Self {
            ZstdCompressor {
                result: Vec::with_capacity(MAX_PKT_BUF_SIZE),
                compressor: Compressor::new(level).unwrap(),
            }
        }

        pub fn compress(&mut self, data: &[u8]) -> Result<&[u8]> {
            self.compressor
                .compress_to_buffer(data, &mut self.result)
                .map_err(|e| Error::Io(e))?;
            Ok(&self.result)
        }
    }

    struct ZstdPacketSender<T: PacketSender> {
        inner: T,
        compressor: ZstdCompressor,
    }

    impl<T: PacketSender> PacketSender for ZstdPacketSender<T> {
        fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
            let compressed = self.compressor.compress(payload)?;
            self.inner.send(compressed, address)
        }
    }

    impl<T: PacketSender> PacketSenderWrapper<T> for ZstdCompressor {
        fn wrap(self, sender: T) -> impl PacketSender {
            ZstdPacketSender {
                inner: sender,
                compressor: self,
            }
        }
    }
}

pub(crate) mod decompression {
    use super::*;
    use crate::transport::middleware::PacketReceiverWrapper;
    use crate::transport::PacketReceiver;
    use zstd::bulk::Decompressor;

    pub(crate) struct ZstdDecompressor {
        result: Vec<u8>,
        decompressor: Decompressor<'static>,
    }

    impl ZstdDecompressor {
        pub fn new() -> Self {
            ZstdDecompressor {
                result: Vec::with_capacity(MAX_PKT_BUF_SIZE),
                decompressor: Decompressor::new().unwrap(),
            }
        }

        pub fn decompress(&mut self, data: &[u8]) -> Result<&mut [u8]> {
            self.decompressor
                .decompress_to_buffer(data, &mut self.result)
                .map_err(|e| Error::Io(e))?;
            Ok(&mut self.result)
        }
    }

    struct ZstdPacketReceiver<T: PacketReceiver> {
        inner: T,
        decompressor: ZstdDecompressor,
    }

    impl<T: PacketReceiver> PacketReceiver for ZstdPacketReceiver<T> {
        fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
            if let Some((buf, addr)) = self.inner.recv()? {
                let decompressed = self.decompressor.decompress(buf)?;
                Ok(Some((decompressed, addr)))
            } else {
                Ok(None)
            }
        }
    }

    impl<T: PacketReceiver> PacketReceiverWrapper<T> for ZstdDecompressor {
        fn wrap(self, receiver: T) -> impl PacketReceiver {
            ZstdPacketReceiver {
                inner: receiver,
                decompressor: self,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::prelude::{SharedIoConfig, TransportConfig};
    use crate::transport::middleware::compression::CompressionConfig;
    use crate::transport::LOCAL_SOCKET;

    #[test]
    fn test_compression() {
        let (send, recv) = crossbeam_channel::unbounded();

        let config = TransportConfig::LocalChannel { send, recv };
        let io_config = SharedIoConfig {
            transport: config,
            conditioner: None,
            compression: CompressionConfig::Zstd { level: 0 },
        };
        let mut io = io_config.connect().unwrap();
        let msg = b"hello world".as_slice();
        // send data
        io.sender.send(&msg, &LOCAL_SOCKET).unwrap();

        // receive data
        let (data, addr) = io.receiver.recv().unwrap().unwrap();
        assert_eq!(data.as_ref(), msg);
    }
}
