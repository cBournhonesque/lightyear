//!
//! This module defines configurations for the WebTransport server and client.
//!
//! It provides builders for creating server and client configurations with various options.
//!
//! The module includes:
//! - [`ServerConfig`]: Configuration for the WebTransport server.
//! - [`ClientConfig`]: Configuration for the WebTransport client.
//!
//! Example for creating a server configuration:
//!
//! ```no_run
//! # async fn run() {
//! use wtransport::Certificate;
//! use wtransport::ServerConfig;
//!
//! let server_config = ServerConfig::builder()
//!     .with_bind_default(443)
//!     .with_certificate(Certificate::load("cert.pem", "key.pem").await.unwrap())
//!     .build();
//! # }
//! ```
//!
//! Example for creating a client configuration:
//!
//! ```no_run
//! use wtransport::ClientConfig;
//!
//! let client_config = ClientConfig::builder()
//!     .with_bind_default()
//!     .with_native_certs()
//!     .build();
//! ```

use crate::Certificate;
use quinn::ClientConfig as QuicClientConfig;
use quinn::ServerConfig as QuicServerConfig;
use quinn::TransportConfig;
use rustls::ClientConfig as TlsClientConfig;
use rustls::RootCertStore;
use rustls::ServerConfig as TlsServerConfig;
use std::future::Future;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;
use std::net::SocketAddr;
use std::net::SocketAddrV6;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use wtransport_proto::WEBTRANSPORT_ALPN;

/// Configuration for IP address socket bind.
#[derive(Debug, Copy, Clone)]
pub enum IpBindConfig {
    /// Bind to LOCALHOST IPv4 address (no IPv6).
    LocalV4,

    /// Bind to LOCALHOST IPv6 address (no IPv4).
    LocalV6,

    /// Bind to LOCALHOST both IPv4 and IPv6 address (dual stack, if supported).
    LocalDual,

    /// Bind to INADDR_ANY IPv4 address (no IPv6).
    InAddrAnyV4,

    /// Bind to INADDR_ANY IPv6 address (no IPv4).
    InAddrAnyV6,

    /// Bind to INADDR_ANY both IPv4 and IPv6 address (dual stack, if supported).
    InAddrAnyDual,
}

impl IpBindConfig {
    fn into_ip(self) -> IpAddr {
        match self {
            IpBindConfig::LocalV4 => Ipv4Addr::LOCALHOST.into(),
            IpBindConfig::LocalV6 => Ipv6Addr::LOCALHOST.into(),
            IpBindConfig::LocalDual => Ipv6Addr::LOCALHOST.into(),
            IpBindConfig::InAddrAnyV4 => Ipv4Addr::UNSPECIFIED.into(),
            IpBindConfig::InAddrAnyV6 => Ipv6Addr::UNSPECIFIED.into(),
            IpBindConfig::InAddrAnyDual => Ipv6Addr::UNSPECIFIED.into(),
        }
    }

    fn into_dual_stack_config(self) -> Ipv6DualStackConfig {
        match self {
            IpBindConfig::LocalV4 | IpBindConfig::InAddrAnyV4 => Ipv6DualStackConfig::OsDefault,
            IpBindConfig::LocalV6 | IpBindConfig::InAddrAnyV6 => Ipv6DualStackConfig::Deny,
            IpBindConfig::LocalDual | IpBindConfig::InAddrAnyDual => Ipv6DualStackConfig::Allow,
        }
    }
}

/// Configuration for IPv6 dual stack.
#[derive(Debug, Copy, Clone)]
pub enum Ipv6DualStackConfig {
    /// Do not configure dual stack. Use OS's default.
    OsDefault,

    /// Deny dual stack. This is equivalent to `IPV6_V6ONLY`.
    ///
    /// Socket will only bind for IPv6 (IPv4 port will still be available).
    Deny,

    /// Allow dual stack.
    ///
    /// Please note that not all configurations/platforms support dual stack.
    Allow,
}

/// Invalid idle timeout.
#[derive(Debug)]
pub struct InvalidIdleTimeout;

