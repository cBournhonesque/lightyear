//! Wrapper around [`ConnectionEvents`] that adds server-specific functionality

use bevy::ecs::entity::EntityHash;
use bevy::prelude::*;
use bevy::utils::{hashbrown, HashMap};
use std::ops::DerefMut;

use crate::connection::id::ClientId;
use crate::prelude::server::is_started;
use crate::prelude::{ComponentRegistry, Message, MessageRegistry, NetworkTarget};
use crate::protocol::event::EventReplicationMode;
use crate::protocol::message::{MessageKind, MessageType};
use crate::serialize::reader::Reader;
use crate::server::connection::ConnectionManager;
use crate::shared::events::connection::{
    ConnectionEvents, IterComponentInsertEvent, IterComponentRemoveEvent, IterComponentUpdateEvent,
    IterEntityDespawnEvent, IterEntitySpawnEvent,
};
use crate::shared::events::plugin::EventsPlugin;
use crate::shared::events::systems::push_component_events;
use crate::shared::sets::{InternalMainSet, ServerMarker};

type EntityHashMap<K, V> = hashbrown::HashMap<K, V, EntityHash>;

/// Plugin that adds bevy [`Events`] related to networking and replication
#[derive(Default)]
pub struct ServerEventsPlugin;

impl Plugin for ServerEventsPlugin {
    fn build(&self, app: &mut App) {
        app
            // EVENTS
            .add_event::<ConnectEvent>()
            .add_event::<DisconnectEvent>()
            // PLUGIN
            .add_plugins(EventsPlugin::<ConnectionManager>::default())
            // SYSTEMS
            .add_systems(
                PreUpdate,
                // TODO: check if this should be between Receive and EmitEvents
                emit_connect_events.in_set(InternalMainSet::<ServerMarker>::EmitEvents),
            );
    }
}

/// Read the events received from the clients and emits the MessageEvent event
fn read_event<E: Event + Message>(
    mut commands: Commands,
    message_registry: Res<MessageRegistry>,
    mut connection_manager: ResMut<ConnectionManager>,
    // mut message_event: EventWriter<MessageEvent<E>>,
    mut event_writer: EventWriter<E>,
) {
    let kind = MessageKind::of::<E>();
    let Some(net) = message_registry.kind_map.net_id(&kind).copied() else {
        error!(
            "Could not find the network id for the message kind: {:?}",
            kind
        );
        return;
    };
    assert_eq!(
        message_registry.message_type(net),
        MessageType::Event,
        "The message must be registered as an event in the protocol by calling `is_event()`"
    );
    // re-borrow to allow split borrows
    let connection_manager = connection_manager.deref_mut();
    for (client_id, connection) in connection_manager.connections.iter_mut() {
        if let Some(event_list) = connection.received_events.remove(&net) {
            for (event_bytes, target, channel_kind) in event_list {
                let mut reader = Reader::from(event_bytes);
                match message_registry.deserialize_event::<E>(
                    &mut reader,
                    &mut connection
                        .replication_receiver
                        .remote_entity_map
                        .remote_to_local,
                ) {
                    Ok((message, event_replication_mode)) => {
                        // rebroadcast
                        if target != NetworkTarget::None {
                            connection.messages_to_rebroadcast.push((
                                reader.consume(),
                                target,
                                channel_kind,
                            ));
                        }
                        trace!("Received message: {:?}", std::any::type_name::<E>());
                        match event_replication_mode {
                            // EventReplicationMode::None => {
                            //     message_event.send(MessageEvent::new(message, *client_id));
                            // }
                            EventReplicationMode::Buffer => {
                                event_writer.send(message);
                            }
                            EventReplicationMode::Trigger => {
                                commands.trigger(message);
                            }
                        }
                    }
                    Err(e) => {
                        error!(
                            "Could not deserialize message {}: {:?}",
                            std::any::type_name::<E>(),
                            e
                        );
                    }
                }
            }
        }
    }
}

