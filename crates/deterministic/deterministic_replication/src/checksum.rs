//! Utilities for computing checksums for data integrity verification.
//!
//! Conventional clients send checksums to the authoritative server. In direct P2P mode, every
//! peer broadcasts and validates checksums against every other peer.
//!
//! Note: we don't have a good way to guarantee that we are iterating through entities in a stable order on both client and server.
//! Because of this, we will compute an order-independent checksum by only hashing component data and then XOR-ing the results together.

use crate::archetypes::ChecksumWorld;
#[cfg(all(feature = "client", feature = "replication"))]
use crate::late_join::CatchUpManager;
use crate::plugin::DeterministicReplicationPlugin;
use alloc::collections::BTreeMap;
#[cfg(feature = "p2p")]
use alloc::vec::Vec;
#[cfg(feature = "server")]
use bevy_app::FixedLast;
use bevy_app::{App, Plugin, PostUpdate};
use bevy_ecs::prelude::*;
use core::hash::Hasher;
#[cfg(all(feature = "client", feature = "replication"))]
use lightyear_connection::client::Client;
#[cfg(feature = "server")]
use lightyear_connection::client::Connected;
use lightyear_connection::direction::NetworkDirection;
#[cfg(feature = "client")]
use lightyear_connection::network_topology::{NetworkTopology, NetworkingMetadata};
#[cfg(feature = "server")]
use lightyear_connection::server::Started;
#[cfg(any(feature = "p2p", feature = "server"))]
use lightyear_core::id::RemoteId;
#[cfg(feature = "server")]
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::tick::Tick;
#[cfg(feature = "client")]
use lightyear_inputs::{InputChannel, client::InputSystems};
#[cfg(feature = "server")]
use lightyear_link::server::{LinkOf, Server};
#[cfg(feature = "client")]
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::AppMessageExt;
#[cfg(feature = "client")]
use lightyear_messages::prelude::MessageSender;
#[cfg(any(feature = "p2p", feature = "server"))]
use lightyear_messages::receive::MessageReceiver;
#[cfg(feature = "client")]
use lightyear_prediction::manager::{LastConfirmedInput, StateRollbackMetadata};
#[cfg(feature = "client")]
use lightyear_sync::prelude::SyncedLocalTimeline;
use serde::{Deserialize, Serialize};
#[cfg(any(feature = "p2p", feature = "server"))]
use tracing::error;
use tracing::{debug, trace};

/// History of the checksums on the server to validate client checksums against.
#[derive(Component, Debug, Default)]
pub struct ChecksumHistory {
    history: BTreeMap<Tick, u64>,
}

const CHECKSUM_HISTORY_TICKS: u32 = 30;

/// Local and remote P2P checksums waiting for their matching sample.
///
/// Peers do not necessarily advance [`LastConfirmedInput`] at the same time. A remote checksum can
/// arrive before this peer has computed that tick, while a slower peer can report a tick after this
/// peer has already computed and sent it. We retain both sides briefly so they can be compared once
/// the same tick is available locally and remotely. An unchanged local or remote value is a no-op,
/// so each checksum is compared only when it is first added or changes.
#[cfg(feature = "p2p")]
#[derive(Resource, Debug, Default)]
struct PendingP2PChecksums {
    by_tick: BTreeMap<Tick, PendingP2PChecksum>,
}

#[cfg(feature = "p2p")]
#[derive(Debug, Default)]
struct PendingP2PChecksum {
    /// The checksum this peer computed and sent for this tick.
    local: Option<u64>,
    /// The latest checksum received from each remote peer for this tick.
    remote: Vec<(lightyear_core::id::PeerId, u64)>,
}

#[cfg(feature = "p2p")]
impl PendingP2PChecksums {
    /// Store a new local value and immediately compare any remote values already waiting for it.
    fn record_local(
        &mut self,
        tick: Tick,
        checksum: u64,
        mut compare: impl FnMut(lightyear_core::id::PeerId, Tick, u64, u64),
    ) {
        let pending = self.by_tick.entry(tick).or_default();
        if pending.local == Some(checksum) {
            return;
        }
        pending.local = Some(checksum);
        pending.remote.iter().for_each(|&(peer, remote)| {
            compare(peer, tick, checksum, remote);
        });
    }

