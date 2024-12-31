use lightyear::prelude::{CompressionConfig, Deserialize, Serialize};
use lightyear_examples_common::settings::{
    ClientSettings, ClientTransports, Conditioner, ServerSettings, ServerTransports, Settings,
    SharedSettings, WebTransportCertificateSettings,
};
use std::net::Ipv4Addr;
use std::string::ToString;

#[derive(Clone, Debug)]
pub struct MySettings {
    pub(crate) common: Settings,

    /// The server will listen on this port for incoming tcp authentication requests
    /// and respond with a [`ConnectToken`](lightyear::prelude::ConnectToken)
    pub(crate) netcode_auth_port: u16,
}

pub(crate) fn get_settings() -> MySettings {
    MySettings {
        common: Settings {
            server: ServerSettings {
                headless: false,
                inspector: true,
                conditioner: Some(Conditioner {
                    latency_ms: 200,
                    jitter_ms: 20,
                    packet_loss: 0.05,
                }),
                transport: vec![
                    ServerTransports::WebTransport {
                        local_port: 5000,
                        certificate: WebTransportCertificateSettings::FromFile {
                            cert: "../certificates/cert.pem".to_string(),
                            key: "../certificates/key.pem".to_string(),
                        },
                    },
                    ServerTransports::Udp { local_port: 5001 },
                    ServerTransports::WebSocket { local_port: 5002 },
                    #[cfg(feature = "steam")]
                    ServerTransports::Steam {
                        app_id: 480,
                        server_ip: Ipv4Addr::UNSPECIFIED,
                        game_port: 5003,
                        query_port: 27016,
                    },
                ],
            },
            client: ClientSettings {
                inspector: true,
                client_id: 0,
                client_port: 0, // 0 means that the OS will assign a random port
                server_addr: Ipv4Addr::LOCALHOST,
                server_port: 5000, // change the port depending on the transport used
                transport: ClientTransports::WebTransport {
                    #[cfg(target_family = "wasm")]
                    certificate_digest: include_str!("../../certificates/digest.txt").to_string(),
                },
                conditioner: None,
            },
            shared: SharedSettings {
                protocol_id: 0,
                private_key: [
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0,
                ],
                compression: CompressionConfig::None,
            },
        },
        netcode_auth_port: 5005,
    }
}
