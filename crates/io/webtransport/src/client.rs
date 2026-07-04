//! Client-side WebTransport transport integration.
//!
//! [`WebTransportClientIo`](crate::client::WebTransportClientIo) is a Lightyear link component that
//! spawns an Aeronet WebTransport client entity when [`LinkStart`](lightyear_link::LinkStart) is
//! triggered. `lightyear_aeronet` then mirrors Aeronet session state and moves payloads between the
//! Aeronet session and the Lightyear [`Link`](lightyear_link::Link).

use crate::WebTransportError;
use aeronet_io::connection::PeerAddr;
use aeronet_webtransport::client::{ClientConfig, WebTransportClient};
use alloc::{format, string::String, vec::Vec};
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use lightyear_aeronet::{AeronetLinkOf, AeronetPlugin};
use lightyear_link::{Link, LinkStart, Linked, Linking};
#[cfg(all(not(target_family = "wasm"), feature = "dangerous-configuration"))]
use tracing::warn;

/// Plugin that starts WebTransport client sessions for [`WebTransportClientIo`] link entities.
///
/// The plugin ensures [`AeronetPlugin`] and Aeronet's WebTransport client plugin are installed,
/// then observes [`LinkStart`] to spawn and connect the underlying Aeronet client entity.
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

/// Lightyear component for a WebTransport client transport.
///
/// Insert this on the entity that owns the client-side [`Link`]. If [`target`](Self::target) is
/// `Some`, it is used as the WebTransport URL. Otherwise a [`PeerAddr`] must be present when
/// [`LinkStart`] is triggered; the plugin builds an `https://<peer_addr>` target and spawns an
/// Aeronet [`WebTransportClient`] related back to this link.
#[derive(Debug, Component)]
#[require(Link)]
pub struct WebTransportClientIo {
    /// Hex-encoded SHA-256 certificate digest/hash expected from the server.
    ///
    /// On native targets this is converted to `wtransport::tls::Sha256Digest`. On WASM targets it
    /// is converted to a browser WebTransport certificate hash. An empty string disables explicit
    /// hashes; on native targets that only disables validation when the `dangerous-configuration`
    /// feature is enabled.
    pub certificate_digest: String,
    /// Full WebTransport URL to connect to, for example `https://example.com:4433`.
    ///
    /// When set, this takes priority over the entity's [`PeerAddr`]. When unset, the target is
    /// derived from [`PeerAddr`] as `https://<peer_addr>`.
    pub target: Option<String>,
}

impl WebTransportClientPlugin {
    fn link(
        trigger: On<LinkStart>,
        query: Query<
            (Entity, &WebTransportClientIo, Option<&PeerAddr>),
            (Without<Linking>, Without<Linked>),
        >,
        mut commands: Commands,
    ) -> Result {
        if let Ok((entity, client, peer_addr)) = query.get(trigger.entity) {
            let target = client
                .target
                .clone()
                .or_else(|| peer_addr.map(|addr| format!("https://{}", addr.0)))
                .ok_or(WebTransportError::PeerAddrMissing)?;
            let digest = client.certificate_digest.clone();
            commands.queue(move |world: &mut World| -> Result {
                let config = Self::client_config(digest)?;
                let target = {
                    #[cfg(target_family = "wasm")]
                    {
                        target
                    }

                    #[cfg(not(target_family = "wasm"))]
                    {
                        use aeronet_webtransport::wtransport::endpoint::IntoConnectOptions;
                        target.into_options()
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
        use tracing::info;

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
        use {
            aeronet_webtransport::wtransport::{config::IpBindConfig, tls::Sha256Digest},
            core::time::Duration,
        };

        // TODO: for some reason on linux the default can bind to ipv6 which is not supported.
        //  Let the user specify the config
        let config = ClientConfig::builder().with_bind_config(IpBindConfig::InAddrAnyV4);
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
    if !hex_str.len().is_multiple_of(2) {
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
