//! Client-side WebTransport transport integration.
//!
//! [`WebTransportClientIo`](crate::client::WebTransportClientIo) is a Lightyear link component that
//! spawns an Aeronet WebTransport client entity when [`LinkStart`](lightyear_link::LinkStart) is
//! triggered. `lightyear_aeronet` then mirrors Aeronet session state and moves payloads between the
//! Aeronet session and the Lightyear [`Link`](lightyear_link::Link).

use crate::{WebTransportLocalAddr, WebTransportPeerAddr};
use aeronet_io::connection::{LocalAddr, PeerAddr};
use aeronet_webtransport::client::{ClientConfig, WebTransportClient};
use alloc::{format, string::String, vec::Vec};
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use core::net::SocketAddr;
use lightyear_aeronet::{AeronetLinkOf, AeronetPlugin};
use lightyear_connection::admission::{DefaultConnectionPlugin, apply_default_ids};
use lightyear_connection::p2p::P2P;
use lightyear_core::id::{LocalId, PeerId, RemoteId};
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
        if !app.is_plugin_added::<DefaultConnectionPlugin>() {
            app.add_plugins(DefaultConnectionPlugin);
        }
        app.add_plugins(aeronet_webtransport::client::WebTransportClientPlugin);
        app.add_observer(Self::link);
        app.add_observer(Self::on_linked);
        app.add_observer(Self::on_addresses);
    }
}

/// Lightyear component for a WebTransport client transport.
///
/// Insert this on the entity that owns the client-side [`Link`]. When [`LinkStart`] is triggered,
/// the plugin connects to [`target`](Self::target) unchanged, preserving its hostname and path,
/// and spawns an Aeronet [`WebTransportClient`] related back to this link.
/// A [`Client`](lightyear_connection::client::Client) role is added automatically. On browsers,
/// which hide socket addresses, default admission uses the entity as a local-only debug identity.
#[derive(Debug, Component)]
#[require(Link, lightyear_connection::client::Client)]
pub struct WebTransportClientIo {
    /// Hex-encoded SHA-256 certificate digest/hash expected from the server.
    ///
    /// On native targets this is converted to `wtransport::tls::Sha256Digest`. On WASM targets it
    /// is converted to a browser WebTransport certificate hash. An empty string disables explicit
    /// hashes; on native targets that only disables validation when the `dangerous-configuration`
    /// feature is enabled.
    pub certificate_digest: String,
    /// Full WebTransport URL to connect to, for example `https://example.com:4433`.
    pub target: String,
}

impl WebTransportClientIo {
    /// Connects to a full URL, preserving its hostname, path and query.
    pub fn from_url(url: impl Into<String>, certificate_digest: impl Into<String>) -> Self {
        Self {
            target: url.into(),
            certificate_digest: certificate_digest.into(),
        }
    }

    /// Connects to a socket address using HTTPS.
    pub fn from_addr(address: SocketAddr, certificate_digest: impl Into<String>) -> Self {
        Self::from_url(format!("https://{address}"), certificate_digest)
    }

    /// Connects to a hostname without resolving DNS in advance.
    pub fn from_host(host: &str, port: u16, certificate_digest: impl Into<String>) -> Self {
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let target = if host.contains(':') {
            format!("https://[{host}]:{port}")
        } else {
            format!("https://{host}:{port}")
        };
        Self::from_url(target, certificate_digest)
    }
}

impl WebTransportClientPlugin {
    fn on_addresses(
        trigger: On<Insert, (LocalAddr, PeerAddr, AeronetLinkOf, WebTransportClient)>,
        query: Query<
            (&AeronetLinkOf, Option<&LocalAddr>, Option<&PeerAddr>),
            With<WebTransportClient>,
        >,
        mut commands: Commands,
    ) {
        if let Ok((owner, local, peer)) = query.get(trigger.entity) {
            let mut link = commands.entity(owner.0);
            if let Some(local) = local {
                link.insert(WebTransportLocalAddr(local.0));
            }
            if let Some(peer) = peer {
                link.insert(WebTransportPeerAddr(peer.0));
            }
        }
    }

    fn on_linked(
        trigger: On<Add, (Linked, WebTransportLocalAddr, WebTransportPeerAddr)>,
        query: Query<
            (
                Option<&WebTransportLocalAddr>,
                Option<&WebTransportPeerAddr>,
            ),
            (With<WebTransportClientIo>, With<Linked>, Without<P2P>),
        >,
        mut commands: Commands,
    ) {
        let Ok((local_addr, peer_addr)) = query.get(trigger.entity) else {
            return;
        };
        let entity = trigger.entity;
        #[cfg(not(target_family = "wasm"))]
        let (local, remote) = {
            let (Some(local_addr), Some(peer_addr)) = (local_addr, peer_addr) else {
                return;
            };
            (PeerId::Raw(local_addr.0), PeerId::Raw(peer_addr.0))
        };
        // Browsers do not expose socket addresses. These are local debug routing labels only.
        #[cfg(target_family = "wasm")]
        let (local, remote) = {
            let _ = (local_addr, peer_addr);
            (PeerId::Entity(entity.to_bits()), PeerId::Server)
        };
        commands.queue(move |world: &mut World| {
            apply_default_ids(world, entity, LocalId(local), RemoteId(remote));
        });
    }

    fn link(
        trigger: On<LinkStart>,
        query: Query<(Entity, &WebTransportClientIo), (Without<Linking>, Without<Linked>)>,
        mut commands: Commands,
    ) -> Result {
        if let Ok((entity, client)) = query.get(trigger.entity) {
            let target = client.target.clone();
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
        use aeronet_webtransport::xwt_web::{CertificateHash, CongestionControl, HashAlgorithm};
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
            congestion_control: CongestionControl::LowLatency,
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
            let bytes = from_hex(&cert_digest)?;
            let hash: [u8; 32] = bytes.try_into().map_err(|bytes: Vec<u8>| {
                format!(
                    "Expected a 32-byte SHA256 digest, got {} bytes.",
                    bytes.len()
                )
            })?;
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

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::{WebTransportClientIo, WebTransportClientPlugin};

    #[test]
    fn host_constructor_brackets_ipv6_once() {
        for host in ["::1", "[::1]"] {
            let client = WebTransportClientIo::from_host(host, 4100, "");
            assert_eq!(client.target, "https://[::1]:4100");
        }
    }

    #[test]
    fn short_hex_digest_is_an_error_not_a_panic() {
        assert!(WebTransportClientPlugin::client_config("abcd".to_string()).is_err());
    }

    #[test]
    fn malformed_hex_digest_is_an_error() {
        assert!(WebTransportClientPlugin::client_config("xyz".to_string()).is_err());
        assert!(
            WebTransportClientPlugin::client_config("zz".repeat(32).as_str().to_string()).is_err()
        );
    }

    #[test]
    fn full_length_hex_digest_builds_config() {
        assert!(
            WebTransportClientPlugin::client_config("ab".repeat(32).as_str().to_string()).is_ok()
        );
    }
}