    /// Store a new remote value and immediately compare it if the local value is already present.
    fn record_remote(
        &mut self,
        peer: lightyear_core::id::PeerId,
        message: ChecksumMessage,
        mut compare: impl FnMut(lightyear_core::id::PeerId, Tick, u64, u64),
    ) {
        let pending = self.by_tick.entry(message.tick).or_default();
        if let Some((_, checksum)) = pending
            .remote
            .iter_mut()
            .find(|(pending_peer, _)| *pending_peer == peer)
        {
            if *checksum == message.checksum {
                return;
            }
            *checksum = message.checksum;
        } else {
            pending.remote.push((peer, message.checksum));
        }
        if let Some(local) = pending.local {
            compare(peer, message.tick, local, message.checksum);
        }
    }

    fn clean(&mut self, current_tick: Tick) {
        let oldest_tick = current_tick - CHECKSUM_HISTORY_TICKS;
        self.by_tick.retain(|tick, _| *tick >= oldest_tick);
    }
}

/// Shared checksum protocol plugin.
///
/// Add this from the shared protocol after `ClientPlugins` / `ServerPlugins`
/// and before spawning networking entities. It installs the client send and/or
/// server receive checksum plugins based on which Lightyear side plugins are
/// present. In P2P mode it installs both plugins because every peer sends and receives.
pub struct ChecksumPlugin;

impl Plugin for ChecksumPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<DeterministicReplicationPlugin>() {
            app.add_plugins(DeterministicReplicationPlugin);
        }

        #[cfg(feature = "client")]
        if app.is_plugin_added::<lightyear_sync::client::ClientPlugin>()
            && !app.is_plugin_added::<ChecksumSendPlugin>()
        {
            app.add_plugins(ChecksumSendPlugin);
        }

        #[cfg(feature = "p2p")]
        if app.is_plugin_added::<lightyear_sync::client::ClientPlugin>()
            && !app.is_plugin_added::<ChecksumReceivePlugin>()
        {
            app.add_plugins(ChecksumReceivePlugin);
        }

        #[cfg(feature = "server")]
        if app.is_plugin_added::<lightyear_sync::server::ServerPlugin>()
            && !app.is_plugin_added::<ChecksumReceivePlugin>()
        {
            app.add_plugins(ChecksumReceivePlugin);
        }
    }
}

fn register_checksum_message(app: &mut App) {
    // Sending reuses InputChannel, which is already bidirectional and unordered-unreliable.
    let direction = if cfg!(feature = "p2p") {
        NetworkDirection::Bidirectional
    } else {
        NetworkDirection::ClientToServer
    };
    if !app.is_message_registered::<ChecksumMessage>() {
        app.register_message::<ChecksumMessage>()
            .add_direction(direction);
    }
}

/// Plugin that computes and sends checksums for deterministic entities with hashable components.
///
/// Conventional clients send to their server. Direct P2P peers broadcast to every connected peer.
#[cfg(feature = "client")]
pub struct ChecksumSendPlugin;

#[cfg(feature = "client")]
impl ChecksumSendPlugin {
    /// Compute and send the checksum at the complete confirmed-input frontier.
    ///
    /// This runs in `PostUpdate`, after both this frame's rollback replay and
    /// [`InputSystems::UpdateRemoteInputTicks`], so the current [`LastConfirmedInput`] can be read
    /// directly. In P2P mode the computed checksum is also retained for remote samples of the same
    /// tick that can arrive later.
    fn compute_and_send_checksum(
        mut world: ChecksumWorld<'_, '_, true>,
        local_timeline: SyncedLocalTimeline,
        metadata: Res<NetworkingMetadata>,
        mut senders: Query<&mut MessageSender<ChecksumMessage>>,
        last_confirmed_input: Res<LastConfirmedInput>,
        #[cfg(feature = "p2p")] mut pending_checksums: ResMut<PendingP2PChecksums>,
        #[cfg(feature = "replication")] catchup_managers: Query<&CatchUpManager, With<Client>>,
        state_metadata: Res<StateRollbackMetadata>,
    ) {
        let current_tick = local_timeline.tick();
        if !last_confirmed_input.received_for_all_clients {
            return;
        }
        let Some(confirmed_tick) = last_confirmed_input.get() else {
            return;
        };
        // only compute the checksum when we have received remote inputs
        if confirmed_tick > current_tick {
            return;
        }
        let conventional_link = match metadata.mode {
            NetworkTopology::Client(link) | NetworkTopology::HostClient { client: link, .. } => {
                Some(link)
            }
            NetworkTopology::P2P(_) => {
                #[cfg(feature = "p2p")]
                {
                    None
                }
                #[cfg(not(feature = "p2p"))]
                {
                    return;
                }
            }
            NetworkTopology::Undefined
            | NetworkTopology::Server(_)
            | NetworkTopology::Invalid(_) => return,
        };
        #[cfg(feature = "replication")]
        // Skip while catch-up is running. The client is intentionally hashing
        // pre-catch-up state until the bundled snapshot has been replayed.
        if conventional_link
            .and_then(|link| catchup_managers.get(link).ok())
            .is_some_and(CatchUpManager::suppresses_checksums)
        {
            return;
        }
        // Skip if a one-shot forced rollback is scheduled but not yet
        // consumed. Until the rollback has replayed from the bundled
        // snapshot tick, the client is intentionally hashing pre-catch-up
        // state that the server should not compare against.
        if state_metadata.forced_rollback_tick().is_some() {
            return;
        }

        world.update_archetypes();

        if let Some(link) = conventional_link {
            let checksum = compute_history_checksum(&mut world, confirmed_tick);
            debug!(
                ?current_tick,
                "Computed checksum for LastConfirmedInput tick {:?}: {:016x}",
                confirmed_tick,
                checksum
            );
            if let Ok(mut sender) = senders.get_mut(link) {
                sender.send::<InputChannel>(ChecksumMessage {
                    tick: confirmed_tick,
                    checksum,
                });
            }
            return;
        }

        #[cfg(feature = "p2p")]
        {
            let NetworkTopology::P2P(joined) = &metadata.mode else {
                return;
            };
            let checksum = compute_history_checksum(&mut world, confirmed_tick);
            pending_checksums.record_local(confirmed_tick, checksum, log_p2p_comparison);
            pending_checksums.clean(confirmed_tick);
            Self::send_p2p_checksum(&mut senders, joined, confirmed_tick, checksum);
        }
    }

