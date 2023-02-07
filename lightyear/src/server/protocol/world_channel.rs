use std::{
    collections::{HashMap, HashSet, VecDeque},
    hash::Hash,
    net::SocketAddr,
};
use std::fmt::Debug;
use bevy_ecs::component::ComponentId;
use bevy_ecs::entity::Entity;

use naia_socket_shared::Instant;

use crate::shared::{ChannelSender, EntityAction, EntityActionReceiver, KeyGenerator,
                    NetEntity, ReliableSender};

use crate::server::protocol::{
        entity_action_event::EntityActionEvent, entity_manager::ActionId,
        entity_message_waitlist::EntityMessageWaitlist,
};


const RESEND_ACTION_RTT_FACTOR: f32 = 1.5;

// ComponentChannel

#[derive(Debug)]
pub enum ComponentChannel {
    /// Component that needs to be inserted as soon as the clients acks the entity was spawned, or that the component was removed.
    ToBeInserted,
    /// Component in the process of being inserted (waiting for an ack from client that component was inserted)
    Inserting,
    /// Component that was acked by client as inserted
    Inserted,
    /// Component hasn't been acked by the client as Inserted (it is still Inserting), but we already want to remove it on the server.
    ToBeRemoved,
    /// We are sending a message to the client to remove the entity, and are waiting for an ack back
    Removing,
}

// EntityChannel

#[derive(Debug)]
pub enum EntityChannel {
    /// Map of component type to the channel state of the components
    Spawning(CheckedMap<ComponentId, ComponentChannel>),
    Spawned(CheckedMap<ComponentId, ComponentChannel>),
    /// Entity hasn't been acked by the client as Spawned (it is still Spawning), but we already want to despawn it on the server.
    ToBeDespawned,
    /// We are sending a message to the client to despawn the entity,
    Despawning,
}


// WorldChannel

/// Channel to perform ECS replication between server and client
/// Only handles entity actions (Spawn/despawn entity and insert/remove components)
/// Will use a reliable sender.
/// Will wait for acks from the client to know the state of the client's ECS world ("remote")
pub struct WorldChannel {
    /// Channels to keep track of the spawning/despawning state of each entity/component
    entity_channels: CheckedMap<Entity, EntityChannel>,
    outgoing_actions: ReliableSender<EntityActionEvent>,
    delivered_actions: EntityActionReceiver,

    address: SocketAddr,
    net_entity_generator: KeyGenerator<NetEntity>,
    entity_to_net_entity_map: HashMap<Entity, NetEntity>,
    net_entity_to_entity_map: HashMap<NetEntity, Entity>,
    pub delayed_entity_messages: EntityMessageWaitlist,
}

impl WorldChannel {
    pub fn new(
        address: SocketAddr,
    ) -> Self {
        Self {
            entity_channels: CheckedMap::new(),
            outgoing_actions: ReliableSender::new(RESEND_ACTION_RTT_FACTOR),
            delivered_actions: EntityActionReceiver::default(),

            address,
            net_entity_generator: KeyGenerator::default(),
            net_entity_to_entity_map: HashMap::new(),
            entity_to_net_entity_map: HashMap::new(),
            delayed_entity_messages: EntityMessageWaitlist::default(),
        }
    }

    // Main

    pub fn entity_channel_is_open(&self, entity: &Entity) -> bool {
        matches!(
            self.entity_channels.get(entity),
            Some(EntityChannel::Spawned(_))
        )
    }

    // Host Updates

    /// Prepare a spawn entity message to be written in the next packet
    pub fn buffer_spawn_entity_message(&mut self, entity: Entity) {
        if self.entity_channels.get(&entity).is_none() {
            // spawn entity
            self.entity_channels.insert(entity, EntityChannel::Spawning(CheckedMap::default()));
            // buffer a spawn entity message
            // (we'll convert to the net entity later)
            self.outgoing_actions.send_message(EntityActionEvent::SpawnEntity(entity));
            // generate a net entity
            self.on_entity_channel_opening(entity);
        }
    }

