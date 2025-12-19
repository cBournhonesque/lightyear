//! Utilities for computing checksums for data integrity verification.
//!
//! The clients will send checksums at regular intervals to the server, which will verify them against its own computed checksums.
//!
//! Note: we don't have a good way to guarantee that we are iterating through entities in a stable order on both client and server.
//! Because of this, we will compute an order-independent checksum by only hashing component data and then XOR-ing the results together.

use crate::archetypes::ChecksumWorld;
use crate::plugin::DeterministicReplicationPlugin;
use alloc::collections::BTreeMap;
use bevy_app::{App, FixedLast, Plugin, PostUpdate};
use bevy_ecs::prelude::*;
use core::hash::Hasher;
use lightyear_connection::client::{Client, Connected};
use lightyear_connection::direction::NetworkDirection;
use lightyear_connection::server::Started;
use lightyear_core::id::RemoteId;
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::tick::Tick;
use lightyear_inputs::InputChannel;
use lightyear_inputs::client::InputSystems;
use lightyear_link::server::{LinkOf, Server};
use lightyear_messages::MessageManager;
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::{AppMessageExt, MessageSender};
use lightyear_messages::receive::MessageReceiver;
use lightyear_prediction::manager::LastConfirmedInput;
use lightyear_sync::prelude::{InputTimeline, IsSynced};
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use tracing::{debug, error, info, trace};

/// History of the checksums on the server to validate client checksums against.
#[derive(Component, Debug, Default)]
pub struct ChecksumHistory {
    history: BTreeMap<Tick, u64>,
}

/// Plugin that can be added to clients to compute and send checksums for all deterministic entities with hashable components.
///
/// The server will receive these checksums and verify them against its own computed checksums.
/// If a checksum does not match, it indicates a desync between the client and server.
pub struct ChecksumSendPlugin;

impl ChecksumSendPlugin {
    /// Compute a checksum for all deterministic entities with hashable components.
    fn compute_and_send_checksum(
        mut world: ChecksumWorld<'_, '_, true>,
        local_timeline: Res<LocalTimeline>,
        client: Single<
            (
                &LastConfirmedInput,
                &MessageManager,
                &mut MessageSender<ChecksumMessage>,
            ),
            (With<Client>, With<IsSynced<InputTimeline>>),
        >,
    ) {
        let mut checksum = 0u64;
        let current_tick = local_timeline.tick();
        let (last_confirmed_input, message_manager, mut sender) = client.into_inner();
        let tick = last_confirmed_input.tick.get();
        // only compute the checksum when we have received remote inputs
        if tick > current_tick {
            return;
        }

        world.update_archetypes();
        // SAFETY: world.update_archetypes() has been called
        unsafe { world.iter_archetypes() }.for_each(|(archetype, checksum_archetype)| {
            // TODO: how can we guarantee that the order is the same on client and server? We need a stable order for entities, otherwise the checksum will differ even if the data is the same.
            archetype.entities().iter().for_each(|entity| {
                // // TODO: currently this only works if the entity was replicated from the server
                // //  if the entity was created locally on the client, it won't have a mapping to a remote entity and will be ignored
                // // convert to a remote entity
                // if let Some(mapped_entity) = message_manager.entity_mapper.get_remote(entity.id()) {
                //     trace!("Adding entity {:?} (mapped to remote entity {:?}) to checksum for tick {:?}", entity.id(), mapped_entity, tick);
                //     // hasher.write_u64(mapped_entity.to_bits());
                // }
                // TODO: in deterministic lockstep mode, we need to fetch directly from the component! There is no
                //  prediction-history and the LastConfirmedTick is always the current tick.
                checksum_archetype.components.iter().for_each(|(component_id, storage_type)| {
                    trace!("Adding component {:?} from entity {:?} to checksum for tick {:?}",
                        component_id, entity.id(), tick);
                    // SAFETY: the way we constructed the archetypes guarantees that the component exists on the entity and we have unique write access
                    let history_ptr = unsafe {
                        lightyear_utils::ecs::get_component_unchecked_mut(world.world, entity, archetype.table_id(), *storage_type, *component_id)
                    };
                    let (hash_fn, pop_until_tick_and_hash_fn) = world.state.hash_fns.get(component_id).expect("Component in checksum archetype must have a hash function registered");

                    let mut hasher = seahash::SeaHasher::default();
                    pop_until_tick_and_hash_fn.unwrap()(history_ptr, tick, &mut hasher, hash_fn.inner);
                    let hash = hasher.finish();
                    checksum ^= hash; // XOR the hashes together to get an order-independent checksum
                });
            });
        });
        debug!(
            ?current_tick,
            "Computed checksum for LastConfirmedInput tick {:?}: {:016x}", tick, checksum
        );

        sender.send::<InputChannel>(ChecksumMessage { tick, checksum });
    }
}