/// Server configuration.
///
/// You can create an instance of `ServerConfig` using its builder pattern by calling
/// the [`builder()`](Self::builder) method.
/// Once you have an instance, you can further customize it by chaining method calls
/// to set various configuration options.
///
/// ## Configuration Builder States
///
/// The configuration process follows a *state-based builder pattern*, where the server
/// configuration progresses through *3* states.
///
/// ### 1. `WantsBindAddress`
///
/// The caller must supply a binding address for the server. This is where to specify
/// the listening port of the server.
/// The following options are mutually exclusive:
///
///   - [`with_bind_default`](ServerConfigBuilder::with_bind_default): the simplest
///     configuration where only the port will be specified.
///   - [`with_bind_config`](ServerConfigBuilder::with_bind_config): configures
///     to bind an address determined by a configuration preset.
///   - [`with_bind_address`](ServerConfigBuilder::with_bind_address): configures
///     to bind a custom specified socket address.
///
/// Only one of these options can be selected during the client configuration process.
///
/// #### Examples:
///
/// ```
/// use wtransport::ServerConfig;
///
/// // Configuration for accepting incoming connection on port 443
/// ServerConfig::builder().with_bind_default(443);
/// ```
///
/// ### 2. `WantsCertificate`
///
/// The caller must supply a TLS certificate for the server.
///
/// - [`with_certificate`](ServerConfigBuilder::with_certificate): configures
///   a TLS [`Certificate`] for the server.
/// - [`with_custom_tls`](ServerConfigBuilder::with_custom_tls): sets the TLS
///   server configuration manually.
///
/// #### Examples:
/// ```
/// # use anyhow::Result;
/// use wtransport::Certificate;
/// use wtransport::ServerConfig;
///
/// # async fn run() -> Result<()> {
/// ServerConfig::builder()
///     .with_bind_default(443)
///     .with_certificate(Certificate::load("cert.pem", "key.pem").await?);
/// # Ok(())
/// # }
/// ```
///
/// ### 3. `WantsTransportConfigServer`
///
/// The caller can supply *additional* transport configurations.
/// Multiple options can be given at this stage. Once the configuration is completed, it is possible
/// to finalize with the method [`build()`](ServerConfigBuilder::build).
///
/// All these options can be omitted in the configuration; default values will be used.
///
/// - [`max_idle_timeout`](ServerConfigBuilder::max_idle_timeout)
/// - [`keep_alive_interval`](ServerConfigBuilder::keep_alive_interval)
/// - [`allow_migration`](ServerConfigBuilder::allow_migration)
///
/// #### Examples:
/// ```
/// # use anyhow::Result;
/// use wtransport::ServerConfig;
/// use wtransport::Certificate;
/// use std::time::Duration;
///
/// # async fn run() -> Result<()> {
/// let server_config = ServerConfig::builder()
///     .with_bind_default(443)
///     .with_certificate(Certificate::load("cert.pem", "key.pem").await?)
///     .keep_alive_interval(Some(Duration::from_secs(3)))
///     .build();
/// # Ok(())
/// # }
pub struct ServerConfig {
    pub(crate) bind_address: SocketAddr,
    pub(crate) dual_stack_config: Ipv6DualStackConfig,
    pub(crate) quic_config: QuicServerConfig,
}

impl ServerConfig {
    /// Creates a builder to build up the server configuration.
    ///
    /// For more information, see the [`ServerConfigBuilder`] documentation.
    pub fn builder() -> ServerConfigBuilder<WantsBindAddress> {
        ServerConfigBuilder::default()
    }

    /// Returns a reference to the inner QUIC configuration.
    #[cfg(feature = "quinn")]
    #[cfg_attr(docsrs, doc(cfg(feature = "quinn")))]
    pub fn quic_config(&self) -> &quinn::ServerConfig {
        &self.quic_config
    }

    /// Returns a mutable reference to the inner QUIC configuration.
    #[cfg(feature = "quinn")]
    #[cfg_attr(docsrs, doc(cfg(feature = "quinn")))]
    pub fn quic_config_mut(&mut self) -> &mut quinn::ServerConfig {
        &mut self.quic_config
    }
}