    /// Prepare a despawn entity message to be written in the next packet.
    ///
    /// Only gets buffered if we received an ack from the client that the entity was actually spawned.
    pub fn buffer_despawn_entity_message(&mut self, entity: Entity) {
        let mut despawn = false;
        let mut removing_components = Vec::new();

        // check if the entity was actually spawned on the client
        if let Some(EntityChannel::Spawned(component_channels)) = self.entity_channels.get(&entity) {
            despawn = true;

            for (component, component_channel) in component_channels.iter() {
                if let ComponentChannel::Inserted = component_channel {
                    removing_components.push(*component);
                }
            }
        }
        // TODO: also drop all components waiting to be inserted

        // in any case, we don't want the entity channel to be 'Spawning' anymore
        self.entity_channels.remove(&entity);

        if despawn {
            self.entity_channels.insert(*entity, EntityChannel::Despawning);

            // if client_despawn
            if true {
                self.outgoing_actions
                    .send_message(EntityActionEvent::DespawnEntity(*entity));
            }
            // if we were sending messages related to that entity, drop them
            self.on_entity_channel_closing(&entity);

            for component in removing_components {
                self.on_component_channel_closing(&entity, &component);
            }
        } else {
            // if the entity was not even acked as spawned by the client yet, set it as ToBeDespawned
            // TODO: should check that the entity is Spawning though...
            self.entity_channels.insert(*entity, EntityChannel::ToBeDespawned);
        }
    }

    /// Prepare a insert component message to be written in the next packet
    ///
    /// Only gets buffered if we received an ack from the client that the entity was actually spawned.
    pub fn buffer_insert_component_message(&mut self, entity: &Entity, component: &ComponentId) {
        match self.entity_channels.get_mut(entity) {
            None => {panic!("Should not be possible because we buffer SpawnEntity messages before InsertComponent")},
            Some(EntityChannel::Spawning(component_channels)) => {
                component_channels.insert(*component, ComponentChannel::ToBeInserted);
            }
            Some(EntityChannel::Despawning) | Some(EntityChannel::ToBeDespawned)  => {
                // TODO: remove from the list of components to be inserted?

            }
            Some(EntityChannel::Spawned(component_channels)) => {
                if component_channels.get(component).is_none() {
                    // insert component
                    component_channels.insert(*component, ComponentChannel::Inserting);
                    self.outgoing_actions
                        .send_message(EntityActionEvent::InsertComponent(*entity, *component));
                }
            }
        }
    }

    /// Prepare a remove component message to be written in the next packet
    ///
    /// Only gets buffered if we received an ack from the client that the entity was actually spawned,
    /// and the component was actually inserted
    pub fn buffer_remove_component(&mut self, entity: &Entity, component: &ComponentId) {
        match self.entity_channels.get_mut(entity) {
            Some(EntityChannel::Spawned(component_channels)) => {
                if let Some(ComponentChannel::Inserted) = component_channels.get(component) {
                    component_channels.remove(component);

                    // remove component
                    component_channels.insert(*component, ComponentChannel::Removing);
                    self.outgoing_actions
                        .send_message(EntityActionEvent::RemoveComponent(*entity, *component));
                    self.on_component_channel_closing(entity, component);
                }
            }
            Some(EntityChannel::Spawning(component_channels)) => {
                component_channels.remove(component);
                component_channels.insert(*component, ComponentChannel::ToBeRemoved);
            }
            _ => {
            }

        }
    }

    // Remote Actions

    /// Ack that the client has received a given Spawn Entity message
    ///
    /// If the entity is to be despawned before the client has acked; we just start despawning it.
    // TODO: also don't even send the spawn message if the entity is to be despawned before we even sent the message?
    pub fn ack_spawn_entity(&mut self, entity: Entity, inserted_components: HashSet<ComponentId>) {
        match self.entity_channels.get(&entity) {
            Some(EntityChannel::Spawning(component_channels_to_be_inserted)) => {
                self.entity_channels.remove(&entity);

                // find the components that still need to be inserted
                let components_to_be_inserted: HashSet::<_> = component_channels_to_be_inserted.iter().filter_map(|(kind, channel)| {
                    if !matches!(channel, ComponentChannel::ToBeInserted) {
                        panic!("all components should be ToBeInserted");
                    }
                    Some(kind)
                }).collect();
                let still_needs_to_be_inserted: HashSet<&ComponentId> = components_to_be_inserted.difference(&inserted_components).collect();

                // for these, change channel to inserting status, and send insert message
                let mut component_channels = CheckedMap::new();
                for component in still_needs_to_be_inserted {
                    component_channels.insert(*component, ComponentChannel::Inserting);
                    self.outgoing_actions.send_message(EntityActionEvent::InsertComponent(entity, *component));
                }

                // for components that were already inserted, set to inserted
                for component in inserted_components {

                    // TODO: instead, we might want to call ack_inserted to check if the component has not been removed already!!!
                    // self.remote_insert_component(entity, *component);
                    component_channels.insert(*component, ComponentChannel::Inserted);
                }

                self.entity_channels.insert(entity, EntityChannel::Spawned(component_channels));

                // TODO: what is this?
                self.on_entity_channel_opened(&entity);
            }
            Some(EntityChannel::ToBeDespawned) => {
                // the entity was despawned on the server when the client received the spawn message
                // Despawn the entity on the client
                self.entity_channels.remove(&entity);
                self.entity_channels.insert(entity, EntityChannel::Despawning);
                self.outgoing_actions.send_message(EntityActionEvent::DespawnEntity(entity));
                self.on_entity_channel_closing(&entity);
            }
            _ => panic!("should only receive this event if entity channel is spawning or to be despawned"),
        }
    }

