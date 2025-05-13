use aeronet_webtransport::cert;
use aeronet_webtransport::client::{ClientConfig, WebTransportClient};
use bevy::prelude::*;
use lightyear_aeronet::{AeronetLinkOf, AeronetPlugin};
use lightyear_link::{Link, LinkStart, Linked, Linking};


pub struct WebTransportClientPlugin;

impl Plugin for WebTransportClientPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<AeronetPlugin>() {
            app.add_plugins(AeronetPlugin);
        }
        app.add_plugins(aeronet_webtransport::client::WebTransportClientPlugin);
        app.add_observer(Self::link);
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

impl WebTransportClientPlugin {
    #[must_use]
    fn link(
        trigger: Trigger<LinkStart>,
        query: Query<(Entity, &WebTransportClientIo), (Without<Linking>, Without<Linked>)>,
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
                        use aeronet_webtransport::wtransport::endpoint::IntoConnectOptions;
                        server_url.into_options()
                    }
                };
                let entity_mut = world.spawn((
                    AeronetLinkOf(entity),
                    Name::from("WebTransportClient"),
                ));
                WebTransportClient::connect(config, target).apply(entity_mut);
                Ok(())
            });
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
        use {aeronet_webtransport::wtransport::tls::Sha256Digest, core::time::Duration};

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