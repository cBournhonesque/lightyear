#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use crate::receive_trigger::receive_trigger_typed;
use crate::registry::{MessageKind, MessageRegistry, SendTriggerMetadata};
use crate::send_trigger::TriggerSender;
use crate::Message;
use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;
use lightyear_connection::direction::NetworkDirection;
use lightyear_serde::entity_map::{ReceiveEntityMap, SendEntityMap};
use lightyear_serde::reader::{ReadVarInt, Reader};
use lightyear_serde::registry::{
    ContextDeserializeFns, ContextSerializeFns, DeserializeFn, SerializeFn, SerializeFns,
};
use lightyear_serde::writer::{WriteInteger, Writer};
use lightyear_serde::{SerializationError, ToBytes};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The message sent over the network to trigger an event `M` on remote targets.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TriggerMessage<M: Message> {
    pub trigger: M,
    pub target_entities: Vec<Entity>,
}

// TODO: handle the case where the trigger message is not MapEntities
impl<M: Message + MapEntities> MapEntities for TriggerMessage<M> {
    fn map_entities<Mapper: EntityMapper>(&mut self, entity_mapper: &mut Mapper) {
        // only map the trigger as the target entities are mapped separately
        self.trigger.map_entities(entity_mapper);
    }
}

pub struct TriggerRegistration<'a, M> {
    pub app: &'a mut App,
    pub(crate) _marker: core::marker::PhantomData<M>,
}

impl<'a, M: Event> TriggerRegistration<'a, M> {
    #[cfg(feature = "test_utils")]
    pub fn new(app: &'a mut App) -> Self {
        Self {
            app,
            _marker: core::marker::PhantomData,
        }
    }

    /// Specify that the Trigger contains entities which should be mapped from the remote world to the local world
    /// upon deserialization
    pub fn add_map_entities(&mut self) -> &mut Self
    where
        M: Clone + MapEntities + 'static,
    {
        let mut registry = self.app.world_mut().resource_mut::<MessageRegistry>();
        registry.add_map_entities::<TriggerMessage<M>, M>(
            trigger_serialize_mapped,
            trigger_deserialize_mapped,
        );
        self
    }

    pub fn add_direction(&mut self, direction: NetworkDirection) -> &mut Self {
        #[cfg(feature = "client")]
        self.add_client_direction(direction);
        #[cfg(feature = "server")]
        self.add_server_direction(direction);
        self
    }
}

pub trait AppTriggerExt {
    /// Register a trigger type `M`.
    fn add_trigger<M: Event + Serialize + DeserializeOwned>(
        &mut self,
    ) -> TriggerRegistration<'_, M> {
        self.add_trigger_custom_serde(SerializeFns::<M>::default())
    }

    /// Register a trigger type `M`.
    fn add_trigger_custom_serde<M: Event>(
        &mut self,
        serialize_fns: SerializeFns<M>,
    ) -> TriggerRegistration<'_, M>;

    #[doc(hidden)]
    /// Register a trigger type `M`.
    fn add_trigger_to_bytes<M: Event + ToBytes>(&mut self) -> TriggerRegistration<'_, M> {
        self.add_trigger_custom_serde(SerializeFns::<M>::with_to_bytes())
    }
}

impl AppTriggerExt for App {
    fn add_trigger_custom_serde<M: Event>(
        &mut self,
        serialize_fns: SerializeFns<M>,
    ) -> TriggerRegistration<'_, M> {
        if self
            .world_mut()
            .get_resource_mut::<MessageRegistry>()
            .is_none()
        {
            self.world_mut().init_resource::<MessageRegistry>();
        }
        let sender_id = self.world_mut().register_component::<TriggerSender<M>>();

        let mut registry = self.world_mut().resource_mut::<MessageRegistry>();
        // Register TriggerMessage<M> for serialization/deserialization
        registry.register_message::<TriggerMessage<M>, M>(
            ContextSerializeFns::new(serialize_fns.serialize).with_context(trigger_serialize),
            ContextDeserializeFns::new(serialize_fns.deserialize).with_context(trigger_deserialize),
        );

        registry.send_trigger_metadata.insert(
            MessageKind::of::<M>(),
            SendTriggerMetadata {
                component_id: sender_id,
                send_trigger_fn: TriggerSender::<M>::send_trigger_typed,
                send_local_trigger_fn: TriggerSender::<M>::send_local_trigger_typed,
            },
        );
        registry
            .receive_trigger
            .insert(MessageKind::of::<M>(), receive_trigger_typed::<M>);
        TriggerRegistration {
            app: self,
            _marker: Default::default(),
        }
    }
}

fn trigger_serialize<M: Event>(
    mapper: &mut SendEntityMap,
    message: &TriggerMessage<M>,
    writer: &mut Writer,
    serialize: SerializeFn<M>,
) -> Result<(), SerializationError> {
    // Serialize the trigger message
    serialize(&message.trigger, writer)?;
    // Serialize the target entities
    writer.write_varint(message.target_entities.len() as u64)?;

    trace!("serialize trigger: {:?}", core::any::type_name::<M>());
    for entity in &message.target_entities {
        trace!("serialize entity before map: {entity:?}");
        let mut entity = *entity;
        entity.map_entities(mapper);
        trace!("serialize entity after map: {entity:?}");
        entity.to_bytes(writer)?;
    }
    Ok(())
}

fn trigger_deserialize<M: Event>(
    mapper: &mut ReceiveEntityMap,
    reader: &mut Reader,
    deserialize: DeserializeFn<M>,
) -> Result<TriggerMessage<M>, SerializationError> {
    // Serialize the trigger message
    let inner = deserialize(reader)?;
    // Serialize the target entities
    let len = reader.read_varint()?;
    let mut targets = Vec::with_capacity(len as usize);
    for _ in 0..len {
        let mut entity = Entity::from_bytes(reader)?;
        entity.map_entities(mapper);
        targets.push(entity);
    }
    Ok(TriggerMessage {
        trigger: inner,
        target_entities: targets,
    })
}

fn trigger_serialize_mapped<M: Event + MapEntities + Clone>(
    mapper: &mut SendEntityMap,
    message: &TriggerMessage<M>,
    writer: &mut Writer,
    serialize: SerializeFn<M>,
) -> Result<(), SerializationError> {
    let mut trigger = message.trigger.clone();
    trigger.map_entities(mapper);
    // Serialize the trigger message
    serialize(&message.trigger, writer)?;
    // Serialize the target entities
    writer.write_varint(message.target_entities.len() as u64)?;
    for entity in &message.target_entities {
        let mut entity = *entity;
        entity.map_entities(mapper);
        entity.to_bytes(writer)?;
    }
    Ok(())
}

fn trigger_deserialize_mapped<M: Event + MapEntities>(
    mapper: &mut ReceiveEntityMap,
    reader: &mut Reader,
    deserialize: DeserializeFn<M>,
) -> Result<TriggerMessage<M>, SerializationError> {
    // Serialize the trigger message
    let mut inner = deserialize(reader)?;
    inner.map_entities(mapper);
    // Serialize the target entities
    let len = reader.read_varint()?;
    let mut targets = Vec::with_capacity(len as usize);
    for _ in 0..len {
        let mut entity = Entity::from_bytes(reader)?;
        entity.map_entities(mapper);
        targets.push(entity);
    }
    Ok(TriggerMessage {
        trigger: inner,
        target_entities: targets,
    })
}