/// Server builder configuration.
///
/// The builder might have different state at compile time.
///
/// # Examples:
/// ```no_run
/// # async fn run() {
/// # use std::net::Ipv4Addr;
/// # use std::net::SocketAddr;
/// # use wtransport::Certificate;
/// # use wtransport::ServerConfig;
/// let config = ServerConfig::builder()
///     .with_bind_default(4433)
///     .with_certificate(Certificate::load("cert.pem", "key.pem").await.unwrap());
/// # }
/// ```
#[must_use]
pub struct ServerConfigBuilder<State>(State);

impl ServerConfigBuilder<WantsBindAddress> {
    /// Configures for accepting incoming connections binding ANY IP (allowing IP dual-stack).
    ///
    /// `listening_port` is the port where the server will accept incoming connections.
    ///
    /// This is equivalent to: [`Self::with_bind_config`] with [`IpBindConfig::InAddrAnyDual`].
    pub fn with_bind_default(self, listening_port: u16) -> ServerConfigBuilder<WantsCertificate> {
        self.with_bind_config(IpBindConfig::InAddrAnyDual, listening_port)
    }

    /// Sets the binding (local) socket address with a specific [`IpBindConfig`].
    ///
    /// `listening_port` is the port where the server will accept incoming connections.
    pub fn with_bind_config(
        self,
        ip_bind_config: IpBindConfig,
        listening_port: u16,
    ) -> ServerConfigBuilder<WantsCertificate> {
        let ip_address: IpAddr = ip_bind_config.into_ip();

        match ip_address {
            IpAddr::V4(ip) => self.with_bind_address(SocketAddr::new(ip.into(), listening_port)),
            IpAddr::V6(ip) => self.with_bind_address_v6(
                SocketAddrV6::new(ip, listening_port, 0, 0),
                ip_bind_config.into_dual_stack_config(),
            ),
        }
    }

    /// Sets the binding (local) socket address for the endpoint.
    pub fn with_bind_address(self, address: SocketAddr) -> ServerConfigBuilder<WantsCertificate> {
        ServerConfigBuilder(WantsCertificate {
            bind_address: address,
            dual_stack_config: Ipv6DualStackConfig::OsDefault,
        })
    }

    /// Sets the binding (local) socket address for the endpoint with Ipv6 address.
    ///
    /// `dual_stack_config` allows/denies dual stack port binding.
    pub fn with_bind_address_v6(
        self,
        address: SocketAddrV6,
        dual_stack_config: Ipv6DualStackConfig,
    ) -> ServerConfigBuilder<WantsCertificate> {
        ServerConfigBuilder(WantsCertificate {
            bind_address: address.into(),
            dual_stack_config,
        })
    }
}

impl ServerConfigBuilder<WantsCertificate> {
    /// Configures TLS with safe defaults and a [`Certificate`].
    ///
    /// # Example
    /// ```no_run
    /// use wtransport::Certificate;
    /// use wtransport::ServerConfig;
    /// # use anyhow::Result;
    ///
    /// # async fn run() -> Result<()> {
    /// let certificate = Certificate::load("cert.pem", "key.pem").await?;
    ///
    /// let server_config = ServerConfig::builder()
    ///     .with_bind_default(4433)
    ///     .with_certificate(certificate)
    ///     .build();
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_certificate(
        self,
        certificate: Certificate,
    ) -> ServerConfigBuilder<WantsTransportConfigServer> {
        self.with_custom_tls(Self::build_tls_config(certificate))
    }

