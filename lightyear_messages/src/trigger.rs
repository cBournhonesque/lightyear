use bevy_app::App;
use bevy_ecs::{entity::MapEntities, event::Event};

use crate::receive_event::receive_event_typed;
use crate::registry::{MessageKind, MessageRegistry, SendTriggerMetadata};
use crate::send_trigger::EventSender;
use lightyear_connection::direction::NetworkDirection;
use lightyear_serde::entity_map::{ReceiveEntityMap, SendEntityMap};
use lightyear_serde::reader::Reader;
use lightyear_serde::registry::{
    ContextDeserializeFns, ContextSerializeFns, DeserializeFn, SerializeFn, SerializeFns,
};
use lightyear_serde::writer::Writer;
use lightyear_serde::{SerializationError, ToBytes};
use serde::Serialize;
use serde::de::DeserializeOwned;

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
        registry.add_map_entities::<M, M>(trigger_serialize_mapped, trigger_deserialize_mapped);
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
    fn register_event<M: Event + Serialize + DeserializeOwned>(
        &mut self,
    ) -> TriggerRegistration<'_, M> {
        self.register_event_custom_serde(SerializeFns::<M>::default())
    }

    /// Register a trigger type `M`.
    fn register_event_custom_serde<M: Event>(
        &mut self,
        serialize_fns: SerializeFns<M>,
    ) -> TriggerRegistration<'_, M>;

    #[doc(hidden)]
    /// Register a trigger type `M`.
    fn register_event_to_bytes<M: Event + ToBytes>(&mut self) -> TriggerRegistration<'_, M> {
        self.register_event_custom_serde(SerializeFns::<M>::with_to_bytes())
    }
}

impl AppTriggerExt for App {
    fn register_event_custom_serde<M: Event>(
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
        let sender_id = self.world_mut().register_component::<EventSender<M>>();

        let mut registry = self.world_mut().resource_mut::<MessageRegistry>();
        // Register M for serialization/deserialization
        registry.register_message::<M, M>(
            ContextSerializeFns::new(serialize_fns.serialize).with_context(trigger_serialize),
            ContextDeserializeFns::new(serialize_fns.deserialize).with_context(trigger_deserialize),
        );

        registry.send_trigger_metadata.insert(
            MessageKind::of::<M>(),
            SendTriggerMetadata {
                component_id: sender_id,
                send_trigger_fn: EventSender::<M>::send_event_typed,
                send_local_trigger_fn: EventSender::<M>::send_local_trigger_typed,
            },
        );
        registry
            .receive_trigger
            .insert(MessageKind::of::<M>(), receive_event_typed::<M>);
        TriggerRegistration {
            app: self,
            _marker: Default::default(),
        }
    }
}

fn trigger_serialize<M: Event>(
    _: &mut SendEntityMap,
    message: &M,
    writer: &mut Writer,
    serialize: SerializeFn<M>,
) -> Result<(), SerializationError> {
    // Serialize the trigger message
    serialize(message, writer)?;
    Ok(())
}

fn trigger_deserialize<M: Event>(
    _: &mut ReceiveEntityMap,
    reader: &mut Reader,
    deserialize: DeserializeFn<M>,
) -> Result<M, SerializationError> {
    deserialize(reader)
}

fn trigger_serialize_mapped<M: Event + MapEntities + Clone>(
    mapper: &mut SendEntityMap,
    event: &M,
    writer: &mut Writer,
    serialize: SerializeFn<M>,
) -> Result<(), SerializationError> {
    let mut event = event.clone();
    event.map_entities(mapper);
    serialize(&event, writer)?;
    Ok(())
}

fn trigger_deserialize_mapped<M: Event + MapEntities>(
    mapper: &mut ReceiveEntityMap,
    reader: &mut Reader,
    deserialize: DeserializeFn<M>,
) -> Result<M, SerializationError> {
    // Serialize the trigger message
    let mut inner = deserialize(reader)?;
    inner.map_entities(mapper);
    Ok(inner)
}