    #[cfg(feature = "p2p")]
    fn send_p2p_checksum(
        senders: &mut Query<&mut MessageSender<ChecksumMessage>>,
        links: &[Entity],
        tick: Tick,
        checksum: u64,
    ) {
        debug!(
            "Computed P2P checksum for tick {:?}: {:016x}",
            tick, checksum
        );
        for &link in links {
            if let Ok(mut sender) = senders.get_mut(link) {
                sender.send::<InputChannel>(ChecksumMessage { tick, checksum });
            }
        }
    }
}

#[cfg(feature = "client")]
fn compute_history_checksum(world: &mut ChecksumWorld<'_, '_, true>, tick: Tick) -> u64 {
    let mut checksum = 0u64;
    // SAFETY: world.update_archetypes() has been called
    unsafe { world.iter_archetypes() }.for_each(|(archetype, checksum_archetype)| {
        // TODO: guarantee stable entity iteration order across peers.
        archetype.entities().iter().for_each(|entity| {
            checksum_archetype
                .components
                .iter()
                .for_each(|(component_id, storage_type)| {
                    trace!(
                        "Adding component {:?} from entity {:?} to checksum for tick {:?}",
                        component_id,
                        entity.id(),
                        tick
                    );
                    // SAFETY: the way we constructed the archetypes guarantees that the component exists on the entity and we have unique write access
                    let history_ptr = unsafe {
                        lightyear_utils::ecs::get_component_unchecked_mut(
                            world.world,
                            entity,
                            archetype.table_id(),
                            *storage_type,
                            *component_id,
                        )
                    };
                    let (hash_fn, pop_until_tick_and_hash_fn) =
                        world.state.hash_fns.get(component_id).expect(
                            "Component in checksum archetype must have a hash function registered",
                        );

                    let mut hasher = seahash::SeaHasher::default();
                    if pop_until_tick_and_hash_fn.unwrap()(
                        history_ptr,
                        tick,
                        &mut hasher,
                        hash_fn.inner,
                    ) {
                        // XOR the hashes together to get an order-independent checksum
                        checksum ^= hasher.finish();
                    }
                });
        });
    });
    checksum
}

#[derive(Serialize, Deserialize)]
pub struct ChecksumMessage {
    pub tick: Tick,
    pub checksum: u64,
}

#[cfg(feature = "client")]
impl Plugin for ChecksumSendPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<DeterministicReplicationPlugin>() {
            app.add_plugins(DeterministicReplicationPlugin);
        }

        // We need the application-wide remote-input frontier to compute checksums.
        app.init_resource::<LastConfirmedInput>();
        #[cfg(feature = "p2p")]
        app.init_resource::<PendingP2PChecksums>();

        register_checksum_message(app);
    }

    fn finish(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            ChecksumSendPlugin::compute_and_send_checksum
                // the LastConfirmedInput must be updated before we compute the checksum
                .after(InputSystems::UpdateRemoteInputTicks)
                .before(MessageSystems::Send),
        );
    }
}

