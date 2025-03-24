use crate::server::io::{ServerIoEventReceiver, ServerNetworkEventSender};
use crate::transport::channels::Channels;
use crate::transport::dummy::DummyIo;
use crate::transport::error::Result;
use crate::transport::io::IoState;
#[cfg(all(feature = "udp", not(target_family = "wasm")))]
use crate::transport::udp::{UdpSocket, UdpSocketBuilder};
#[cfg(all(feature = "websocket", not(target_family = "wasm")))]
use crate::transport::websocket::server::{WebSocketServerSocket, WebSocketServerSocketBuilder};
#[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
use crate::transport::webtransport::server::{
    WebTransportServerSocket, WebTransportServerSocketBuilder,
};
use enum_dispatch::enum_dispatch;

#[enum_dispatch]
pub(crate) trait ServerTransportBuilder: Send + Sync {
    /// Attempt to listen for incoming connections
    fn start(
        self,
    ) -> Result<(
        ServerTransportEnum,
        IoState,
        Option<ServerIoEventReceiver>,
        Option<ServerNetworkEventSender>,
    )>;
}

#[enum_dispatch(ServerTransportBuilder)]
pub(crate) enum ServerTransportBuilderEnum {
    #[cfg(all(feature = "udp", not(target_family = "wasm")))]
    UdpSocket(UdpSocketBuilder),
    #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
    WebTransportServer(WebTransportServerSocketBuilder),
    #[cfg(all(feature = "websocket", not(target_family = "wasm")))]
    WebSocketServer(WebSocketServerSocketBuilder),
    Channels(Channels),
    Dummy(DummyIo),
}

#[allow(clippy::large_enum_variant)]
#[enum_dispatch(Transport)]
pub(crate) enum ServerTransportEnum {
    #[cfg(all(feature = "udp", not(target_family = "wasm")))]
    UdpSocket(UdpSocket),
    #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
    WebTransportServer(WebTransportServerSocket),
    #[cfg(all(feature = "websocket", not(target_family = "wasm")))]
    WebSocketServer(WebSocketServerSocket),
    Channels(Channels),
    Dummy(DummyIo),
}
