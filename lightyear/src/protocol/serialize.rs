use crate::prelude::{ComponentRegistry, Message, MessageRegistry};
use crate::serialize::{reader::Reader, writer::Writer, SerializationError};
use crate::shared::replication::entity_map::EntityMap;
use bevy::app::App;
use bevy::ecs::entity::MapEntities;
use bevy::ptr::{OwningPtr, Ptr, PtrMut};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::any::TypeId;
use std::mem::ManuallyDrop;
use std::ptr::NonNull;

/// Stores function pointers related to serialization and deserialization
#[derive(Clone, Debug, PartialEq)]
pub struct ErasedSerializeFns {
    pub(crate) type_id: TypeId,
    pub(crate) type_name: &'static str,
    // TODO: maybe use `Vec<MaybeUninit<u8>>` instead of unsafe fn(), like bevy?
    pub serialize: unsafe fn(),
    pub erased_serialize: ErasedSerializeFn,
    pub deserialize: unsafe fn(),
    pub map_entities: Option<ErasedMapEntitiesFn>,
    pub map_entities_in_place: Option<ErasedMapEntitiesInPlaceFn>,
}

pub struct SerializeFns<M> {
    pub serialize: SerializeFn<M>,
    pub deserialize: DeserializeFn<M>,
}

type ErasedSerializeFn = unsafe fn(
    erased_serialize_fn: &ErasedSerializeFns,
    message: Ptr,
    writer: &mut Writer,
    entity_map: Option<&mut EntityMap>,
) -> Result<(), SerializationError>;

/// Type of the serialize function without entity mapping
type SerializeFn<M> = fn(message: &M, writer: &mut Writer) -> Result<(), SerializationError>;
/// Type of the deserialize function without entity mapping
type DeserializeFn<M> = fn(reader: &mut Reader) -> Result<M, SerializationError>;

/// Type of the entity mapping function used for serialization.
/// We required a Clone because we modify the data before serializing
pub(crate) type ErasedMapEntitiesFn =
    for<'a> unsafe fn(message: Ptr<'a>, entity_map: &mut EntityMap) -> OwningPtr<'a>;
/// Type of the entity mapping function used for deserialiaztion
pub(crate) type ErasedMapEntitiesInPlaceFn =
    for<'a> unsafe fn(message: PtrMut<'a>, entity_map: &mut EntityMap);

unsafe fn erased_serialize_fn<M: Message>(
    erased_serialize_fn: &ErasedSerializeFns,
    message: Ptr,
    writer: &mut Writer,
    entity_map: Option<&mut EntityMap>,
) -> Result<(), SerializationError> {
    let typed_serialize_fns = erased_serialize_fn.typed::<M>();
    if let Some(map_entities) = erased_serialize_fn.map_entities {
        let message: OwningPtr = map_entities(
            message,
            entity_map.expect("EntityMap is required to serialize this message"),
        );
        // SAFETY: the Ptr was created for the message of type M
        let message = message.read::<M>();
        (typed_serialize_fns.serialize)(&message, writer)?;

        // don't forget to manually drop because map_entities doesn't
        drop(message);
        Ok(())
    } else {
        // SAFETY: the Ptr was created for the message of type M
        let message = message.deref::<M>();
        (typed_serialize_fns.serialize)(message, writer)
    }
}

/// Default serialize function using bincode
fn default_serialize<M: Message + Serialize>(
    message: &M,
    buffer: &mut Writer,
) -> Result<(), SerializationError> {
    let _ = bincode::serde::encode_into_std_write(message, buffer, bincode::config::standard())?;
    Ok(())
}

/// Default deserialize function using bincode
fn default_deserialize<M: Message + DeserializeOwned>(
    buffer: &mut Reader,
) -> Result<M, SerializationError> {
    let data = bincode::serde::decode_from_std_read(buffer, bincode::config::standard())?;
    Ok(data)
}

/// SAFETY: the Ptr must be a valid pointer to a value of type M
unsafe fn erased_map_entities<'a, M: Clone + MapEntities + 'static>(
    message: Ptr<'a>,
    entity_map: &mut EntityMap,
) -> OwningPtr<'a> {
    let message = message.deref::<M>();
    let mut new_message = message.clone();
    M::map_entities(&mut new_message, entity_map);
    OwningPtr::new(NonNull::from(&mut ManuallyDrop::new(new_message)).cast())
}

/// SAFETY: the PtrMut must be a valid pointer to a value of type M
unsafe fn erased_map_entities_in_place<M: MapEntities + 'static>(
    message: PtrMut,
    entity_map: &mut EntityMap,
) {
    let message = message.deref_mut::<M>();
    M::map_entities(message, entity_map);
}