    /// The client ack-ed a Despawn Entity message. Do some cleanup.
    pub fn ack_despawn_entity(&mut self, entity: Entity) {
        if let Some(EntityChannel::Despawning) = self.entity_channels.get(&entity) {
            self.entity_channels.remove(&entity);
            // cleanup net entity
            self.on_entity_channel_closed(&entity);

            // TODO: how is this possible?
            // if entity is spawned in host, respawn entity channel
        } else {
            panic!("should only receive this event if entity channel is despawning");
        }
    }

    /// The client ack-ed a Insert Component message.
    pub fn ack_insert_component(&mut self, entity: Entity, component: ComponentId) {
        match self.entity_channels.get_mut(&entity) {
            Some(EntityChannel::Spawned(component_channels)) => {
                match component_channels.get(&component) {
                    Some(ComponentChannel::Inserting) => {
                        // if component exist in host, finalize channel state
                        component_channels.remove(&component);
                        component_channels.insert(component, ComponentChannel::Inserted);
                        self.on_component_channel_opened(&entity, &component);
                    },
                    Some(ComponentChannel::ToBeRemoved) => {
                        // while waiting for the ComponentInsertedAck, we already removed the component on the server
                        component_channels.remove(&component);
                        component_channels.insert(component, ComponentChannel::Removing);
                        self.outgoing_actions.send_message(EntityActionEvent::RemoveComponent(entity, component));
                        self.on_component_channel_closing(&entity, &component);
                    }
                    e => {panic!("ComponentChannel is in an invalid state: {:?}", e)}
                }
            },
            Some(EntityChannel::Despawning) => {}
            e => {panic!("EntityChannel is in an invalid state: {:?}", e)}
        }
    }

    /// The client ack-ed a RemoveComponent message
    pub fn ack_remove_component(&mut self, entity: Entity, component: ComponentId) {
        match self.entity_channels.get_mut(&entity) {
            Some(EntityChannel::Spawned(component_channels)) => {
                match component_channels.get(&component) {
                    Some(ComponentChannel::Removing) => {
                        component_channels.remove(&component);
                    },
                    Some(ComponentChannel::ToBeRemoved) => {
                        // insert component
                        component_channels.remove(&component);
                        component_channels.insert(component, ComponentChannel::Inserting);
                        self.outgoing_actions
                            .send_message(EntityActionEvent::InsertComponent(entity, component));
                    }
                    e => { panic!("ComponentChannel is in an invalid state: {:?}", e) }
                }
            },
            Some(EntityChannel::Despawning) => {}
            e => { panic!("EntityChannel is in an invalid state: {:?}", e) }
        }
    }

    // State Transition events

    /// Generate a new net_entity for the provided entity. NetEntity is a cheaper way to send
    /// the entity id across the network.
    fn on_entity_channel_opening(&mut self, entity: Entity) {
        // generate new net entity
        let new_net_entity = self.net_entity_generator.generate();
        self.entity_to_net_entity_map
            .insert(*entity, new_net_entity);
        self.net_entity_to_entity_map
            .insert(new_net_entity, *entity);
    }

    fn on_entity_channel_opened(&mut self, entity: &Entity) {
        self.delayed_entity_messages.add_entity(entity);
    }

    fn on_entity_channel_closing(&mut self, entity: &Entity) {
        self.delayed_entity_messages.remove_entity(entity);
    }

    fn on_entity_channel_closed(&mut self, entity: &Entity) {
        // cleanup net entity
        let net_entity = self.entity_to_net_entity_map.remove(entity).unwrap();
        self.net_entity_to_entity_map.remove(&net_entity);
        self.net_entity_generator.recycle_key(&net_entity);
    }

    fn on_component_channel_opened(&mut self, entity: &Entity, component: &ComponentId) {
    }

    fn on_component_channel_closing(&mut self, entity: &Entity, component: &ComponentId) {
    }

    // Action Delivery

    pub fn action_delivered(&mut self, action_id: ActionId, action: EntityAction) {
        if self.outgoing_actions.deliver_message(&action_id).is_some() {
            self.delivered_actions.buffer_action(action_id, action);
            self.process_delivered_actions();
        }
    }

