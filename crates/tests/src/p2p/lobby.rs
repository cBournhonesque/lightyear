//! Lobby discovery over the real UDP transport.
//!
//! These tests are the proof that the lobby is transport-agnostic: the same [`LobbyPlugin`] that
//! other transports drive is driven here by UDP's glue, through the same [`DialPeer`] event and the
//! same scope — Links carrying [`P2P`]. Nothing in `lightyear_p2p` knows about UDP.
//!
//! Native addresses live in dial context; an endpoint can choose an independent routing alias.
//! A UDP route is only Linked until the peer's announcement resolves its identity.

use super::stepper::{Stepper, TestTransport, lobby_id, next_base_port, peer_id};
use lightyear::p2p::{LobbyId, LobbyIdPolicy};
use lightyear::prelude::*;
use test_log::test;

fn other_lobby_id() -> LobbyId {
    LobbyId::from_bytes([9; 32])
}
#[test]
fn chosen_udp_identities_are_final_at_connection_admission() {
    super::stepper::chosen_identity_admission(TestTransport::Udp);
}

#[test]
fn two_peers_discover_each_other_over_udp_and_start_a_session() {
    let base_port = next_base_port();
    let mut stepper = Stepper::new(
        TestTransport::Udp,
        base_port,
        [
            LobbyIdPolicy::Pinned(Some(lobby_id())),
            LobbyIdPolicy::Adopt,
        ],
    );
    // Only one side is seeded. The peer that is dialed learns the dialer from the datagram it
    // receives, so a UDP pair does not need a dial in each direction.
    stepper.bootstrap(base_port, 0, &[1]);

    // The endpoint knows its own identity before any remote link is admitted.
    assert_eq!(
        stepper.peers[0].lobby().waiting().collect::<Vec<_>>(),
        vec![peer_id(TestTransport::Udp, base_port, 1)]
    );
    assert_eq!(
        stepper.peers[0].lobby().roster(),
        [peer_id(TestTransport::Udp, base_port, 0)]
    );

    stepper.start_session_when_ready(&[0, 1]);
    stepper.wait_for_session(&[0, 1]);

    {
        let first = stepper.peers[0].lobby();
        let second = stepper.peers[1].lobby();

        // The joiner adopted the lobby it was invited to rather than inventing one.
        assert_eq!(first.id(), Some(lobby_id()));
        assert_eq!(second.id(), Some(lobby_id()));

        // Both derived the same two-member roster, so `P2PStart` froze the same cohort and the
        // session's roster hash agreed. Both sides resolve the same opaque UDP endpoint aliases.
        assert_eq!(first.roster(), second.roster());
        let mut expected = vec![
            peer_id(TestTransport::Udp, base_port, 0),
            peer_id(TestTransport::Udp, base_port, 1),
        ];
        expected.sort_unstable();
        assert_eq!(first.roster(), expected);

        assert_eq!(first.connected_members().count(), 1);
        assert!(first.waiting().next().is_none(), "nobody is left waiting");
    }
    assert_eq!(stepper.peers[0].connected_links(), 1);
    assert_eq!(stepper.peers[1].connected_links(), 1);
}

#[test]
fn the_two_peers_agree_on_slots() {
    let base_port = next_base_port();
    let mut stepper = Stepper::new(
        TestTransport::Udp,
        base_port,
        [
            LobbyIdPolicy::Pinned(Some(lobby_id())),
            LobbyIdPolicy::Adopt,
        ],
    );
    stepper.bootstrap(base_port, 0, &[1]);
    stepper.start_session_when_ready(&[0, 1]);
    stepper.wait_for_session(&[0, 1]);

    // Slots come from the sorted roster, so both peers assign the same slot to the same peer
    // without exchanging anything but membership.
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
        "the two peers must occupy different slots"
    );
}

