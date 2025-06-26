use bevy_ecs::resource::Resource;

use crate::{ConnectToken, Error, Key, generate_key};
use core::net::SocketAddr;
use core::str::FromStr;

#[derive(Resource, Default, Clone)]
#[allow(clippy::large_enum_variant)]
/// Struct used to authenticate with the server when using the Netcode connection.
///
/// Netcode is a standard to establish secure connections between clients and game servers on top of
/// an unreliable unordered transport such as UDP.
/// You can read more about it here: `<https://github.com/mas-bandwidth/netcode/blob/main/STANDARD.md>`
///
/// The client sends a [`ConnectToken`] to the game server to start the connection process.
///
/// There are several ways to obtain a [`ConnectToken`]:
/// - the client can request a [`ConnectToken`] via a secure (e.g. HTTPS) connection from a backend server.
///   The server must use the same `protocol_id` and `private_key` as the game servers.
///   The backend server could be a dedicated webserver; or the game server itself, if it has a way to
///   establish secure connection.
/// - when testing, it can be convenient for the client to create its own [`ConnectToken`] manually.
///   You can use `Authentication::Manual` for those cases.
pub enum Authentication {
    /// Use a [`ConnectToken`] to authenticate with the game server.
    ///
    /// The client must have already received the [`ConnectToken`] from the backend.
    /// (The backend will generate a new `client_id` for the user, and use that to generate the
    /// [`ConnectToken`])
    Token(ConnectToken),
    /// The client can build a [`ConnectToken`] manually.
    ///
    /// This is only useful for testing purposes. In production, the client should not have access
    /// to the `private_key`.
    Manual {
        server_addr: SocketAddr,
        client_id: u64,
        private_key: Key,
        protocol_id: u64,
    },
    #[default]
    /// The client has no [`ConnectToken`], so it cannot connect to the game server yet.
    ///
    /// This is provided so that you can still build a Client while waiting
    /// to receive a [`ConnectToken`] from the backend.
    None,
}

impl Authentication {
    /// Returns true if the Authentication contains a [`ConnectToken`] that can be used to
    /// connect to the game server
    pub fn has_token(&self) -> bool {
        matches!(self, Authentication::Token(..))
    }

    pub fn get_token(
        self,
        client_timeout_secs: i32,
        token_expire_secs: i32,
    ) -> Result<ConnectToken, Error> {
        Ok(match self {
            Authentication::Token(token) => token,
            Authentication::Manual {
                server_addr,
                client_id,
                private_key,
                protocol_id,
            } => ConnectToken::build(server_addr, protocol_id, client_id, private_key)
                .timeout_seconds(client_timeout_secs)
                .expire_seconds(token_expire_secs)
                .generate()?,
            Authentication::None => {
                // create a fake connect token so that we can build a NetcodeClient
                ConnectToken::build(
                    SocketAddr::from_str("0.0.0.0:0").unwrap(),
                    0,
                    0,
                    generate_key(),
                )
                .timeout_seconds(client_timeout_secs)
                .generate()?
            }
        })
    }
}

impl core::fmt::Debug for Authentication {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Authentication::Token(_) => write!(f, "Token(<connect_token>)"),
            Authentication::Manual {
                server_addr,
                client_id,
                private_key,
                protocol_id,
            } => f
                .debug_struct("Manual")
                .field("server_addr", server_addr)
                .field("client_id", client_id)
                .field("private_key", private_key)
                .field("protocol_id", protocol_id)
                .finish(),
            Authentication::None => write!(f, "None"),
        }
    }
}
