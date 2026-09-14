//! Lobby discovery over the real WebSocket transport.
//!
//! The UDP tests prove the lobby is transport-agnostic; this one proves the WebSocket glue dials.
//! What differs is worth stating, because it is what the glue exists for:
//!
//! - WebSocket authenticates nothing, so the address is all a peer needs and the announce carries no
//!   dial context.
//! - A stream transport cannot tell who dialed it: the accepted session reports the ephemeral port
//!   the peer connected from, not the endpoint it listens on. The announce names its sender, and the
//!   lobby re-keys the Link to it — without that the two peers would disagree on the roster.
//! - Only `RawClient` and Steam promote `Linked` to `Connected`, so the glue does it.

use super::stepper::{
    Stepper, TestTransport, explicit_retry_reconnects_members, lobby_id, next_base_port, peer_id,
};
use lightyear::p2p::LobbyIdPolicy;
use lightyear::prelude::*;
use lightyear::websocket::client::WebSocketClientIo;
use test_log::test;

#[test]
fn two_peers_discover_each_other_over_websocket_and_start_a_session() {
    let base_port = next_base_port();
    let mut stepper = Stepper::new(
        TestTransport::WebSocket,
        base_port,
        [
            LobbyIdPolicy::Pinned(Some(lobby_id())),
            LobbyIdPolicy::Adopt,
        ],
    );
    // Only the host is seeded. The dial tie-break picks the one direction.
    stepper.bootstrap(base_port, 0, &[1]);

    stepper.start_session_when_ready(&[0, 1]);
    stepper.wait_for_session(&[0, 1]);

    {
        let first = stepper.peers[0].lobby();
        let second = stepper.peers[1].lobby();
        assert_eq!(first.id(), Some(lobby_id()));
        assert_eq!(second.id(), Some(lobby_id()));
        // Both sides call the *endpoint* address of the peer their peer id, even though the acceptor
        // only ever saw the dialer's ephemeral port. That agreement is what the re-key buys.
        assert_eq!(
            first.roster(),
            vec![peer_id(base_port, 0), peer_id(base_port, 1)]
        );
        assert_eq!(first.roster(), second.roster());
    }
    assert_eq!(stepper.peers[0].connected_links(), 1);
    assert_eq!(stepper.peers[1].connected_links(), 1);
    for (slot, peer) in stepper.peers.iter_mut().enumerate() {
        let advertised = peer_id(base_port, 1 - slot as u8);
        let world = peer.app.world_mut();
        let entity = world.resource::<NetworkingMetadata>().peer_map[&advertised];
        assert_eq!(world.get::<RemoteId>(entity), Some(&RemoteId(advertised)));
        assert!(world.get::<Connected>(entity).is_some());
        assert_eq!(
            world
                .resource::<NetworkingMetadata>()
                .peer_map
                .values()
                .filter(|owner| **owner == entity)
                .count(),
            1,
            "the accepted Link must no longer be looked up by its ephemeral address"
        );
    }
}

#[test]
fn the_websocket_peers_agree_on_slots() {
    let base_port = next_base_port();
    let mut stepper = Stepper::new(
        TestTransport::WebSocket,
        base_port,
        [
            LobbyIdPolicy::Pinned(Some(lobby_id())),
            LobbyIdPolicy::Adopt,
        ],
    );
    stepper.bootstrap(base_port, 0, &[1]);
    stepper.start_session_when_ready(&[0, 1]);
    stepper.wait_for_session(&[0, 1]);

    let slots_of = |slot: u8| -> Vec<(PeerId, Option<u8>)> {
        let lobby = stepper.peers[slot as usize].lobby();
        lobby
            .roster()
            .iter()
            .map(|peer| (*peer, lobby.slot_of(*peer)))
            .collect()
    };
    assert_eq!(slots_of(0), slots_of(1));
    assert_ne!(
        stepper.peers[0].lobby().local_slot(),
        stepper.peers[1].lobby().local_slot(),
    );
}

#[test]
fn three_peers_join_one_seed_without_identity_collisions() {
    let base_port = next_base_port();
    let mut stepper = Stepper::new(
        TestTransport::WebSocket,
        base_port,
        [
            LobbyIdPolicy::Pinned(Some(lobby_id())),
            LobbyIdPolicy::Adopt,
            LobbyIdPolicy::Adopt,
        ],
    );
    stepper.bootstrap(base_port, 1, &[0]);
    stepper.bootstrap(base_port, 2, &[0]);
    stepper.start_session_when_ready(&[0, 1, 2]);
    stepper.wait_for_session(&[0, 1, 2]);
    for (slot, peer) in stepper.peers.iter_mut().enumerate() {
        assert_eq!(peer.connected_links(), 2);
        let local = peer_id(base_port, slot as u8);
        let expected: Vec<_> = (0..3)
            .map(|slot| peer_id(base_port, slot))
            .filter(|id| *id != local)
            .collect();
        let metadata = peer.app.world().resource::<NetworkingMetadata>();
        let mut actual: Vec<_> = metadata.peer_map.keys().copied().collect();
        actual.sort_unstable();
        assert_eq!(
            actual, expected,
            "all peers must resolve by endpoint identity"
        );
    }
}

#[test]
fn explicit_retry_reconnects_websocket_members_in_both_directions() {
    let base_port = next_base_port();
    let mut stepper = Stepper::new(
        TestTransport::WebSocket,
        base_port,
        [
            LobbyIdPolicy::Pinned(Some(lobby_id())),
            LobbyIdPolicy::Adopt,
        ],
    );
    explicit_retry_reconnects_members(&mut stepper, base_port, |world, entity| {
        world.get::<WebSocketClientIo>(entity).is_some()
    });
}