/// Register an event that can be sent from client to server
pub(crate) fn add_server_receive_event_from_client<E: Event + Message>(app: &mut App) {
    app.add_event::<E>();
    app.add_systems(
        PreUpdate,
        read_event::<E>
            .in_set(InternalMainSet::<ServerMarker>::EmitEvents)
            .run_if(is_started),
    );
}

/// Emit events related to connections and disconnections
fn emit_connect_events(
    mut commands: Commands,
    mut connect_events: EventWriter<ConnectEvent>,
    mut disconnect_events: EventWriter<DisconnectEvent>,
    mut connection_manager: ResMut<ConnectionManager>,
) {
    // EVENTS: Write the received events into bevy events
    if !connection_manager.events.is_empty() {
        // Connection / Disconnection events
        if connection_manager.events.has_connections() {
            for connect_event in connection_manager.events.iter_connections() {
                debug!("Client connected event: {}", connect_event.client_id);
                connect_events.send(connect_event);
                // TODO: trigger all events in batch? https://github.com/bevyengine/bevy/pull/13953
                // NOTE: we don't trigger the event immediately because we're inside world.resource_scope
                //  so a bunch of Resources have been removed from the World
                commands.trigger(connect_event);
                // world.trigger(connect_event);
            }
        }

        if connection_manager.events.has_disconnections() {
            for disconnect_event in connection_manager.events.iter_disconnections() {
                debug!("Client disconnected event: {}", disconnect_event.client_id);
                disconnect_events.send(disconnect_event);
                // TODO: trigger all events in batch? https://github.com/bevyengine/bevy/pull/13953
                // NOTE: we don't trigger the event immediately because we're inside world.resource_scope
                //  so a bunch of Resources have been removed from the World
                commands.trigger(disconnect_event);
                // world.trigger(disconnect_event);
            }
        }
    }
}

#[derive(Debug)]
pub struct ServerEvents {
    pub connections: Vec<ConnectEvent>,
    pub disconnections: Vec<DisconnectEvent>,
    pub events: HashMap<ClientId, ConnectionEvents>,
    pub empty: bool,
}

pub(crate) fn emit_replication_events<C: Component>(app: &mut App) {
    app.add_event::<ComponentUpdateEvent<C>>();
    app.add_event::<ComponentInsertEvent<C>>();
    app.add_event::<ComponentRemoveEvent<C>>();
    app.add_systems(
        PreUpdate,
        push_component_events::<C, ConnectionManager>
            .in_set(InternalMainSet::<ServerMarker>::EmitEvents),
    );
}

impl crate::shared::events::connection::ClearEvents for ServerEvents {
    fn clear(&mut self) {
        self.connections = Vec::new();
        self.disconnections = Vec::new();
        self.empty = true;
        self.events = HashMap::default();
    }
}

impl ServerEvents {
    pub(crate) fn new() -> Self {
        Self {
            connections: Vec::new(),
            disconnections: Vec::new(),
            events: HashMap::default(),
            empty: true,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.empty
    }

    // TODO: could also return a IntoIterMessages struct and impl Iterator for that

    // TODO: seems like we cannot chain iterators like this; because then we need to keep &mut Self around
    //  instead we want to take the contents
    // pub fn into_iter_messages<M: Message>(&mut self) -> impl Iterator<Item = (M, ClientId)> + '_
    // where
    //     P::Message: TryInto<M, Error = ()>,
    // {
    //     self.events.iter_mut().flat_map(|(client_id, events)| {
    //         let messages = events.into_iter_messages::<M>();
    //         let client_ids = std::iter::once(client_id.clone()).cycle();
    //         return messages.zip(client_ids);
    //     })
    // }
    //
    // pub fn has_messages<M: Message>(&mut self) -> bool {
    //     self.events
    //         .iter()
    //         .any(|(_, connection_events)| connection_events.has_messages::<M>())
    // }

    // TODO: should we consume connections?
    pub fn iter_connections(&mut self) -> Vec<ConnectEvent> {
        std::mem::take(&mut self.connections)
    }

    pub fn has_connections(&self) -> bool {
        !self.connections.is_empty()
    }

    pub fn iter_disconnections(&mut self) -> Vec<DisconnectEvent> {
        std::mem::take(&mut self.disconnections)
    }

    pub fn has_disconnections(&self) -> bool {
        !self.disconnections.is_empty()
    }

    pub(crate) fn add_connect_event(&mut self, connect_event: ConnectEvent) {
        self.connections.push(connect_event);
        self.empty = false;
    }

    pub(crate) fn add_disconnect_event(&mut self, disconnect_event: DisconnectEvent) {
        self.disconnections.push(disconnect_event);
        self.events.remove(&disconnect_event.client_id);
        self.empty = false;
    }

    pub(crate) fn push_events(&mut self, client_id: ClientId, events: ConnectionEvents) {
        if !events.is_empty() {
            self.events.insert(client_id, events);
            self.empty = false;
        }
    }
}

impl IterEntitySpawnEvent<ClientId> for ServerEvents {
    fn into_iter_entity_spawn(&mut self) -> Box<dyn Iterator<Item = (Entity, ClientId)> + '_> {
        Box::new(self.events.iter_mut().flat_map(|(client_id, events)| {
            let entities = events.into_iter_entity_spawn().map(|(entity, _)| entity);
            let client_ids = std::iter::once(*client_id).cycle();
            entities.zip(client_ids)
        }))
    }

