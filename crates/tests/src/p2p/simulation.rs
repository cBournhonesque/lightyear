//! Exercise input rollback and sequential catch-up through real lobby-discovered UDP Links.

use super::stepper::{Stepper, TestTransport, lobby_id, next_base_port, peer_id};
use crate::protocol::{CompA, NativeInput};
use bevy::prelude::*;
use core::hash::Hasher;
use lightyear::input::native::prelude::NativeBuffer;
use lightyear::p2p::{Lobby, LobbyIdPolicy};
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::native::{ActionState, InputMarker, NativeStateSequence};
use lightyear::prelude::*;
use lightyear_core::history_buffer::HistoryState;
use lightyear_deterministic_replication::Deterministic;
use lightyear_deterministic_replication::join_catch_up::P2PCatchUpReplay;
use lightyear_deterministic_replication::prelude::{ChecksumPlugin, JoinCatchUpPlugin};
use lightyear_inputs::client::InputSystems;
use lightyear_prediction::manager::LastConfirmedInput;

#[derive(Component)]
struct Player(PeerId);

fn setup(app: &mut App) {
    app.insert_resource(PredictionManager::default());
    app.insert_resource(P2PSession::default().with_start_delay_ticks(20));
    app.component::<CompA>().predict();
    app.component::<CompA>()
        .add_custom_hash(|position, hasher| hasher.write_u32(position.0.to_bits()));
    app.add_plugins((
        ChecksumPlugin,
        JoinCatchUpPlugin::<NativeStateSequence<NativeInput>>::enabled(),
    ));
    app.add_observer(|event: On<P2PJoinRequested>, mut commands: Commands| {
        commands.trigger(P2PJoinAdmission {
            peer_id: event.peer_id,
            result: Ok(()),
        });
    });
    app.add_observer(|event: On<P2PJoinRejected>| panic!("join rejected: {:?}", event.reason));
    app.add_observer(start_world);
    app.add_observer(prepare_catch_up);
    app.add_observer(rebuild_world);
    app.add_observer(append_player);
    app.add_systems(
        FixedPreUpdate,
        write_inputs.in_set(InputSystems::WriteClientInputs),
    );
    app.add_systems(FixedUpdate, move_players);
}

fn spawn_player(
    commands: &mut Commands,
    lobby: &Lobby,
    metadata: &NetworkingMetadata,
    peer: PeerId,
    slot: usize,
) {
    let mut target = PreSpawned::new(peer.to_bits());
    if lobby.local() != Some(peer) {
        target = target.for_receiver(metadata.peer_map[&peer]);
    }
    let mut entity = commands.spawn((
        Player(peer),
        CompA(slot as f32 * 100.0),
        Deterministic,
        DeterministicPredicted {
            skip_despawn: true,
            enable_rollback_after: 0,
        },
        target,
        ActionState::<NativeInput>::default(),
        NativeBuffer::<NativeInput>::default(),
    ));
    if lobby.local() == Some(peer) {
        entity.insert(InputMarker::<NativeInput>::default());
    }
}

fn start_world(
    _: On<P2PStarted>,
    mut commands: Commands,
    lobby: Res<Lobby>,
    metadata: Res<NetworkingMetadata>,
) {
    for (slot, peer) in lobby.roster().into_iter().enumerate() {
        spawn_player(&mut commands, &lobby, &metadata, peer, slot);
    }
}

fn prepare_catch_up(
    _: On<P2PJoinCatchUp>,
    mut commands: Commands,
    session: Res<P2PSession>,
    lobby: Res<Lobby>,
    metadata: Res<NetworkingMetadata>,
) {
    for (slot, peer) in session.started_peers().into_iter().enumerate() {
        spawn_player(&mut commands, &lobby, &metadata, peer, slot);
    }
}

fn rebuild_world(
    event: On<P2PCatchUpReplay>,
    mut commands: Commands,
    players: Query<Entity, With<Player>>,
    lobby: Res<Lobby>,
    metadata: Res<NetworkingMetadata>,
) {
    for entity in &players {
        commands.entity(entity).despawn();
    }
    for (slot, peer) in event.initial_peers.iter().copied().enumerate() {
        spawn_player(&mut commands, &lobby, &metadata, peer, slot);
    }
}

