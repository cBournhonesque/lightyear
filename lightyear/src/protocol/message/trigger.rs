use crate::client::config::ClientConfig;
use crate::prelude::server::ServerConfig;
use crate::prelude::{AppSerializeExt, ChannelDirection, Deserialize, Message, MessageRegistry};
use crate::protocol::message::registry::AppMessageInternalExt;
use crate::protocol::SerializeFns;
use bevy::app::App;
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{Entity, EntityMapper};
use serde::de::DeserializeOwned;
use serde::Serialize;
#[cfg(not(feature = "std"))]
use {
    alloc::{vec::Vec},
};

pub trait AppTriggerExt {
    /// Registers an [`Event`] that can be triggered over the network
    fn register_trigger<E: Message + Clone + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
    );

    /// Registers an [`Event`] that can be triggered over the network
    ///
    /// You need to provide your own [`SerializeFns`] for this message
    fn register_trigger_custom_serde<E: Message + Clone>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<TriggerMessage<E>>,
    );
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TriggerMessage<E> {
    // TODO: we want to use &E for serialization, E for deserialization
    pub(crate) event: E,
    pub(crate) target_entities: Vec<Entity>,
    // TODO: not useful right now since we cannot construct TriggerTargets that are both entities and components
    // target_components: Vec<ComponentKind>,
}

impl<E> MapEntities for TriggerMessage<E> {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.target_entities.iter_mut().for_each(|e| {
            *e = entity_mapper.get_mapped(*e);
        });
    }
}

// struct TriggerMessageMapped<E: MapEntities> {
//     message: E,
//     target_entities: Vec<Entity>,
//     target_components: Vec<ComponentKind>,
// }
//
// impl<E: MapEntities> TriggerMessageMapped<E> {
//     fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
//         self.message.map_entities(entity_mapper);
//         self.target_entities = self.target_entities.iter().map(|e| entity_mapper.get_mapped(*e)).collect();
//     }
// }

impl AppTriggerExt for App {
    fn register_trigger<E: Message + Clone + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
    ) {
        self.register_trigger_custom_serde(direction, SerializeFns::<TriggerMessage<E>>::default());
    }

    // TODO: be able to register_trigger for triggers that are not clone!
    // TODO: register_trigger_mapped? in case E also has entity mapping?

    /// Register a resource to be automatically replicated over the network
    fn register_trigger_custom_serde<E: Message + Clone>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<TriggerMessage<E>>,
    ) {
        self.register_message_internal_custom_serde::<TriggerMessage<E>>(direction, serialize_fns);
        self.add_map_entities::<TriggerMessage<E>>();
        register_trigger::<E>(self, direction);
    }
}

/// Register the trigger-receive metadata for a given message E
pub(crate) fn register_trigger<E: Message>(app: &mut App, direction: ChannelDirection) {
    let is_client = app.world().get_resource::<ClientConfig>().is_some();
    let is_server = app.world().get_resource::<ServerConfig>().is_some();
    match direction {
        ChannelDirection::ClientToServer => {
            if is_server {
                MessageRegistry::register_server_trigger_receive::<E>(app);
            };
        }
        ChannelDirection::ServerToClient => {
            if is_client {
                MessageRegistry::register_client_trigger_receive::<E>(app);
            };
        }
        ChannelDirection::Bidirectional => {
            register_trigger::<E>(app, ChannelDirection::ClientToServer);
            register_trigger::<E>(app, ChannelDirection::ServerToClient);
        }
    }
}