    /// Allows for manual configuration of a custom TLS setup using a provided
    /// [`rustls::ServerConfig`].
    ///
    /// This method is provided for advanced users who need fine-grained control over the
    /// TLS configuration. It allows you to pass a preconfigured [`rustls::ServerConfig`]
    /// instance to customize the TLS settings according to your specific requirements.
    ///
    /// Generally, it is recommended to use the [`with_certificate`](Self::with_certificate) method
    /// to configure TLS with safe defaults and a [`Certificate`].
    ///
    /// # Example
    ///
    /// ```no_run
    /// use wtransport::tls::rustls;
    /// use wtransport::ServerConfig;
    ///
    /// // Create a custom rustls::ServerConfig with specific TLS settings
    /// let custom_tls_config = rustls::ServerConfig::builder();
    /// // Customize TLS settings here...
    /// # let custom_tls_config = custom_tls_config
    /// #          .with_safe_defaults()
    /// #          .with_no_client_auth()
    /// #          .with_single_cert(todo!(), todo!()).unwrap();
    ///
    /// // Create a ServerConfigBuilder with the custom TLS configuration
    /// let server_config = ServerConfig::builder()
    ///     .with_bind_default(4433)
    ///     .with_custom_tls(custom_tls_config)
    ///     .build();
    /// ```
    pub fn with_custom_tls(
        self,
        tls_config: rustls::ServerConfig,
    ) -> ServerConfigBuilder<WantsTransportConfigServer> {
        let transport_config = TransportConfig::default();

        ServerConfigBuilder(WantsTransportConfigServer {
            bind_address: self.0.bind_address,
            dual_stack_config: self.0.dual_stack_config,
            tls_config,
            transport_config,
            migration: true,
        })
    }

    fn build_tls_config(certificate: Certificate) -> TlsServerConfig {
        let mut tls_config = TlsServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(certificate.certificates, certificate.key)
            .expect("Certificate and private key should be already validated");

        tls_config.alpn_protocols = [WEBTRANSPORT_ALPN.to_vec()].to_vec();

        tls_config
    }
}

impl ServerConfigBuilder<WantsTransportConfigServer> {
    /// Completes configuration process.
    #[must_use]
    pub fn build(self) -> ServerConfig {
        let mut quic_config = QuicServerConfig::with_crypto(Arc::new(self.0.tls_config));
        quic_config.transport_config(Arc::new(self.0.transport_config));
        quic_config.migration(self.0.migration);

        ServerConfig {
            bind_address: self.0.bind_address,
            dual_stack_config: self.0.dual_stack_config,
            quic_config,
        }
    }

    /// Maximum duration of inactivity to accept before timing out the connection.
    ///
    /// The true idle timeout is the minimum of this and the peer's own max idle timeout. `None`
    /// represents an infinite timeout.
    ///
    /// **WARNING**: If a peer or its network path malfunctions or acts maliciously, an infinite
    /// idle timeout can result in permanently hung futures!
    pub fn max_idle_timeout(
        mut self,
        idle_timeout: Option<Duration>,
    ) -> Result<Self, InvalidIdleTimeout> {
        let idle_timeout = idle_timeout
            .map(quinn::IdleTimeout::try_from)
            .transpose()
            .map_err(|_| InvalidIdleTimeout)?;

        self.0.transport_config.max_idle_timeout(idle_timeout);

        Ok(self)
    }

    /// Period of inactivity before sending a keep-alive packet
    ///
    /// Keep-alive packets prevent an inactive but otherwise healthy connection from timing out.
    ///
    /// `None` to disable, which is the default. Only one side of any given connection needs keep-alive
    /// enabled for the connection to be preserved. Must be set lower than the
    /// [`max_idle_timeout`](Self::max_idle_timeout) of both peers to be effective.
    pub fn keep_alive_interval(mut self, interval: Option<Duration>) -> Self {
        self.0.transport_config.keep_alive_interval(interval);
        self
    }

    /// Whether to allow clients to migrate to new addresses.
    ///
    /// Improves behavior for clients that move between different internet connections or suffer NAT
    /// rebinding. Enabled by default.
    pub fn allow_migration(mut self, value: bool) -> Self {
        self.0.migration = value;
        self
    }
}