    /// Ack that the client has received a given entity action message
    fn process_delivered_actions(&mut self) {
        let delivered_actions = self.delivered_actions.receive_actions();
        for action in delivered_actions {
            match action {
                EntityAction::SpawnEntity(entity, components) => {
                    let component_set: HashSet<ComponentId> = components.iter().copied().collect();
                    self.ack_spawn_entity(entity, component_set);
                }
                EntityAction::DespawnEntity(entity) => {
                    self.ack_despawn_entity(entity);
                }
                EntityAction::InsertComponent(entity, component) => {
                    self.ack_insert_component(entity, component);
                }
                EntityAction::RemoveComponent(entity, component) => {
                    self.ack_remove_component(entity, component);
                }
                EntityAction::Noop => {
                    // do nothing
                }
            }
        }
    }

    // Collect

    pub fn take_next_actions(
        &mut self,
        now: &Instant,
        rtt_millis: &f32,
    ) -> VecDeque<(ActionId, EntityActionEvent)> {
        self.outgoing_actions.collect_messages(now, rtt_millis);
        self.outgoing_actions.take_next_messages()
    }

    pub fn collect_next_updates(&self) -> HashMap<Entity, HashSet<ComponentId>> {
        let mut output = HashMap::new();

        for (entity, entity_channel) in self.entity_channels.iter() {
            if let EntityChannel::Spawned(component_channels) = entity_channel {
                for (component, component_channel) in component_channels.iter() {
                    if let ComponentChannel::Inserted = component_channel {
                        match self.diff_handler.diff_mask_is_clear(entity, component) {
                            None | Some(true) => {
                                // no updates detected, do nothing
                                continue;
                            }
                            _ => {}
                        }

                        if !output.contains_key(entity) {
                            output.insert(*entity, HashSet::new());
                        }
                        let send_component_set = output.get_mut(entity).unwrap();
                        send_component_set.insert(*component);
                    }
                    if let ComponentChannel::Removing = component_channel {
                        #[cfg(feature="debug")]
                        {
                            let e_u16: u16 = (*self.entity_to_net_entity(entity).unwrap()).into();
                            log::info!("have removing component channel for entity {:?}", e_u16);
                        }
                    }
                }
            }
        }

        output
    }

    // Net Entity Conversions

    pub fn entity_to_net_entity(&self, entity: &Entity) -> Option<&NetEntity> {
        self.entity_to_net_entity_map.get(entity)
    }

    pub fn net_entity_to_entity(&self, net_entity: &NetEntity) -> Option<&Entity> {
        self.net_entity_to_entity_map.get(net_entity)
    }
}

// CheckedMap
#[derive(Debug)]
pub struct CheckedMap<K: Eq + Hash, V> {
    pub inner: HashMap<K, V>,
}

impl<K: Eq + Hash, V> CheckedMap<K, V> {
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.inner.contains_key(key)
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.inner.get(key)
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.inner.get_mut(key)
    }

    pub fn insert(&mut self, key: K, value: V) {
        if self.inner.contains_key(&key) {
            panic!("Cannot insert and replace value for given key. Check first.")
        }

        self.inner.insert(key, value);
    }

    pub fn remove(&mut self, key: &K) {
        if !self.inner.contains_key(key) {
            panic!("Cannot remove value for key with non-existent value. Check whether map contains key first.")
        }

        self.inner.remove(key);
    }

    pub fn iter(&self) -> std::collections::hash_map::Iter<K, V> {
        self.inner.iter()
    }
}

// CheckedSet
pub struct CheckedSet<K: Eq + Hash> {
    pub inner: HashSet<K>,
}

impl<K: Eq + Hash> CheckedSet<K> {
    pub fn new() -> Self {
        Self {
            inner: HashSet::new(),
        }
    }

    pub fn contains(&self, key: &K) -> bool {
        self.inner.contains(key)
    }

    pub fn insert(&mut self, key: K) {
        if self.inner.contains(&key) {
            panic!("Cannot insert and replace given key. Check first.")
        }

        self.inner.insert(key);
    }

    pub fn remove(&mut self, key: &K) {
        if !self.inner.contains(key) {
            panic!("Cannot remove given non-existent key. Check first.")
        }

        self.inner.remove(key);
    }
}


#[cfg(test)]
mod tests {
    use crate::server::protocol::world_channel::WorldChannel;

    fn get_world_channel() -> WorldChannel {
        todo!()
    }


    #[test]
    fn spawn_entity() {

    }

}