#[derive(Serialize, Deserialize)]
pub struct ChecksumMessage {
    pub tick: Tick,
    pub checksum: u64,
}

impl Plugin for ChecksumSendPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<DeterministicReplicationPlugin>() {
            app.add_plugins(DeterministicReplicationPlugin);
        }

        // we need the LastConfirmedInput to compute the checksums
        app.register_required_components::<InputTimeline, LastConfirmedInput>();

        app.register_message::<ChecksumMessage>()
            .add_direction(NetworkDirection::ClientToServer);
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

/// Plugin that can be added to the server to receive and validate checksums sent by clients.
///
/// The server needs to also run the simulation to be able to compute its own checksums for comparison.
pub struct ChecksumReceivePlugin;

impl ChecksumReceivePlugin {
    /// Compute a checksum for all deterministic entities with hashable components.
    fn compute_and_store_checksum(
        mut world: ChecksumWorld<'_, '_, false>,
        timeline: Res<LocalTimeline>,
        server: Single<&mut ChecksumHistory, With<Started>>,
    ) {
        let mut checksum = 0u64;
        let tick = timeline.tick();
        let mut history = server.into_inner();

        // SAFETY: world.update_archetypes() has been called
        world.update_archetypes();
        unsafe { world.iter_archetypes() }.for_each(|(archetype, checksum_archetype)| {
            // TODO: how can we ensure that we are iterating entities in a stable order on both client and server?
            archetype.entities().iter().for_each(|entity| {
                // TODO: we don't write entities in the checksum because if there are some non-replicated entities, the entity ids will differ between client and server.
                //  We should maybe wait for the ability to reserve ranges of entities so that entity ids match perfectly.
                // hasher.write_u64(entity.id().to_bits());
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
                    let expected = history.history.get(&message.tick);
                    if let Some(&expected) = expected {
                        if expected != message.checksum {
                            if message.checksum != 0 {
                                error!("Checksum mismatch from client {:?} at tick {:?}: expected {:016x}, got {:016x}", remote_id, message.tick, expected, message.checksum);
                            }
                        } else {
                            debug!("Checksum match from client {:?} at tick {:?}: {:016x}", remote_id, message.tick, message.checksum);
                        }
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
        // keep only the last 30 ticks of history
        history.history.retain(|t, _| *t >= tick - 30);
    }
}

impl Plugin for ChecksumReceivePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<DeterministicReplicationPlugin>() {
            app.add_plugins(DeterministicReplicationPlugin);
        }

        // the server will check the checksum validity
        app.register_required_components::<Server, ChecksumHistory>();

        if !app.is_message_registered::<ChecksumMessage>() {
            app.register_message::<ChecksumMessage>()
                .add_direction(NetworkDirection::ClientToServer);
        }

        app.add_systems(
            PostUpdate,
            (
                ChecksumReceivePlugin::clean_history,
                ChecksumReceivePlugin::receive_checksum_message,
            ),
        );
    }

    fn finish(&self, app: &mut App) {
        app.add_systems(FixedLast, ChecksumReceivePlugin::compute_and_store_checksum);
    }
}