impl ErasedSerializeFns {
    pub(crate) fn new<M: Message + Serialize + DeserializeOwned>() -> Self {
        let serialize_fns = SerializeFns {
            serialize: default_serialize::<M>,
            deserialize: default_deserialize::<M>,
        };
        Self {
            type_id: TypeId::of::<M>(),
            type_name: std::any::type_name::<M>(),
            erased_serialize: erased_serialize_fn::<M>,
            serialize: unsafe { std::mem::transmute(serialize_fns.serialize) },
            deserialize: unsafe { std::mem::transmute(serialize_fns.deserialize) },
            map_entities: None,
            map_entities_in_place: None,
        }
    }

    pub(crate) fn new_custom_serde<M: Message>(serialize_fns: SerializeFns<M>) -> Self {
        Self {
            type_id: TypeId::of::<M>(),
            type_name: std::any::type_name::<M>(),
            erased_serialize: erased_serialize_fn::<M>,
            serialize: unsafe { std::mem::transmute(serialize_fns.serialize) },
            deserialize: unsafe { std::mem::transmute(serialize_fns.deserialize) },
            map_entities: None,
            map_entities_in_place: None,
        }
    }

    pub(crate) unsafe fn typed<M: 'static>(&self) -> SerializeFns<M> {
        debug_assert_eq!(
            self.type_id,
            TypeId::of::<M>(),
            "The erased message fns were created for type {}, but we are trying to convert to type {}",
            self.type_name,
            std::any::type_name::<M>(),
        );
        SerializeFns {
            serialize: unsafe { std::mem::transmute(self.serialize) },
            deserialize: unsafe {
                std::mem::transmute::<
                    unsafe fn(),
                    for<'a> fn(&'a mut Reader) -> std::result::Result<M, SerializationError>,
                >(self.deserialize)
            },
        }
    }

    // We need to be able to clone the data, because when serialize we:
    // - clone the data
    // - map the entities
    // - serialize the cloned data
    // Note that this is fairly inefficient because in most cases (when there is no authority transfer)
    // there is no entity mapping done on the serialization side, just on the deserialization side.
    // However components that contain other entities should be small in general.
    pub(crate) fn add_map_entities<M: Clone + MapEntities + 'static>(&mut self) {
        self.map_entities = Some(erased_map_entities::<M>);
        self.map_entities_in_place = Some(erased_map_entities_in_place::<M>);
    }

    pub(crate) fn map_entities<M: 'static>(&self, message: &mut M, entity_map: &mut EntityMap) {
        let ptr = PtrMut::from(message);
        if let Some(map_entities_fn) = self.map_entities_in_place {
            unsafe {
                map_entities_fn(ptr, entity_map);
            }
        }
    }

    /// Serialize the message into the writer.
    /// If available, we try to map the entities in the message from local to remote.
    ///
    /// SAFETY: the ErasedSerializeFns must be created for the type M
    pub(crate) unsafe fn serialize<M: 'static>(
        &self,
        message: &M,
        writer: &mut Writer,
        entity_map: Option<&mut EntityMap>,
    ) -> Result<(), SerializationError> {
        let fns = unsafe { self.typed::<M>() };
        if let Some(map_entities) = self.map_entities {
            let message: OwningPtr = map_entities(
                Ptr::from(message),
                entity_map.expect("EntityMap is required to serialize this message"),
            );
            let message = message.read::<M>();
            (fns.serialize)(&message, writer)?;
            // don't forget to manually drop because `map_entities` doesn't
            drop(message);
            Ok(())
        } else {
            (fns.serialize)(message, writer)
        }
    }

    /// Deserialize the message value from the reader
    ///
    /// SAFETY: the ErasedSerializeFns must be created for the type M
    pub(crate) unsafe fn deserialize<M: 'static>(
        &self,
        reader: &mut Reader,
        entity_map: &mut EntityMap,
    ) -> Result<M, SerializationError> {
        let fns = unsafe { self.typed::<M>() };
        let mut message = (fns.deserialize)(reader)?;
        if let Some(map_entities) = self.map_entities_in_place {
            map_entities(PtrMut::from(&mut message), entity_map);
        }
        Ok(message)
    }
}

pub trait AppSerializeExt {
    /// Indicate that the type `M` contains Entity references, and that the entities
    /// should be mapped during deserialization
    fn add_map_entities<M: Clone + MapEntities + 'static>(&mut self);
}

impl AppSerializeExt for App {
    // TODO: should we return Result<()> to indicate if adding the map_entities was successful?
    //  otherwise it might not work if the message was not registered before
    // TODO: or have a single SerializeRegistry?
    // TODO: or should we just have add_message_map_entities and add_component_map_entities?
    fn add_map_entities<M: Clone + MapEntities + 'static>(&mut self) {
        let mut registry = self.world_mut().resource_mut::<MessageRegistry>();
        registry.try_add_map_entities::<M>();
        let mut registry = self.world_mut().resource_mut::<ComponentRegistry>();
        registry.try_add_map_entities::<M>();
    }
}
