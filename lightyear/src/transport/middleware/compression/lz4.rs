//! Zstd compression

use crate::connection::netcode::MAX_PKT_BUF_SIZE;
use crate::transport::error::Result;
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use std::net::SocketAddr;

pub(crate) use compression::Compressor;
pub(crate) use decompression::Decompressor;

pub(crate) mod compression {
    use super::*;
    use crate::transport::middleware::PacketSenderWrapper;
    use crate::transport::PacketSender;
    use lz4_flex::block::compress_into;

    pub(crate) struct Compressor {
        result: Vec<u8>,
    }

    impl Default for Compressor {
        fn default() -> Self {
            Compressor {
                // TODO: the max output size if input is 1200 would be 1340 bytes...
                result: vec![0; MAX_PKT_BUF_SIZE],
            }
        }
    }

    impl Compressor {
        pub fn compress(&mut self, data: &[u8]) -> Result<&[u8]> {
            // let res = compress(data);
            // error!(
            //     "input size: {:?}, compressed size: {:?}",
            //     data.len(),
            //     res.len()
            // );
            let size = compress_into(data, &mut self.result)?;
            Ok(&self.result[..size])
        }
    }

    struct Lz4PacketSender<T: PacketSender> {
        inner: T,
        compressor: Compressor,
    }

    impl<T: PacketSender> PacketSender for Lz4PacketSender<T> {
        fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
            let compressed = self.compressor.compress(payload)?;
            self.inner.send(compressed, address)
        }
    }

    impl<T: PacketSender> PacketSenderWrapper<T> for Compressor {
        fn wrap(self, sender: T) -> impl PacketSender {
            Lz4PacketSender {
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
    use lz4_flex::block::decompress_into;

    pub(crate) struct Decompressor {
        result: Vec<u8>,
    }

    impl Default for Decompressor {
        fn default() -> Self {
            Decompressor {
                // TODO: the max output size if input is 1200 would be 1340 bytes...
                result: vec![0; MAX_PKT_BUF_SIZE],
            }
        }
    }

    impl Decompressor {
        pub fn decompress(&mut self, data: &[u8]) -> Result<&mut [u8]> {
            let size = decompress_into(data, &mut self.result)?;
            Ok(&mut self.result[..size])
        }
    }

    struct Lz4PacketReceiver<T: PacketReceiver> {
        inner: T,
        decompressor: Decompressor,
    }

    impl<T: PacketReceiver> PacketReceiver for Lz4PacketReceiver<T> {
        fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
            if let Some((buf, addr)) = self.inner.recv()? {
                let decompressed = self.decompressor.decompress(buf)?;
                Ok(Some((decompressed, addr)))
            } else {
                Ok(None)
            }
        }
    }

    impl<T: PacketReceiver> PacketReceiverWrapper<T> for Decompressor {
        fn wrap(self, receiver: T) -> impl PacketReceiver {
            Lz4PacketReceiver {
                inner: receiver,
                decompressor: self,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::client::io::config::ClientTransport;
    use crate::transport::config::SharedIoConfig;
    use crate::transport::middleware::compression::CompressionConfig;
    use crate::transport::LOCAL_SOCKET;

    #[test]
    fn test_compression() {
        let (send, recv) = crossbeam_channel::unbounded();

        let config = ClientTransport::LocalChannel { send, recv };
        let io_config = SharedIoConfig::<ClientTransport> {
            transport: config,
            conditioner: None,
            compression: CompressionConfig::Lz4,
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
