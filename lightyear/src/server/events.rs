//! Wrapper around [`ConnectionEvents`] that adds server-specific functionality
use bevy::ecs::entity::EntityHash;
use bevy::prelude::*;
use bevy::utils::HashMap;
use tracing::trace;

use crate::_reexport::{
    FromType, IterComponentInsertEvent, IterComponentRemoveEvent, IterComponentUpdateEvent,
    ServerMarker,
};
use crate::connection::id::ClientId;
#[cfg(feature = "leafwing")]
use crate::inputs::leafwing::{InputMessage, LeafwingUserAction};
use crate::packet::message::Message;
use crate::prelude::{server, ComponentRegistry};
use crate::protocol::Protocol;
use crate::server::connection::ConnectionManager;
use crate::server::networking::{clear_events, is_started};
#[cfg(feature = "leafwing")]
use crate::shared::events::connection::IterInputMessageEvent;
use crate::shared::events::connection::{
    ConnectionEvents, IterEntityDespawnEvent, IterEntitySpawnEvent, IterMessageEvent,
};
use crate::shared::events::plugin::EventsPlugin;
use crate::shared::events::systems::push_component_events;
use crate::shared::sets::InternalMainSet;

type EntityHashMap<K, V> = hashbrown::HashMap<K, V, EntityHash>;

/// Plugin that handles generating bevy [`Events`] related to networking and replication
#[derive(Default)]
pub struct ServerEventsPlugin;

impl Plugin for ServerEventsPlugin {
    fn build(&self, app: &mut App) {
        app
            // PLUGIN
            .add_plugins(EventsPlugin::<ClientId>::default())
            // SYSTEM_SET
            .add_systems(PostUpdate, clear_events.run_if(is_started));
    }
}

#[derive(Debug)]
pub struct ServerEvents {
    pub connections: Vec<ClientId>,
    pub disconnections: Vec<ClientId>,
    pub events: HashMap<ClientId, ConnectionEvents>,
    pub empty: bool,
}

pub(crate) fn emit_replication_events<C: Component>(app: &mut App) {
    app.add_systems(
        PreUpdate,
        push_component_events::<C, ConnectionManager>
            .after(InternalMainSet::<ServerMarker>::Receive)
            .run_if(is_started),
    );
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

    /// Clear all events except for the input buffer which we want to keep around
    pub(crate) fn clear(&mut self) {
        self.connections = Vec::new();
        self.disconnections = Vec::new();
        self.empty = true;
        self.events = HashMap::default();
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
    pub fn iter_connections(&mut self) -> impl Iterator<Item = ClientId> + '_ {
        std::mem::take(&mut self.connections).into_iter()
    }

    pub fn has_connections(&self) -> bool {
        !self.connections.is_empty()
    }

    pub fn iter_disconnections(&mut self) -> impl Iterator<Item = ClientId> + '_ {
        std::mem::take(&mut self.disconnections).into_iter()
    }

    pub fn has_disconnections(&self) -> bool {
        !self.disconnections.is_empty()
    }

    pub(crate) fn push_connection(&mut self, client_id: ClientId) {
        self.connections.push(client_id);
        // self.events.remove(&client_id);
        self.empty = false;
    }

    pub(crate) fn push_disconnection(&mut self, client_id: ClientId) {
        self.disconnections.push(client_id);
        self.events.remove(&client_id);
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
    fn iter_component_update<C: Component>(
        &mut self,
        component_registry: &ComponentRegistry,
    ) -> Box<dyn Iterator<Item = (Entity, ClientId)> + '_> {
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
    fn iter_component_remove<C: Component>(
        &mut self,
        component_registry: &ComponentRegistry,
    ) -> Box<dyn Iterator<Item = (Entity, ClientId)> + '_> {
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
    fn iter_component_insert<C: Component>(
        &mut self,
        component_registry: &ComponentRegistry,
    ) -> Box<dyn Iterator<Item = (Entity, ClientId)> + '_> {
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
pub type ConnectEvent = crate::shared::events::components::ConnectEvent<ClientId>;
/// Bevy [`Event`] emitted on the server on the frame where a client is disconnected
pub type DisconnectEvent = crate::shared::events::components::DisconnectEvent<ClientId>;
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
    use crate::protocol::channel::ChannelKind;
    use crate::tests::protocol::{
        Channel1, Channel2, Message1, Message2, MyMessageProtocol, MyProtocol,
    };

    use super::*;

    #[test]
    fn test_iter_messages() {
        let client_1 = ClientId::Netcode(1);
        let client_2 = ClientId::Netcode(2);
        let mut events_1 = ConnectionEvents::<MyProtocol>::new();
        let channel_kind_1 = ChannelKind::of::<Channel1>();
        let channel_kind_2 = ChannelKind::of::<Channel2>();
        let message1_a = Message1("hello".to_string());
        let message1_b = Message1("world".to_string());
        events_1.push_message(
            channel_kind_1,
            MyMessageProtocol::Message1(message1_a.clone()),
        );
        events_1.push_message(
            channel_kind_2,
            MyMessageProtocol::Message1(message1_b.clone()),
        );
        events_1.push_message(channel_kind_1, MyMessageProtocol::Message2(Message2(1)));

        let mut server_events = ServerEvents::<MyProtocol>::new();
        server_events.push_events(client_1, events_1);

        let mut events_2 = ConnectionEvents::<MyProtocol>::new();
        let message1_c = Message1("test".to_string());
        events_2.push_message(
            channel_kind_1,
            MyMessageProtocol::Message1(message1_c.clone()),
        );
        events_2.push_message(channel_kind_1, MyMessageProtocol::Message2(Message2(2)));

        server_events.push_events(client_2, events_2);

        // check that we have the correct messages
        let messages: Vec<(Message1, ClientId)> = server_events.into_iter_messages().collect();
        assert_eq!(messages.len(), 3);
        assert!(messages.contains(&(message1_a, client_1)));
        assert!(messages.contains(&(message1_b, client_1)));
        assert!(messages.contains(&(message1_c, client_2)));

        // check that there are no more message of that kind in the events
        assert!(!server_events
            .events
            .get(&client_1)
            .unwrap()
            .has_messages::<Message1>());
        assert!(!server_events
            .events
            .get(&client_2)
            .unwrap()
            .has_messages::<Message1>());

        // check that we still have the other message kinds
        assert!(server_events
            .events
            .get(&client_2)
            .unwrap()
            .has_messages::<Message2>());
    }
}