/// Plugin that receives and validates checksums.
///
/// An authoritative server compares clients against its simulation. In P2P mode each peer compares
/// remote samples against its local confirmed prediction history.
#[cfg(any(feature = "p2p", feature = "server"))]
pub struct ChecksumReceivePlugin;

#[cfg(feature = "p2p")]
impl ChecksumReceivePlugin {
    /// Drain checksum samples from every joined P2P Link that currently has a receiver.
    fn receive_p2p_checksum_messages(
        metadata: Res<NetworkingMetadata>,
        mut messages: Query<(&mut MessageReceiver<ChecksumMessage>, &RemoteId)>,
        mut pending_checksums: ResMut<PendingP2PChecksums>,
    ) {
        let NetworkTopology::P2P(joined) = &metadata.mode else {
            return;
        };
        for &link in joined {
            let Ok((mut receiver, remote_id)) = messages.get_mut(link) else {
                continue;
            };
            for message in receiver.receive() {
                pending_checksums.record_remote(remote_id.0, message, log_p2p_comparison);
            }
        }
    }
}

#[cfg(feature = "p2p")]
fn log_p2p_comparison(peer: lightyear_core::id::PeerId, tick: Tick, expected: u64, actual: u64) {
    if expected == actual {
        debug!(
            ?peer,
            ?tick,
            checksum = format_args!("{actual:016x}"),
            "P2P checksum match"
        );
    } else {
        error!(
            ?peer,
            ?tick,
            expected = format_args!("{expected:016x}"),
            actual = format_args!("{actual:016x}"),
            "P2P checksum mismatch"
        );
    }
}

#[cfg(feature = "server")]
impl ChecksumReceivePlugin {
    /// Compute a checksum over all deterministic entities' hashable
    /// components at the current server tick and store it for later
    /// comparison against incoming client checksums.
    fn compute_and_store_checksum(
        mut world: ChecksumWorld<'_, '_, false>,
        timeline: Res<LocalTimeline>,
        server: Single<&mut ChecksumHistory, With<Started>>,
    ) {
        let mut checksum = 0u64;
        let tick = timeline.tick();
        let mut history = server.into_inner();

        world.update_archetypes();
        // SAFETY: world.update_archetypes() has been called
        unsafe { world.iter_archetypes() }.for_each(|(archetype, checksum_archetype)| {
            // TODO: guarantee stable entity iteration order across peers.
            archetype.entities().iter().for_each(|entity| {
                checksum_archetype
                    .components
                    .iter()
                    .for_each(|(component_id, storage_type)| {
                        trace!(
                            "Adding component {:?} from entity {:?} to checksum for tick {:?}",
                            component_id,
                            entity.id(),
                            tick
                        );
                        // SAFETY: the way we constructed the archetypes guarantees that the component exists on the entity and we have unique write access
                        let component_ptr = unsafe {
                            lightyear_utils::ecs::get_component_unchecked(
                                world.world,
                                entity,
                                archetype.table_id(),
                                *storage_type,
                                *component_id,
                            )
                        };
                        let (hash_fn, _) = world.state.hash_fns.get(component_id).expect(
                            "Component in checksum archetype must have a hash function registered",
                        );
                        let mut hasher = seahash::SeaHasher::default();
                        hash_fn.hash_component(component_ptr, &mut hasher);
                        let hash = hasher.finish();
                        checksum ^= hash; // XOR the hashes together to get an order-independent checksum
                    });
            });
        });

        debug!("Computed checksum for tick {:?}: {:016x}", tick, checksum);

        history.history.insert(tick, checksum);
    }

    fn receive_checksum_message(
        mut messages: Query<
            (&mut MessageReceiver<ChecksumMessage>, &LinkOf, &RemoteId),
            With<Connected>,
        >,
        server: Query<&ChecksumHistory, (With<Server>, With<Started>)>,
    ) {
        messages.iter_mut().for_each(|(mut receiver, link_of, remote_id)| {
            if let Ok(history) = server.get(link_of.server) {
                receiver.receive().for_each(|message| {
                    let Some(&expected) = history.history.get(&message.tick) else {
                        return;
                    };
                    if expected == message.checksum {
                        debug!("Checksum match from client {:?} at tick {:?}: {:016x}", remote_id, message.tick, message.checksum);
                    } else if message.checksum != 0 {
                        error!("Checksum mismatch from client {:?} at tick {:?}: expected {:016x}, got {:016x}", remote_id, message.tick, expected, message.checksum);
                    }
                })
            }
        })
    }

    fn clean_history(
        timeline: Res<LocalTimeline>,
        history: Single<&mut ChecksumHistory, (With<Server>, With<Started>)>,
    ) {
        let tick = timeline.tick();
        let mut history = history.into_inner();
        history
            .history
            .retain(|t, _| *t >= tick - CHECKSUM_HISTORY_TICKS);
    }
}