/// Client configuration.
///
///
/// You can create an instance of `ClientConfig` using its builder pattern by calling
/// the [`builder()`](Self::builder) method.
/// Once you have an instance, you can further customize it by chaining method calls
/// to set various configuration options.
///
/// ## Configuration Builder States
///
/// The configuration process follows a *state-based builder pattern*, where the client
/// configuration progresses through *3* states.
///
/// ### 1. `WantsBindAddress`
///
/// The caller must supply a binding address for the client.
/// The following options are mutually exclusive:
///
///   - [`with_bind_default`](ClientConfigBuilder::with_bind_default): configures to use
///     the default bind address. This is generally the *default* choice for a client.
///   - [`with_bind_config`](ClientConfigBuilder::with_bind_config): configures
///     to bind an address determined by a configuration preset.
///   - [`with_bind_address`](ClientConfigBuilder::with_bind_address): configures
///     to bind a custom specified socket address.
///
/// Only one of these options can be selected during the client configuration process.
///
/// #### Examples:
///
/// ```
/// use wtransport::ClientConfig;
///
/// ClientConfig::builder().with_bind_default();
/// ```
///
/// ### 2. `WantsRootStore`
///
/// The caller must supply a TLS root store configuration for server certificate validation.
/// The following options are mutually exclusive:
///
/// - [`with_native_certs`](ClientConfigBuilder::with_native_certs): configures to use
///   root certificates found in the platform's native certificate store. This is the *default*
///   configuration as it uses root store installed on the current machine.
/// - [`with_custom_tls`](ClientConfigBuilder::with_custom_tls): sets the TLS client
///   configuration manually.
/// - (**unsafe**) [`with_no_cert_validation`](ClientConfigBuilder::with_no_cert_validation):
///   configure to skip server certificate validation. This might be handy for testing purpose
///   to accept *self-signed* certificate.
///
/// Only one of these options can be selected during the client configuration process.
///
/// #### Examples:
/// ```
/// use wtransport::ClientConfig;
///
/// ClientConfig::builder()
///     .with_bind_default()
///     .with_native_certs();
/// ```
///
/// ### 3. `WantsTransportConfigClient`
///
/// The caller can supply *additional* transport configurations.
/// Multiple options can be given at this stage. Once the configuration is completed, it is possible
/// to finalize with the method [`build()`](ClientConfigBuilder::build).
///
/// All these options can be omitted in the configuration; default values will be used.
///
/// - [`max_idle_timeout`](ClientConfigBuilder::max_idle_timeout)
/// - [`keep_alive_interval`](ClientConfigBuilder::keep_alive_interval)
/// - [`dns_resolver`](ClientConfigBuilder::dns_resolver)
///
/// #### Examples:
/// ```
/// use std::time::Duration;
/// use wtransport::ClientConfig;
///
/// let client_config = ClientConfig::builder()
///     .with_bind_default()
///     .with_native_certs()
///     .max_idle_timeout(Some(Duration::from_secs(30)))
///     .unwrap()
///     .keep_alive_interval(Some(Duration::from_secs(3)))
///     .build();
/// ```
pub struct ClientConfig {
    pub(crate) bind_address: SocketAddr,
    pub(crate) dual_stack_config: Ipv6DualStackConfig,
    pub(crate) quic_config: QuicClientConfig,
    pub(crate) dns_resolver: Box<dyn DnsResolver + Send + Sync>,
}

impl ClientConfig {
    /// Creates a builder to build up the client configuration.
    ///
    /// For more information, see the [`ClientConfigBuilder`] documentation.
    pub fn builder() -> ClientConfigBuilder<WantsBindAddress> {
        ClientConfigBuilder::default()
    }

    /// Returns a reference to the inner QUIC configuration.
    #[cfg(feature = "quinn")]
    #[cfg_attr(docsrs, doc(cfg(feature = "quinn")))]
    pub fn quic_config(&self) -> &quinn::ClientConfig {
        &self.quic_config
    }

    /// Returns a mutable reference to the inner QUIC configuration.
    #[cfg(feature = "quinn")]
    #[cfg_attr(docsrs, doc(cfg(feature = "quinn")))]
    pub fn quic_config_mut(&mut self) -> &mut quinn::ClientConfig {
        &mut self.quic_config
    }
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig::builder()
            .with_bind_default()
            .with_native_certs()
            .build()
    }
}

/// Client builder configuration.
///
/// The builder might have different state at compile time.
///
/// # Example
/// ```no_run
/// # use std::net::Ipv4Addr;
/// # use std::net::SocketAddr;
/// # use wtransport::ClientConfig;
/// let config = ClientConfig::builder().with_bind_default();
/// ```
#[must_use]
pub struct ClientConfigBuilder<State>(State);