    fn has_entity_spawn(&self) -> bool {
        self.events
            .iter()
            .any(|(_, connection_events)| connection_events.has_entity_spawn())
    }
}

impl IterEntityDespawnEvent<ClientId> for ServerEvents {
    fn into_iter_entity_despawn(&mut self) -> Box<dyn Iterator<Item = (Entity, ClientId)> + '_> {
        Box::new(self.events.iter_mut().flat_map(|(client_id, events)| {
            let entities = events.into_iter_entity_despawn().map(|(entity, _)| entity);
            let client_ids = std::iter::once(*client_id).cycle();
            entities.zip(client_ids)
        }))
    }

    fn has_entity_despawn(&self) -> bool {
        self.events
            .iter()
            .any(|(_, connection_events)| connection_events.has_entity_despawn())
    }
}

impl IterComponentUpdateEvent<ClientId> for ServerEvents {
    fn iter_component_update<'a, 'b: 'a, C: Component>(
        &'a mut self,
        component_registry: &'b ComponentRegistry,
    ) -> Box<dyn Iterator<Item = (Entity, ClientId)> + 'a> {
        Box::new(self.events.iter_mut().flat_map(|(client_id, events)| {
            let updates = events
                .iter_component_update::<C>(component_registry)
                .map(|(entity, _)| entity);
            let client_ids = std::iter::once(*client_id).cycle();
            updates.zip(client_ids)
        }))
    }
}

impl IterComponentRemoveEvent<ClientId> for ServerEvents {
    fn iter_component_remove<'a, 'b: 'a, C: Component>(
        &'a mut self,
        component_registry: &'b ComponentRegistry,
    ) -> Box<dyn Iterator<Item = (Entity, ClientId)> + 'a> {
        Box::new(self.events.iter_mut().flat_map(|(client_id, events)| {
            let updates = events
                .iter_component_remove::<C>(component_registry)
                .map(|(entity, _)| entity);
            let client_ids = std::iter::once(*client_id).cycle();
            updates.zip(client_ids)
        }))
    }
}

impl IterComponentInsertEvent<ClientId> for ServerEvents {
    fn iter_component_insert<'a, 'b: 'a, C: Component>(
        &'a mut self,
        component_registry: &'b ComponentRegistry,
    ) -> Box<dyn Iterator<Item = (Entity, ClientId)> + 'a> {
        Box::new(self.events.iter_mut().flat_map(|(client_id, events)| {
            let updates = events
                .iter_component_insert::<C>(component_registry)
                .map(|(entity, _)| entity);
            let client_ids = std::iter::once(*client_id).cycle();
            updates.zip(client_ids)
        }))
    }
}

/// Bevy [`Event`] emitted on the server on the frame where a client is connected
#[derive(Event, Debug, Copy, Clone)]
pub struct ConnectEvent {
    pub client_id: ClientId,
    pub entity: Entity,
}

