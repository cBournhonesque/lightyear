use std::{net::Ipv4Addr, string::ToString};

use lightyear::prelude::CompressionConfig;
use lightyear_examples_common::settings::{
    ClientSettings, ClientTransports, Conditioner, ServerSettings, ServerTransports, Settings,
    SharedSettings, WebTransportCertificateSettings,
};

#[derive(Clone, Debug)]
pub struct MySettings {
    pub common: Settings,

    /// If true, we will predict the client's entities, but also the ball and other clients' entities!
    /// This is what is done by RocketLeague (see [video](https://www.youtube.com/watch?v=ueEmiDM94IE))
    ///
    /// If false, we will predict the client's entities but simple interpolate everything else.
    pub(crate) predict_all: bool,

    /// By how many ticks an input press will be delayed before we apply client-prediction?
    ///
    /// This can be useful as a tradeoff between input delay and prediction accuracy.
    /// If the input delay is greater than the RTT, then there won't ever be any mispredictions/rollbacks.
    /// See [this article](https://www.snapnet.dev/docs/core-concepts/input-delay-vs-rollback/) for more information.
    pub(crate) input_delay_ticks: u16,

    /// What is the maximum number of ticks that we will rollback for?
    /// After applying input delay, we will try cover `max_prediction_ticks` ticks of latency using client-side prediction
    /// Any more latency beyond that will use more input delay.
    pub(crate) max_prediction_ticks: u16,

    /// If visual correction is enabled, we don't instantly snapback to the corrected position
    /// when we need to rollback. Instead we interpolated between the current position and the
    /// corrected position.
    /// This controls the duration of the interpolation; the higher it is, the longer the interpolation
    /// will take
    pub(crate) correction_ticks_factor: f32,

    /// If true, we will also show the Confirmed entities (on top of the Predicted entities)
    pub(crate) show_confirmed: bool,

    /// Sets server replication send interval in both client and server configs
    pub(crate) server_replication_send_interval: u64,
}

pub(crate) fn get_settings() -> MySettings {
    MySettings {
        common: Settings {
            server: ServerSettings {
                headless: false,
                inspector: true,
                conditioner: Some(Conditioner {
                    latency_ms: 25,
                    jitter_ms: 2,
                    packet_loss: 0.005,
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
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0,
                ],
                compression: CompressionConfig::None,
            },
        },
        predict_all: true,
        input_delay_ticks: 6,
        max_prediction_ticks: 100,
        correction_ticks_factor: 1.5,
        show_confirmed: true,
        server_replication_send_interval: 20,
    }
}