#[cfg(any(feature = "p2p", feature = "server"))]
impl Plugin for ChecksumReceivePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<DeterministicReplicationPlugin>() {
            app.add_plugins(DeterministicReplicationPlugin);
        }

        #[cfg(feature = "p2p")]
        if app.is_plugin_added::<lightyear_sync::client::ClientPlugin>() {
            app.init_resource::<LastConfirmedInput>();
            app.init_resource::<PendingP2PChecksums>();
        }

        #[cfg(feature = "server")]
        // the server will check the checksum validity
        if app.is_plugin_added::<lightyear_sync::server::ServerPlugin>() {
            app.register_required_components::<Server, ChecksumHistory>();
        }

        register_checksum_message(app);
    }

    fn finish(&self, app: &mut App) {
        #[cfg(feature = "p2p")]
        if app.is_plugin_added::<lightyear_sync::client::ClientPlugin>() {
            app.add_systems(
                PostUpdate,
                ChecksumReceivePlugin::receive_p2p_checksum_messages.before(MessageSystems::Send),
            );
        }

        #[cfg(feature = "server")]
        if app.is_plugin_added::<lightyear_sync::server::ServerPlugin>() {
            app.add_systems(
                PostUpdate,
                (
                    ChecksumReceivePlugin::clean_history,
                    ChecksumReceivePlugin::receive_checksum_message,
                ),
            );
            app.add_systems(FixedLast, ChecksumReceivePlugin::compute_and_store_checksum);
        }
    }
}

#[cfg(all(test, feature = "p2p"))]
mod tests {
    use super::*;
    use lightyear_core::id::PeerId;

    fn peer(id: u64) -> PeerId {
        PeerId::Entity(id)
    }

    #[test]
    fn remote_checksum_waits_for_the_local_checksum() {
        let mut pending = PendingP2PChecksums::default();
        let mut comparisons = Vec::new();
        pending.record_remote(
            peer(1),
            ChecksumMessage {
                tick: Tick(12),
                checksum: 0xCAFE,
            },
            |peer, tick, local, remote| {
                comparisons.push((peer, tick, local, remote));
            },
        );
        assert!(comparisons.is_empty());

        pending.record_local(Tick(12), 0xCAFE, |peer, tick, local, remote| {
            comparisons.push((peer, tick, local, remote));
        });
        assert_eq!(comparisons, [(peer(1), Tick(12), 0xCAFE, 0xCAFE)]);

        // Re-recording the same local value on the next frame does not compare it again.
        pending.record_local(Tick(12), 0xCAFE, |peer, tick, local, remote| {
            comparisons.push((peer, tick, local, remote));
        });
        assert_eq!(comparisons.len(), 1);
    }

    #[test]
    fn local_checksum_waits_for_each_remote_peer() {
        let mut pending = PendingP2PChecksums::default();
        let mut comparisons = Vec::new();
        pending.record_local(Tick(20), 7, |peer, tick, local, remote| {
            comparisons.push((peer, tick, local, remote));
        });
        assert!(comparisons.is_empty());

        for peer in [peer(1), peer(2), peer(3)] {
            pending.record_remote(
                peer,
                ChecksumMessage {
                    tick: Tick(20),
                    checksum: 7,
                },
                |peer, tick, local, remote| {
                    comparisons.push((peer, tick, local, remote));
                },
            );
        }
        assert_eq!(comparisons.len(), 3);

        // A repeated value does not compare again.
        pending.record_remote(
            peer(2),
            ChecksumMessage {
                tick: Tick(20),
                checksum: 7,
            },
            |peer, tick, local, remote| {
                comparisons.push((peer, tick, local, remote));
            },
        );
        assert_eq!(comparisons.len(), 3);
        assert!(comparisons.contains(&(peer(1), Tick(20), 7, 7)));
        assert!(comparisons.contains(&(peer(2), Tick(20), 7, 7)));
        assert!(comparisons.contains(&(peer(3), Tick(20), 7, 7)));

        // A changed value is a new checksum and compares once.
        pending.record_remote(
            peer(2),
            ChecksumMessage {
                tick: Tick(20),
                checksum: 8,
            },
            |peer, tick, local, remote| {
                comparisons.push((peer, tick, local, remote));
            },
        );
        assert_eq!(comparisons.len(), 4);
        assert_eq!(comparisons[3], (peer(2), Tick(20), 7, 8));

        pending.clean(Tick(51));
        assert!(pending.by_tick.is_empty());
    }
}