impl ClientConfigBuilder<WantsBindAddress> {
    /// Configures for connecting binding ANY IP (allowing IP dual-stack).
    ///
    /// Bind port will be randomly picked.
    ///
    /// This is equivalent to: [`Self::with_bind_config`] with [`IpBindConfig::InAddrAnyDual`].
    pub fn with_bind_default(self) -> ClientConfigBuilder<WantsRootStore> {
        self.with_bind_config(IpBindConfig::InAddrAnyDual)
    }

    /// Sets the binding (local) socket address with a specific [`IpBindConfig`].
    ///
    /// Bind port will be randomly picked.
    pub fn with_bind_config(
        self,
        ip_bind_config: IpBindConfig,
    ) -> ClientConfigBuilder<WantsRootStore> {
        let ip_address: IpAddr = ip_bind_config.into_ip();

        match ip_address {
            IpAddr::V4(ip) => self.with_bind_address(SocketAddr::new(ip.into(), 0)),
            IpAddr::V6(ip) => self.with_bind_address_v6(
                SocketAddrV6::new(ip, 0, 0, 0),
                ip_bind_config.into_dual_stack_config(),
            ),
        }
    }

    /// Sets the binding (local) socket address for the endpoint.
    pub fn with_bind_address(self, address: SocketAddr) -> ClientConfigBuilder<WantsRootStore> {
        ClientConfigBuilder(WantsRootStore {
            bind_address: address,
            dual_stack_config: Ipv6DualStackConfig::OsDefault,
        })
    }

    /// Sets the binding (local) socket address for the endpoint.
    ///
    /// `dual_stack_config` allows/denies dual stack port binding.
    pub fn with_bind_address_v6(
        self,
        address: SocketAddrV6,
        dual_stack_config: Ipv6DualStackConfig,
    ) -> ClientConfigBuilder<WantsRootStore> {
        ClientConfigBuilder(WantsRootStore {
            bind_address: address.into(),
            dual_stack_config,
        })
    }
}

impl ClientConfigBuilder<WantsRootStore> {
    /// Configures the client to use native (local) root certificates for server validation.
    ///
    /// This method loads trusted root certificates from the system's certificate store,
    /// ensuring that your client can trust certificates signed by well-known authorities.
    ///
    /// It configures safe default TLS configuration.
    pub fn with_native_certs(self) -> ClientConfigBuilder<WantsTransportConfigClient> {
        self.with_custom_tls(Self::build_tls_config(Self::native_cert_store()))
    }

    /// Allows for manual configuration of a custom TLS setup using a provided
    /// [`rustls::ClientConfig`].
    ///
    /// This method is provided for advanced users who need fine-grained control over the
    /// TLS configuration. It allows you to pass a preconfigured [`rustls::ClientConfig`]
    /// instance to customize the TLS settings according to your specific requirements.
    ///
    /// For most use cases, it is recommended to use the [`with_native_certs`](Self::with_native_certs)
    /// method to configure TLS with safe defaults.
    pub fn with_custom_tls(
        self,
        tls_config: rustls::ClientConfig,
    ) -> ClientConfigBuilder<WantsTransportConfigClient> {
        let transport_config = TransportConfig::default();

        ClientConfigBuilder(WantsTransportConfigClient {
            bind_address: self.0.bind_address,
            dual_stack_config: self.0.dual_stack_config,
            tls_config,
            transport_config,
            dns_resolver: Box::<TokioDnsResolver>::default(),
        })
    }

