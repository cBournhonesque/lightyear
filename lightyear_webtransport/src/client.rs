use crate::WebTransportError;
use aeronet_io::connection::PeerAddr;
use aeronet_webtransport::client::{ClientConfig, WebTransportClient};
use alloc::{format, string::String, vec::Vec};
use bevy_app::{App, Plugin};
use bevy_ecs::{
    error::Result,
    prelude::{Commands, Component, Entity, EntityCommand, Name, Query, Trigger, Without, World},
};
use lightyear_aeronet::{AeronetLinkOf, AeronetPlugin};
use lightyear_link::{Link, LinkStart, Linked, Linking};
#[cfg(all(not(target_family = "wasm"), feature = "dangerous-configuration"))]
use tracing::warn;

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

/// WebTransport session implementation which acts as a dedicated client,
/// connecting to a target endpoint.
///
/// The [`PeerAddr`] component will be used to find the server_addr.
///
/// Use [`WebTransportClient::connect`] to start a connection.
#[derive(Debug, Component)]
#[require(Link)]
pub struct WebTransportClientIo {
    pub certificate_digest: String,
}

impl WebTransportClientPlugin {
    fn link(
        trigger: Trigger<LinkStart>,
        query: Query<
            (Entity, &WebTransportClientIo, Option<&PeerAddr>),
            (Without<Linking>, Without<Linked>),
        >,
        mut commands: Commands,
    ) -> Result {
        if let Ok((entity, client, peer_addr)) = query.get(trigger.target()) {
            let server_addr = peer_addr.ok_or(WebTransportError::PeerAddrMissing)?.0;
            let digest = client.certificate_digest.clone();
            commands.queue(move |world: &mut World| -> Result {
                let config = Self::client_config(digest)?;
                let server_url = format!("https://{}", server_addr);
                let target = {
                    #[cfg(target_family = "wasm")]
                    {
                        server_url
                    }

                    #[cfg(not(target_family = "wasm"))]
                    {
                        use aeronet_webtransport::wtransport::endpoint::IntoConnectOptions;
                        server_url.into_options()
                    }
                };
                let entity_mut =
                    world.spawn((AeronetLinkOf(entity), Name::from("WebTransportClient")));
                WebTransportClient::connect(config, target).apply(entity_mut);
                Ok(())
            });
        }
        Ok(())
    }

    // `cert_hash` is expected to be the hexadecimal representation of the SHA256 Digest, without colons
    #[cfg(target_family = "wasm")]
    fn client_config(cert_hash: String) -> Result<ClientConfig> {
        use aeronet_webtransport::xwt_web::{CertificateHash, HashAlgorithm};

        info!("Connecting to server with certificate hash: {cert_hash}");
        let server_certificate_hashes = if cert_hash.is_empty() {
            Vec::new()
        } else {
            let hash = from_hex(&cert_hash)?;
            vec![CertificateHash {
                algorithm: HashAlgorithm::Sha256,
                value: Vec::from(hash),
            }]
        };

        Ok(ClientConfig {
            server_certificate_hashes,
            ..Default::default()
        })
    }

    // `cert_digest` is expected to be the hexadecimal representation of the SHA256 Digest, without colons
    #[cfg(not(target_family = "wasm"))]
    fn client_config(cert_digest: String) -> Result<ClientConfig> {
        use {aeronet_webtransport::wtransport::tls::Sha256Digest, core::time::Duration};

        let config = ClientConfig::builder().with_bind_default();
        let config = if cert_digest.is_empty() {
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
            let mut hash = [0u8; 32];
            hash.copy_from_slice(&from_hex(&cert_digest)?);
            let digest = Sha256Digest::new(hash);
            config.with_server_certificate_hashes([digest])
        };

        Ok(config
            .keep_alive_interval(Some(Duration::from_secs(1)))
            .max_idle_timeout(Some(Duration::from_secs(5)))
            .expect("should be a valid idle timeout")
            .build())
    }
}

// Adapted from https://github.com/briansmith/ring/blob/befdc87ac7cbca615ab5d68724f4355434d3a620/src/test.rs#L364-L393
fn from_hex(hex_str: &str) -> core::result::Result<Vec<u8>, String> {
    if hex_str.len() % 2 != 0 {
        return Err(format!(
            "Hex string does not have an even number of digits. Length: {}. String: .{}.",
            hex_str.len(),
            hex_str
        ));
    }

    let mut result = Vec::with_capacity(hex_str.len() / 2);
    for digits in hex_str.as_bytes().chunks(2) {
        let hi = from_hex_digit(digits[0])?;
        let lo = from_hex_digit(digits[1])?;
        result.push((hi * 0x10) | lo);
    }
    Ok(result)
}

fn from_hex_digit(d: u8) -> core::result::Result<u8, String> {
    use core::ops::RangeInclusive;
    const DECIMAL: (u8, RangeInclusive<u8>) = (0, b'0'..=b'9');
    const HEX_LOWER: (u8, RangeInclusive<u8>) = (10, b'a'..=b'f');
    const HEX_UPPER: (u8, RangeInclusive<u8>) = (10, b'A'..=b'F');
    for (offset, range) in &[DECIMAL, HEX_LOWER, HEX_UPPER] {
        if range.contains(&d) {
            return Ok(d - range.start() + offset);
        }
    }
    Err(format!("Invalid hex digit '{}'", d as char))
}
