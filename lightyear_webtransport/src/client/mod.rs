//! See [`WebTransportClient`].

mod backend;

use crate::cert;
use aeronet_io::connection::Disconnected;
use aeronet_webtransport::client::WebTransportClient;
use lightyear_link::{Link, LinkStart, Unlink, Unlinked};
use std::net::ToSocketAddrs;
use wtransport::endpoint::IntoConnectOptions;
use {
    crate::session::{
        self, WebTransportSessionPlugin,
    },
    bevy_app::prelude::*,
    bevy_ecs::error::Result,
    bevy_ecs::{prelude::*, system::EntityCommand},
    lightyear_link::LinkSet,
    tracing::Instrument,
};

cfg_if::cfg_if! {
    if #[cfg(target_family = "wasm")] {
        /// Configuration for the [`WebTransportClient`] on WASM platforms.
        pub type ClientConfig = xwt_web::WebTransportOptions;

        type ConnectTarget = String;

        type ConnectError = crate::JsError;
        type AwaitConnectError = crate::JsError;
    } else {
        use wtransport::endpoint::endpoint_side;
        use xwt_core::endpoint::{Connect as XwtConnect, connect::Connecting as XwtConnecting};

        /// Configuration for the [`WebTransportClient`] on non-WASM platforms.
        pub type ClientConfig = wtransport::ClientConfig;

        type ConnectTarget = wtransport::endpoint::ConnectOptions;
        type ClientEndpoint = xwt_wtransport::Endpoint<endpoint_side::Client>;

        type ConnectError = <ClientEndpoint as XwtConnect>::Error;
        type AwaitConnectError = <<ClientEndpoint as XwtConnect>::Connecting as XwtConnecting>::Error;
    }
}

/// Allows using [`WebTransportClient`].
pub struct WebTransportClientPlugin;

impl Plugin for WebTransportClientPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<WebTransportSessionPlugin>() {
            app.add_plugins(WebTransportSessionPlugin);
        }

        app.add_observer(WebTransportClient::link);
        app.add_observer(WebTransportClient::unlink);
        app.add_systems(
            PreUpdate,
            (poll_connecting, poll_connected)
                .in_set(LinkSet::Receive)
                .before(session::receive),
        );
    }
}

#[derive(thiserror::Error, Debug)]
pub enum WebTransportError {
    #[error("the certificate hash `{0}` is invalid")]
    Certificate(String),
}

/// WebTransport session implementation which acts as a dedicated client,
/// connecting to a target endpoint.
///
/// Use [`WebTransportClient::connect`] to start a connection.
#[derive(Debug, Component)]
#[require(Link)]
pub struct WebTransportClientIo {
    pub server_addr: core::net::SocketAddr,
    pub certificate_digest: String,
}

impl WebTransportClientIo {
    #[must_use]
    fn link(
        trigger: Trigger<LinkStart>,
        query: Query<(Entity, &WebTransportClientIo), With<Unlinked>>,
        mut commands: Commands,
    ) {
        if let Ok((entity, client)) = query.get(trigger.target()) {
            let digest = client.certificate_digest.clone();
            let server_addr = client.server_addr;
            commands.queue(move |world: &mut World| -> Result {
                let config = Self::client_config(digest)?;
                let server_url = format!("https://{}", server_addr);
                let target = {
                    #[cfg(target_family = "wasm")]
                    {
                        server_url.into()
                    }

                    #[cfg(not(target_family = "wasm"))]
                    {
                        server_url.into_options()
                    }
                };
                let entity_mut = world.spawn(
                    ChildOf(entity));
                WebTransportClient::connect(config, target).apply(entity_mut);
                Ok(())
            });
        }
    }

    #[must_use]
    fn unlink(
        mut trigger: Trigger<Unlink>,
        mut query: Query<&Children, (Without<Unlinked>, With<WebTransportClient>)>,
        child_query: Query<(Entity, &WebTransportClient)>,
        mut commands: Commands,
    ) {
        if let Ok(children) = query.get(trigger.target()) {
            for child in children.iter() {
                if let Ok((child_entity, _)) = child_query.get(*child) {
                    match trigger.event_mut() {
                        Unlink::ByLocal(reason) => {
                            commands.entity(child_entity).trigger(Disconnected::ByUser(core::mem::take(reason)));
                        }
                        Unlink::ByRemote(reason) => {
                            commands.entity(child_entity).trigger(Disconnected::ByPeer(core::mem::take(reason)));
                        }
                    }
                }
            }
        }
    }

    #[cfg(target_family = "wasm")]
    fn client_config(cert_hash: String) -> Result<ClientConfig> {
        use xwt_web::{CertificateHash, HashAlgorithm};

        let server_certificate_hashes = if cert_hash.is_empty() {
            Vec::new()
        } else {
            match cert::hash_from_b64(&cert_hash) {
                Ok(hash) => vec![CertificateHash {
                    algorithm: HashAlgorithm::Sha256,
                    value: Vec::from(hash),
                }],
                Err(err) => {
                    WebTransportError::Certificate("Failed to read certificate hash from string: {err:?}".to_string())?
                }
            }
        };

        Ok(ClientConfig {
            server_certificate_hashes,
            ..Default::default()
        })
    }

    #[cfg(not(target_family = "wasm"))]
    fn client_config(cert_hash: String) -> Result<ClientConfig> {
        use {core::time::Duration, wtransport::tls::Sha256Digest};

        let config = ClientConfig::builder().with_bind_default();

        let config = if cert_hash.is_empty() {
            #[cfg(feature = "dangerous-configuration")]
            {
                warn!("Connecting with no certificate validation");
                config.with_no_cert_validation()
            }
            #[cfg(not(feature = "dangerous-configuration"))]
            {
                config.with_server_certificate_hashes([])
            }
        } else {
            let hash = cert::hash_from_b64(&cert_hash)?;
            config.with_server_certificate_hashes([Sha256Digest::new(hash)])
        };

        Ok(config
            .keep_alive_interval(Some(Duration::from_secs(1)))
            .max_idle_timeout(Some(Duration::from_secs(5)))
            .expect("should be a valid idle timeout")
            .build())
    }
}