    /// Configures the client to skip server certificate validation, potentially
    /// compromising security.
    ///
    /// This method is intended for advanced users and should be used with caution. It
    /// configures the client to bypass server certificate validation during the TLS
    /// handshake, effectively trusting any server certificate presented, even if it is
    /// not signed by a trusted certificate authority (CA). Using this method can expose
    /// your application to security risks.
    ///
    /// # Safety Note
    ///
    /// Using [`with_no_cert_validation`] should only be considered when you have a
    /// specific need to disable certificate validation. In most cases, it is strongly
    /// recommended to validate server certificates using trusted root certificates
    /// (e.g., [`with_native_certs`]) to ensure secure communication.
    ///
    /// However, this method can be useful in testing environments or situations where
    /// you intentionally want to skip certificate validation for specific use cases.
    ///
    /// [`with_native_certs`]: #method.with_native_certs
    /// [`with_no_cert_validation`]: #method.with_no_cert_validation
    #[cfg(feature = "dangerous-configuration")]
    #[cfg_attr(docsrs, doc(cfg(feature = "dangerous-configuration")))]
    pub fn with_no_cert_validation(self) -> ClientConfigBuilder<WantsTransportConfigClient> {
        let mut tls_config = Self::build_tls_config(RootCertStore::empty());
        tls_config
            .dangerous()
            .set_certificate_verifier(Arc::new(dangerous_configuration::NoServerVerification));

        let transport_config = TransportConfig::default();

        ClientConfigBuilder(WantsTransportConfigClient {
            bind_address: self.0.bind_address,
            dual_stack_config: self.0.dual_stack_config,
            tls_config,
            transport_config,
            dns_resolver: Box::<TokioDnsResolver>::default(),
        })
    }

    fn native_cert_store() -> RootCertStore {
        let mut root_store = RootCertStore::empty();

        let _var_restore_guard = utils::remove_var_tmp("SSL_CERT_FILE");

        match rustls_native_certs::load_native_certs() {
            Ok(certs) => {
                for c in certs {
                    let _ = root_store.add(&rustls::Certificate(c.0));
                }
            }
            Err(_error) => {}
        }

        root_store
    }

    fn build_tls_config(root_store: RootCertStore) -> TlsClientConfig {
        let mut config = TlsClientConfig::builder()
            .with_safe_default_cipher_suites()
            .with_safe_default_kx_groups()
            .with_safe_default_protocol_versions()
            .expect("Safe protocols should not error")
            .with_root_certificates(root_store)
            .with_no_client_auth();

        config.alpn_protocols = [WEBTRANSPORT_ALPN.to_vec()].to_vec();
        config
    }
}

impl ClientConfigBuilder<WantsTransportConfigClient> {
    /// Completes configuration process.
    #[must_use]
    pub fn build(self) -> ClientConfig {
        let mut quic_config = QuicClientConfig::new(Arc::new(self.0.tls_config));
        quic_config.transport_config(Arc::new(self.0.transport_config));

        ClientConfig {
            bind_address: self.0.bind_address,
            dual_stack_config: self.0.dual_stack_config,
            quic_config,
            dns_resolver: self.0.dns_resolver,
        }
    }

    /// Maximum duration of inactivity to accept before timing out the connection.
    ///
    /// The true idle timeout is the minimum of this and the peer's own max idle timeout. `None`
    /// represents an infinite timeout.
    ///
    /// **WARNING**: If a peer or its network path malfunctions or acts maliciously, an infinite
    /// idle timeout can result in permanently hung futures!
    pub fn max_idle_timeout(
        mut self,
        idle_timeout: Option<Duration>,
    ) -> Result<Self, InvalidIdleTimeout> {
        let idle_timeout = idle_timeout
            .map(quinn::IdleTimeout::try_from)
            .transpose()
            .map_err(|_| InvalidIdleTimeout)?;

        self.0.transport_config.max_idle_timeout(idle_timeout);

        Ok(self)
    }

    /// Period of inactivity before sending a keep-alive packet
    ///
    /// Keep-alive packets prevent an inactive but otherwise healthy connection from timing out.
    ///
    /// `None` to disable, which is the default. Only one side of any given connection needs keep-alive
    /// enabled for the connection to be preserved. Must be set lower than the
    /// [`max_idle_timeout`](Self::max_idle_timeout) of both peers to be effective.
    pub fn keep_alive_interval(mut self, interval: Option<Duration>) -> Self {
        self.0.transport_config.keep_alive_interval(interval);
        self
    }

    /// Sets the *DNS* resolver used during [`Endpoint::connect`](crate::Endpoint::connect).
    ///
    /// Default configuration uses [`TokioDnsResolver`].
    pub fn dns_resolver(mut self, dns_resolver: Box<dyn DnsResolver + Send + Sync>) -> Self {
        self.0.dns_resolver = dns_resolver;
        self
    }
}

