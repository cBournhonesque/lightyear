use crate::prelude::MessageReceiver;
use crate::registry::{MessageKind, MessageRegistry};
use crate::send::TriggerSender;
use crate::Message;
use bevy::app::App;
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{Entity, EntityMapper, Event};
use lightyear_serde::entity_map::{ReceiveEntityMap, SendEntityMap};
use lightyear_serde::reader::{ReadVarInt, Reader};
use lightyear_serde::registry::{ContextDeserializeFns, ContextSerializeFns, DeserializeFn, SerializeFn, SerializeFns};
use lightyear_serde::writer::{WriteInteger, Writer};
use lightyear_serde::{SerializationError, ToBytes};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tracing::error;
use crate::send_trigger::TriggerSender;

/// The message sent over the network to trigger an event `M` on remote targets.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TriggerMessage<M: Message> {
    pub trigger: M,
    pub target_entities: Vec<Entity>,
}

impl<M: Message> Message for TriggerMessage<M> {}

// TODO: handle the case where the trigger message is not MapEntities
impl<M: Message + MapEntities> MapEntities for TriggerMessage<M> {
    fn map_entities<Mapper: EntityMapper>(&mut self, entity_mapper: &mut Mapper) {
        // Map the inner trigger first if it contains entities
        self.trigger.map_entities(entity_mapper);
        // Map the target entities
        self.target_entities = self
            .target_entities
            .iter()
            .map(|e| entity_mapper.map_entity(*e))
            .collect();
    }
}

pub struct TriggerRegistration<'a, M> {
    pub app: &'a mut App,
    pub(crate) _marker: core::marker::PhantomData<M>,
}

impl<M: Event> TriggerRegistration<'_, M> {

    #[cfg(feature = "test_utils")]
    pub fn new(app: &mut App) -> Self {
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
        registry.add_map_entities::<M>(trigger_serialize_mapped, trigger_deserialize_mapped);
        self
    }
}

pub trait AppTriggerExt {

    /// Register a trigger type `M`.
    fn add_trigger<M: Event + Serialize + DeserializeOwned>(&mut self) -> TriggerRegistration<'_, M> {
        self.register_trigger_custom_serde(SerializeFns::<TriggerMessage<M>>::default());
    }

    /// Register a trigger type `M`.
    fn add_trigger_custom_serde<M: Event>(&mut self, serialize_fns: SerializeFns<M>) -> TriggerRegistration<'_, M>;
}

impl AppTriggerExt for App {
    fn add_trigger_custom_serde<M: Event>(&mut self, serialize_fns: SerializeFns<M>) -> TriggerRegistration<'_, M> {
        if self.world_mut().get_resource_mut::<MessageRegistry>().is_none() {
            self.world_mut().init_resource::<MessageRegistry>();
        }
        let sender_id = self.world_mut().register_component::<TriggerSender<M>>();

        let mut registry = self.world_mut().resource_mut::<MessageRegistry>();
        // Register TriggerMessage<M> for serialization/deserialization
        registry.register_message::<TriggerMessage<M>>(
            ContextSerializeFns::new(serialize_fns.serialize).with_context(trigger_serialize),
            ContextDeserializeFns::new(serialize_fns.deserialize).with_context(trigger_deserialize),
        );
    }
}

    fn add_trigger<M>(&mut self) -> TriggerRegistration<'_, M>
    where
        M: Message + Serialize + DeserializeOwned + Clone + 'static,
        TriggerMessage<M>: Message + Serialize + DeserializeOwned + MapEntities + Clone + 'static,
    {
        if self.world_mut().get_resource_mut::<MessageRegistry>().is_none() {
            self.world_mut().init_resource::<MessageRegistry>();
        }

        // 1. Register the RemoteTrigger<M> event
        self.add_event::<RemoteTrigger<M>>();

        // 2. Register the TriggerSender<M> component
        self.world_mut().register_component::<TriggerSender<M>>();

        // 3. Register the TriggerMessage<M> network message
        //    - This registers serde for TriggerMessage<M>
        //    - This registers MessageSender<TriggerMessage<M>> (used internally by TriggerSender<M>)
        //    - This registers MessageReceiver<TriggerMessage<M>> (needed to get metadata, but component won't be used)
        //    - Crucially, this does NOT set the trigger_fn yet.
        let mut trigger_message_registration = self.add_message_custom_serde::<TriggerMessage<M>>(
            // Assume default serde for TriggerMessage<M> for now.
            // Needs refinement if M uses custom serde that affects TriggerMessage<M>.
            SerializeFns::<TriggerMessage<M>>::default()
        );
        // Explicitly add MapEntities for TriggerMessage<M>
        trigger_message_registration.add_map_entities();

        // 4. Register the inner type M for serde if it wasn't already registered
        //    (add_message_custom_serde above doesn't handle the inner M type)
        //    We only need its serde functions, not sender/receiver components.
        let mut registry = self.world_mut().resource_mut::<MessageRegistry>();
        if !registry.is_registered::<M>() {
            // Use default serde for M here. If M needs custom serde, it should be registered
            // via add_message_custom_serde separately first.
            registry.register_message::<M>();
            // If M needs MapEntities, it should be registered via add_message().add_map_entities() first.
        }

        // 5. Set the trigger_fn for TriggerMessage<M>
        let message_kind = MessageKind::of::<TriggerMessage<M>>();
        if let Some(metadata) = registry.receive_metadata.get_mut(&message_kind) {
            // Set the trigger function pointer to the generic version
            metadata.trigger_fn = Some(MessageReceiver::<M>::receive_trigger_typed::<M>);
        } else {
            // This should not happen if add_message_custom_serde worked correctly
            error!("Failed to find receive metadata for TriggerMessage<{}> after registration.", std::any::type_name::<M>());
        }

        TriggerRegistration {
            app: self,
            _marker: Default::default(),
        }
    }


fn trigger_serialize<M: Event>(
    _: &mut SendEntityMap,
    message: &TriggerMessage<M>,
    writer: &mut Writer,
    serialize: SerializeFn<M>,
) -> Result<(), SerializationError> {
    // Serialize the trigger message
    serialize(&message.trigger, writer)?;
    // Serialize the target entities
    writer.write_varint(message.target_entities.len() as u64)?;
    writer.write_u32(message.target_entities.len() as u32)?;
    for entity in &message.target_entities {
        entity.to_bytes(writer)?;
    }
    Ok(())
}

fn trigger_deserialize<M: Event>(
    _: &mut ReceiveEntityMap,
    reader: &mut Reader,
    deserialize: DeserializeFn<M>,
) -> Result<TriggerMessage<M>, SerializationError> {
    // Serialize the trigger message
    let inner = deserialize(reader)?;
    // Serialize the target entities
    let len = reader.read_varint()?;
    let mut targets = Vec::with_capacity(len as usize);
    for _ in 0..len {
        targets.push(Entity::from_bytes(reader)?);
    };
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
    writer.write_u32(message.target_entities.len() as u32)?;
    for entity in &message.target_entities {
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
        targets.push(Entity::from_bytes(reader)?);
    };
    Ok(TriggerMessage {
        trigger: inner,
        target_entities: targets,
    })
}