fn append_player(
    event: On<P2PJoined>,
    mut commands: Commands,
    players: Query<Entity, With<Player>>,
    lobby: Res<Lobby>,
    metadata: Res<NetworkingMetadata>,
) {
    spawn_player(
        &mut commands,
        &lobby,
        &metadata,
        event.peer_id,
        players.iter().count(),
    );
}

fn write_inputs(mut players: Query<&mut ActionState<NativeInput>, With<InputMarker<NativeInput>>>) {
    for mut input in &mut players {
        input.0 = NativeInput(1);
    }
}

fn move_players(mut players: Query<(&ActionState<NativeInput>, &mut CompA)>) {
    for (input, mut position) in &mut players {
        position.0 += f32::from(input.0.0);
    }
}

fn assert_same_world(stepper: &mut Stepper, base: u16, slots: &[usize]) {
    let tick = slots
        .iter()
        .map(|&slot| {
            let world = stepper.peers[slot].app.world();
            world
                .resource::<LastConfirmedInput>()
                .get()
                .unwrap()
                .min(world.resource::<LocalTimeline>().tick())
        })
        .min()
        .unwrap();
    let mut expected_peers: Vec<_> = slots
        .iter()
        .map(|&slot| peer_id(base, slot as u8))
        .collect();
    expected_peers.sort_unstable();
    let mut reference = None;
    for &slot in slots {
        let world = stepper.peers[slot].app.world_mut();
        let mut state: Vec<_> = world
            .query::<(&Player, &PredictionHistory<CompA>)>()
            .iter(world)
            .map(|(player, history)| {
                let Some(HistoryState::Updated(position)) = history.get_state(tick) else {
                    panic!("missing player {:?} at {tick:?} on peer {slot}", player.0);
                };
                (player.0, position.0)
            })
            .collect();
        state.sort_unstable_by_key(|(peer, _)| *peer);
        assert_eq!(
            state.iter().map(|(peer, _)| *peer).collect::<Vec<_>>(),
            expected_peers
        );
        if let Some(reference) = &reference {
            assert_eq!(&state, reference, "divergence at {tick:?} on peer {slot}");
        } else {
            reference = Some(state);
        }
    }
}

#[test]
fn sequential_join_replays_founders_before_a_lower_sorted_newcomer() {
    let base = next_base_port();
    let mut stepper = Stepper::new_with_setup(
        TestTransport::Udp,
        base,
        [
            LobbyIdPolicy::Adopt,
            LobbyIdPolicy::Pinned(Some(lobby_id())),
            LobbyIdPolicy::Adopt,
            LobbyIdPolicy::Adopt,
        ],
        setup,
    );
    stepper.bootstrap(base, 1, &[2]);
    stepper.start_session_when_ready(&[1, 2]);
    stepper.wait_for_session(&[1, 2]);
    stepper.step(40);
    assert_same_world(&mut stepper, base, &[1, 2]);

    // Slot zero sorts ahead of both founders. Its replay must preserve their original slots.
    for (newcomer, remote_peer, members) in [(0, 1, vec![0, 1, 2]), (3, 0, vec![0, 1, 2, 3])] {
        stepper.bootstrap(base, newcomer, &[remote_peer]);
        stepper.wait_until(|stepper| {
            members.iter().all(|&slot| {
                stepper.peers[slot]
                    .app
                    .world()
                    .resource::<Lobby>()
                    .connected_members()
                    .count()
                    == members.len() - 1
            })
        });
        stepper.peers[newcomer as usize]
            .app
            .world_mut()
            .trigger(P2PJoin {
                remote_peer: peer_id(base, remote_peer),
            });
        stepper.wait_for_session(&[newcomer]);
        stepper.step(40);
        assert_same_world(&mut stepper, base, &members);
    }
}