impl Default for ServerConfigBuilder<WantsBindAddress> {
    fn default() -> Self {
        Self(WantsBindAddress {})
    }
}

impl Default for ClientConfigBuilder<WantsBindAddress> {
    fn default() -> Self {
        Self(WantsBindAddress {})
    }
}

/// Config builder state where the caller must supply binding address.
pub struct WantsBindAddress {}

/// Config builder state where the caller must supply TLS certificate.
pub struct WantsCertificate {
    bind_address: SocketAddr,
    dual_stack_config: Ipv6DualStackConfig,
}

/// Config builder state where the caller must supply TLS root store.
pub struct WantsRootStore {
    bind_address: SocketAddr,
    dual_stack_config: Ipv6DualStackConfig,
}

/// Config builder state where transport properties can be set.
pub struct WantsTransportConfigServer {
    bind_address: SocketAddr,
    dual_stack_config: Ipv6DualStackConfig,
    tls_config: TlsServerConfig,
    transport_config: quinn::TransportConfig,
    migration: bool,
}

/// Config builder state where transport properties can be set.
pub struct WantsTransportConfigClient {
    bind_address: SocketAddr,
    dual_stack_config: Ipv6DualStackConfig,
    tls_config: TlsClientConfig,
    transport_config: quinn::TransportConfig,
    dns_resolver: Box<dyn DnsResolver + Send + Sync>,
}

#[cfg(feature = "dangerous-configuration")]
mod dangerous_configuration {
    use rustls::client::ServerCertVerified;
    use rustls::client::ServerCertVerifier;

    pub(super) struct NoServerVerification;

    impl ServerCertVerifier for NoServerVerification {
        fn verify_server_cert(
            &self,
            _end_entity: &rustls::Certificate,
            _intermediates: &[rustls::Certificate],
            _server_name: &rustls::ServerName,
            _scts: &mut dyn Iterator<Item = &[u8]>,
            _ocsp_response: &[u8],
            _now: std::time::SystemTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }
    }
}

/// A type alias representing a dynamic future for asynchronous DNS resolution.
///
/// `DynFutureResolver` is a trait object type alias that represents a future yielding
/// a result of DNS resolution. The future's output is of type `std::io::Result<Option<SocketAddr>>`,
/// indicating the result of the DNS resolution operation.
pub type DynFutureResolver = dyn Future<Output = std::io::Result<Option<SocketAddr>>> + Send;

/// A trait for asynchronously resolving domain names to IP addresses using DNS.
pub trait DnsResolver {
    /// Resolves a domain name to one IP address.
    ///
    /// It returns a [`Pin<Box<DynFutureResolver>>`](DynFutureResolver) that represents
    /// the asynchronous result of the DNS resolution.
    fn resolve(&self, host: &str) -> Pin<Box<DynFutureResolver>>;
}

/// A DNS resolver implementation using the *Tokio* asynchronous runtime.
///
/// Internally, it uses [`tokio::net::lookup_host`].
#[derive(Default)]
pub struct TokioDnsResolver;

impl DnsResolver for TokioDnsResolver {
    fn resolve(&self, host: &str) -> Pin<Box<DynFutureResolver>> {
        let host = host.to_string();
        Box::pin(async move { Ok(tokio::net::lookup_host(host).await?.next()) })
    }
}

mod utils {
    use std::env;
    use std::ffi::OsStr;
    use std::ffi::OsString;

    pub struct VarRestoreGuard {
        key: OsString,
        value: Option<OsString>,
    }

    impl Drop for VarRestoreGuard {
        fn drop(&mut self) {
            if let Some(value) = self.value.take() {
                env::set_var(self.key.clone(), value);
            }
        }
    }

    pub fn remove_var_tmp<K: AsRef<OsStr>>(key: K) -> VarRestoreGuard {
        let value = env::var_os(key.as_ref());

        env::remove_var(key.as_ref());

        VarRestoreGuard {
            key: key.as_ref().to_os_string(),
            value,
        }
    }
}
