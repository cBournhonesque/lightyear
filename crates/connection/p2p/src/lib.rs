//! Deterministic peer-to-peer session lifecycle for Lightyear.
//!
//! Applications declare [`P2P::Inactive`](lightyear_connection::p2p::P2P::Inactive) Links, then
//! trigger [`P2PStart`] when the desired cohort is present. The current inactive Links become
//! candidates; the session waits for them and the synchronized input timeline, chooses one shared
//! future start tick, then marks them joined.
//!
//! [`P2PSessionPlugin`] owns the barrier bookkeeping; applications do not need to configure a
//! [`P2PSession`]. The session does not own peer discovery or Link connection. Stopping it can
//! either preserve every Link for a lobby/rematch or unlink every currently declared P2P Link.
//!
//! [`LobbyPlugin`] adds optional peer discovery on top: it turns "some peers I can dial" into a
//! complete cohort. An application that already knows its peers does not need it — declaring the
//! Links is what selects the cohort either way, and the lobby is transport-agnostic, reading only
//! [`P2P`](lightyear_connection::p2p::P2P) Links and emitting [`DialPeer`] for a transport's glue
//! to handle.

#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

mod lobby;
mod session;

pub use lobby::*;
pub use session::*;

/// Commonly used P2P session and lobby types.
///
/// [`Lobby`] itself is deliberately **not** re-exported here. It is a common name for an
/// application's own lobby type, and a glob-imported `Lobby` would make every such application
/// ambiguous. Reach it as `lightyear_p2p::Lobby`.
pub mod prelude {
    pub use crate::lobby::{
        DialPeer, LobbyAnnounce, LobbyId, LobbyIdPolicy, LobbyPlugin, PeerState,
    };
    pub use crate::{
        P2PSession, P2PSessionPlugin, P2PSessionState, P2PStart, P2PStarted, P2PStop, P2PStopped,
    };
}