/// Bevy [`Event`] emitted on the server on the frame where a client is disconnected
#[derive(Event, Debug, Copy, Clone)]
pub struct DisconnectEvent {
    pub client_id: ClientId,
    pub entity: Entity,
}

/// Bevy [`Event`] emitted on the server on the frame where an input message from a client is received
pub type InputEvent<I> = crate::shared::events::components::InputEvent<I, ClientId>;
/// Bevy [`Event`] emitted on the server on the frame where a EntitySpawn replication message is received
pub type EntitySpawnEvent = crate::shared::events::components::EntitySpawnEvent<ClientId>;
/// Bevy [`Event`] emitted on the server on the frame where a EntityDepawn replication message is received
pub type EntityDespawnEvent = crate::shared::events::components::EntityDespawnEvent<ClientId>;
/// Bevy [`Event`] emitted on the server on the frame where a ComponentUpdate replication message is received
pub type ComponentUpdateEvent<C> =
    crate::shared::events::components::ComponentUpdateEvent<C, ClientId>;
/// Bevy [`Event`] emitted on the server on the frame where a ComponentInsert replication message is received
pub type ComponentInsertEvent<C> =
    crate::shared::events::components::ComponentInsertEvent<C, ClientId>;
/// Bevy [`Event`] emitted on the server on the frame where a ComponentRemove replication message is received
pub type ComponentRemoveEvent<C> =
    crate::shared::events::components::ComponentRemoveEvent<C, ClientId>;

/// Bevy [`Event`] emitted on the server on the frame where a (non-replication) message is received
pub type MessageEvent<M> = crate::shared::events::components::MessageEvent<M, ClientId>;

#[cfg(test)]
mod tests {
    use crate::prelude::Tick;
    use crate::protocol::channel::ChannelKind;
    use crate::tests::protocol::{
        Channel1, Channel2, ComponentSyncModeFull, ComponentSyncModeOnce, StringMessage,
    };

    use super::*;

    #[test]
    fn test_iter_component_removes() {
        let client_1 = ClientId::Netcode(1);
        let client_2 = ClientId::Netcode(2);
        let entity_1 = Entity::from_raw(0);
        let entity_2 = Entity::from_raw(1);
        let mut events_1 = ConnectionEvents::new();
        let channel_kind_1 = ChannelKind::of::<Channel1>();
        let channel_kind_2 = ChannelKind::of::<Channel2>();
        let message1_a = StringMessage("hello".to_string());
        let message1_b = StringMessage("world".to_string());
        let mut component_registry = ComponentRegistry::default();
        component_registry.register_component::<ComponentSyncModeFull>();
        component_registry.register_component::<ComponentSyncModeOnce>();
        let net_id_1 = component_registry.net_id::<ComponentSyncModeFull>();
        let net_id_2 = component_registry.net_id::<ComponentSyncModeOnce>();
        events_1.push_remove_component(entity_1, net_id_1, Tick(0));
        events_1.push_remove_component(entity_1, net_id_2, Tick(0));
        events_1.push_remove_component(entity_2, net_id_1, Tick(0));
        let mut server_events = ServerEvents::new();
        server_events.push_events(client_1, events_1);

        let mut events_2 = ConnectionEvents::new();
        events_2.push_remove_component(entity_2, net_id_2, Tick(0));
        server_events.push_events(client_2, events_2);

        // check that we have the correct messages
        let data: Vec<(Entity, ClientId)> = server_events
            .iter_component_remove::<ComponentSyncModeFull>(&component_registry)
            .collect();
        assert_eq!(data.len(), 2);
        assert!(data.contains(&(entity_1, client_1)));
        assert!(data.contains(&(entity_2, client_1)));

        let data: Vec<(Entity, ClientId)> = server_events
            .iter_component_remove::<ComponentSyncModeOnce>(&component_registry)
            .collect();
        assert_eq!(data.len(), 2);
        assert!(data.contains(&(entity_1, client_1)));
        assert!(data.contains(&(entity_2, client_2)));
    }
}
