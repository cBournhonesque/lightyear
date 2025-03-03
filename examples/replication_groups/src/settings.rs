use std::{net::Ipv4Addr, string::ToString};

use lightyear::prelude::CompressionConfig;
use lightyear_examples_common::settings::{
    ClientSettings, ClientTransports, Conditioner, ServerSettings, ServerTransports, Settings,
    SharedSettings, WebTransportCertificateSettings,
};

pub(crate) fn get_settings() -> Settings {
    Settings {
        server: ServerSettings {
            headless: false,
            inspector: true,
            conditioner: Some(Conditioner {
                latency_ms: 400,
                jitter_ms: 80,
                packet_loss: 0.1,
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
                #[cfg(feature = "websocket")]
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
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0,
            ],
            compression: CompressionConfig::None,
        },
    }
}
