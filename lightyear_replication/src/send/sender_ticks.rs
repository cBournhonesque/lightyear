use alloc::vec::Vec;
use core::time::Duration;

use crate::registry::component_mask::ComponentMask;
use bevy_ecs::{component::Tick as BevyTick, entity::hash_map::EntityHashMap, prelude::*};
use bevy_platform::collections::HashMap;
use lightyear_core::tick::Tick;
use lightyear_transport::packet::message::MessageId;
#[allow(unused_imports)]
use tracing::{debug, trace};

/// Tracks replication ticks for a `ReplicationSender`.
#[derive(Component, Default, Debug)]
pub struct SenderTicks {
    /// Last acknowledged tick for each visible entity with its components.
    ///
    /// Used to track what the peer has already received.
    pub(crate) entities: EntityHashMap<EntityTicks>,

    /// The last tick in which we sent an Actions
    ///
    /// It should be included in actions messages and server events to avoid needless waiting for the next actions
    /// message to arrive.
    pub(crate) action_tick: Tick,

    /// Sent update message indices mapped to their info.
    updates: HashMap<MessageId, UpdateInfo>,
}

impl SenderTicks {
    /// Registers update message to later acknowledge updated entities.
    pub(crate) fn register_update_message(&mut self, index: MessageId, info: UpdateInfo) {
        self.updates.insert(index, info);
    }

    /// Marks mutate message as acknowledged by its index.
    ///
    /// Returns associated entities and their component IDs.
    ///
    /// Updates the tick and components of all entities from this mutation message if the tick is higher.
    pub(crate) fn ack_mutate_message(
        &mut self,
        client: Entity,
        mutate_index: MessageId,
    ) -> Option<Vec<(Entity, ComponentMask)>> {
        let Some(mutate_info) = self.updates.remove(&mutate_index) else {
            debug!("received unknown `{mutate_index:?}` from client `{client}`");
            return None;
        };

        for (entity, components) in &mutate_info.entities {
            let Some(entity_ticks) = self.entities.get_mut(entity) else {
                // We ignore missing entities, since they were probably despawned.
                continue;
            };

            // Received tick could be outdated because we bump it
            // if we detect any insertion on the entity in `collect_changes`.
            if entity_ticks.server_tick < mutate_info.server_tick {
                entity_ticks.server_tick = mutate_info.server_tick;
                entity_ticks.system_tick = mutate_info.system_tick;
                entity_ticks.components |= components;
            }
        }
        trace!(
            "acknowledged mutate message with `{:?}` from client `{client}`",
            mutate_info.server_tick,
        );

        Some(mutate_info.entities)
    }

    /// Removes all mutate messages older then `min_timestamp`.
    ///
    /// Calls given function for each removed message.
    pub(crate) fn cleanup_older_mutations(
        &mut self,
        min_timestamp: Duration,
        mut f: impl FnMut(&mut UpdateInfo),
    ) {
        self.updates.retain(|_, mutate_info| {
            if mutate_info.timestamp < min_timestamp {
                (f)(mutate_info);
                false
            } else {
                true
            }
        });
    }
}

/// Acknowledgment information about an entity.
#[derive(Debug)]
pub(crate) struct EntityTicks {
    /// The last server tick for which data for this entity was sent.
    ///
    /// This tick serves as the reference point for determining whether components
    /// on the entity have changed and need to be replicated. Component changes
    /// older than this update tick are assumed to have been acknowledged by the client.
    pub(crate) server_tick: Tick,

    // TODO: add send bevy_tick vs ack bevy_tick
    /// The corresponding tick for change detection.
    pub(crate) system_tick: BevyTick,

    /// The list of components that were replicated, as of this tick
    pub(crate) components: ComponentMask,
}

/// Information about an Updates message that was sent
#[derive(Debug)]
pub(crate) struct UpdateInfo {
    // timeline tick when the message was sent
    pub(crate) server_tick: Tick,
    // system tick when the message was sent
    pub(crate) system_tick: BevyTick,
    pub(crate) entities: Vec<(Entity, ComponentMask)>,
}
