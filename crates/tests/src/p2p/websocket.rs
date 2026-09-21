//! Lobby discovery over the real WebSocket transport.
//!
//! The UDP tests prove the lobby is transport-agnostic; this one proves the WebSocket glue dials.
//! What differs is worth stating, because it is what the glue exists for:
//!
//! - Native socket and scheme travel in dial context, independently of the routing alias.
//! - Accepted native sessions report ephemeral source ports. Those are never admitted identities:
//!   the bounded first announcement resolves the sender before Connected.

use super::stepper::{
    Stepper, TestTransport, explicit_retry_reconnects_members, lobby_id, next_base_port, peer_id,
};
use lightyear::p2p::LobbyIdPolicy;
use lightyear::prelude::*;
use lightyear::websocket::client::WebSocketClientIo;
use test_log::test;

#[test]
fn chosen_websocket_identities_are_final_at_connection_admission() {
    super::stepper::chosen_identity_admission(TestTransport::WebSocket);
}

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
        // Admission resolves the endpoint alias before any observer can see an ephemeral source.
        let mut expected = vec![
            peer_id(TestTransport::WebSocket, base_port, 0),
            peer_id(TestTransport::WebSocket, base_port, 1),
        ];
        expected.sort_unstable();
        assert_eq!(first.roster(), expected);
        assert_eq!(first.roster(), second.roster());
    }
    assert_eq!(stepper.peers[0].connected_links(), 1);
    assert_eq!(stepper.peers[1].connected_links(), 1);
    for (slot, peer) in stepper.peers.iter_mut().enumerate() {
        let advertised = peer_id(TestTransport::WebSocket, base_port, 1 - slot as u8);
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
        let local = peer_id(TestTransport::WebSocket, base_port, slot as u8);
        let mut expected: Vec<_> = (0..3)
            .map(|slot| peer_id(TestTransport::WebSocket, base_port, slot))
            .filter(|id| *id != local)
            .collect();
        expected.sort_unstable();
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