#[test]
fn a_third_peer_is_discovered_and_dialed_without_being_configured() {
    // Only peer 0 is seeded. Peers 1 and 2 are never told about each other — not their addresses,
    // not that they exist. The Link between them can only come from peer 0 announcing who it knows,
    // and the address to dial comes from the announced native dial context.
    let base_port = next_base_port();
    let mut stepper = Stepper::new(
        TestTransport::Udp,
        base_port,
        [
            LobbyIdPolicy::Pinned(Some(lobby_id())),
            LobbyIdPolicy::Adopt,
            LobbyIdPolicy::Adopt,
        ],
    );
    stepper.bootstrap(base_port, 0, &[1, 2]);

    stepper.start_session_when_ready(&[0, 1, 2]);
    stepper.wait_for_session(&[0, 1, 2]);

    let slots = [0u8, 1, 2];
    for slot in slots {
        let lobby = stepper.peers[slot as usize].lobby();
        // Every peer sees the two others as members, and never itself: the member set is the roster
        // minus the local peer.
        for other in slots.into_iter().filter(|other| *other != slot) {
            assert!(
                lobby.is_member(peer_id(TestTransport::Udp, base_port, other)),
                "peer {slot} should know peer {other} as a member"
            );
        }
        assert!(
            !lobby.is_member(peer_id(TestTransport::Udp, base_port, slot)),
            "peer {slot} should not list itself as a member"
        );
        assert_eq!(
            lobby.connected_members().count(),
            2,
            "peer {slot} should be connected to both other members"
        );
    }
    for slot in slots {
        assert_eq!(
            stepper.peers[slot as usize].connected_links(),
            2,
            "peer {slot} should have a Link to both other peers"
        );
    }

    // All three derived the same roster, so the barrier could agree.
    let rosters: Vec<Vec<PeerId>> = stepper
        .peers
        .iter()
        .map(|peer| peer.lobby().roster())
        .collect();
    assert_eq!(rosters[0], rosters[1]);
    assert_eq!(rosters[1], rosters[2]);
    assert_eq!(rosters[0].len(), 3);
}

#[test]
fn a_peer_pinned_to_another_lobby_is_excluded_from_discovery() {
    // A stranger dials the founder but insists on a different lobby. This is what the lobby id is
    // for: without it the two gatherings would merge into one peer set.
    let base_port = next_base_port();
    let mut stepper = Stepper::new(
        TestTransport::Udp,
        base_port,
        [
            LobbyIdPolicy::Pinned(Some(lobby_id())),
            LobbyIdPolicy::Adopt,
            LobbyIdPolicy::Pinned(Some(other_lobby_id())),
        ],
    );
    stepper.bootstrap(base_port, 0, &[1, 2]);

    // Let the announces be exchanged and reacted to.
    stepper.step(400);

    // The founder and its rightful joiner form one lobby; the stranger is recorded as foreign,
    // contributes nothing to the roster, and is never advertised onward.
    assert_eq!(
        stepper.peers[0].lobby().members().collect::<Vec<_>>(),
        vec![peer_id(TestTransport::Udp, base_port, 1)]
    );
    assert_eq!(stepper.peers[0].lobby().id(), Some(lobby_id()));
    assert_eq!(stepper.peers[1].lobby().id(), Some(lobby_id()));
    assert!(!stepper.peers[0].lobby().roster().contains(&peer_id(
        TestTransport::Udp,
        base_port,
        2
    )));
    assert!(!stepper.peers[1].lobby().roster().contains(&peer_id(
        TestTransport::Udp,
        base_port,
        2
    )));
    assert!(stepper.peers[2].lobby().members().next().is_none());

    // The lobby leaves the stranger's Link alone: what to do with a connected peer from another
    // lobby is the application's decision.
    assert!(
        stepper.peers[0]
            .lobby()
            .is_connected(peer_id(TestTransport::Udp, base_port, 2)),
        "the lobby does not unlink a peer from another lobby"
    );
}

#[test]
fn an_unreachable_peer_stays_unconfirmed_and_is_retried() {
    // Slot 9 is never started, so nothing is listening at its address: the dial goes out and no
    // announce ever comes back.
    let base_port = next_base_port();
    let mut stepper = Stepper::new(
        TestTransport::Udp,
        base_port,
        [LobbyIdPolicy::Pinned(Some(lobby_id()))],
    );
    stepper.bootstrap(base_port, 0, &[9]);

    stepper.step(200);
    assert_eq!(
        stepper.peers[0].lobby().members().count(),
        0,
        "nothing replied, so the peer was never confirmed"
    );
    // A native UDP route is not proof of a remote peer identity.
    assert!(
        !stepper.peers[0]
            .lobby()
            .is_connected(peer_id(TestTransport::Udp, base_port, 9))
    );
    assert!(
        !stepper.peers[0]
            .lobby()
            .is_member(peer_id(TestTransport::Udp, base_port, 9))
    );

    assert_eq!(stepper.peers[0].connected_links(), 0);
    stepper.peers[0]
        .app
        .world_mut()
        .trigger(lightyear::p2p::P2PStart);
    stepper.retry_failed(0);

    // Retrying does not conjure the peer: it stays unconfirmed while nothing is there.
    stepper.step(50);
    assert_eq!(stepper.peers[0].lobby().members().count(), 0);
    assert_eq!(stepper.peers[0].connected_links(), 0);
    assert!(!stepper.peers[0].in_session());
}
