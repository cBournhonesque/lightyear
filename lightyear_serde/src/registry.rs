use crate::entity_map::{EntityMap, ReceiveEntityMap, SendEntityMap};
use crate::reader::Reader;
use crate::writer::Writer;
use crate::{SerializationError, ToBytes};
use bevy::ecs::entity::MapEntities;
use bevy::ptr::{Ptr, PtrMut};
use core::any::TypeId;
use serde::de::DeserializeOwned;
use serde::Serialize;

// TODO: this should be in lightyear_serde? it's not strictly related to messages?
/// Stores function pointers related to serialization and deserialization
#[derive(Clone, Debug, PartialEq)]
pub struct ErasedSerializeFns {
    pub(crate) type_id: TypeId,
    pub type_name: &'static str,
    // TODO: maybe use `Vec<MaybeUninit<u8>>` instead of unsafe fn(), like bevy?
    pub serialize: unsafe fn(),
    pub erased_serialize: ErasedSerializeFn,
    pub deserialize: unsafe fn(),
    pub erased_clone: Option<unsafe fn()>,
    pub map_entities: Option<ErasedMapEntitiesFn>,
    pub send_map_entities: Option<ErasedSendMapEntitiesFn>,
    pub receive_map_entities: Option<ErasedReceiveMapEntitiesFn>,
}

/// Controls how a type (resources/components/messages) is serialized and deserialized
pub struct SerializeFns<M> {
    /// Called to serialize the type into the writer
    pub serialize: SerializeFn<M>,
    /// Called to deserialize the type from the reader
    pub deserialize: DeserializeFn<M>,
}

impl<M: Serialize + DeserializeOwned> Default for SerializeFns<M> {
    fn default() -> Self {
        Self {
            serialize: default_serialize::<M>,
            deserialize: default_deserialize::<M>,
        }
    }
}

impl<M: ToBytes> SerializeFns<M> {
    pub fn with_to_bytes() -> Self {
        Self {
            serialize: |message, writer| message.to_bytes(writer),
            deserialize: |reader| M::from_bytes(reader),
        }
    }
}

type ErasedSerializeFn = unsafe fn(
    erased_serialize_fn: &ErasedSerializeFns,
    message: Ptr,
    writer: &mut Writer,
    entity_map: &mut SendEntityMap,
) -> Result<(), SerializationError>;

/// Type of the serialize function without entity mapping
pub type SerializeFn<M> = fn(message: &M, writer: &mut Writer) -> Result<(), SerializationError>;

/// Type of the deserialize function without entity mapping
pub type DeserializeFn<M> = fn(reader: &mut Reader) -> Result<M, SerializationError>;

type CloneFn<M> = fn(&M) -> M;

/// Type of the entity mapping function
pub(crate) type ErasedMapEntitiesFn =
    for<'a> unsafe fn(message: PtrMut<'a>, entity_map: &mut EntityMap);
/// Type of the entity mapping function used for serialiaztion
pub(crate) type ErasedSendMapEntitiesFn =
    for<'a> unsafe fn(message: PtrMut<'a>, entity_map: &mut SendEntityMap);
/// Type of the entity mapping function used for deserialiaztion
pub(crate) type ErasedReceiveMapEntitiesFn =
    for<'a> unsafe fn(message: PtrMut<'a>, entity_map: &mut ReceiveEntityMap);

unsafe fn erased_serialize_fn<M: 'static>(
    erased_serialize_fn: &ErasedSerializeFns,
    message: Ptr,
    writer: &mut Writer,
    entity_map: &mut SendEntityMap,
) -> Result<(), SerializationError> {
    // SAFETY: the typed serialize functions are created for the message of type M
    let typed_serialize_fns = unsafe { erased_serialize_fn.typed::<M>() };
    if let Some(map_entities) = erased_serialize_fn.send_map_entities {
        // SAFETY: the Ptr was created for the message of type M
        let message = unsafe { message.deref::<M>() };
        let clone_fn: CloneFn<M> = unsafe{ core::mem::transmute(erased_serialize_fn.erased_clone.unwrap()) } ;
        let mut new_message = clone_fn(message);
        unsafe {
            map_entities(PtrMut::from(&mut new_message), entity_map);
        }
        (typed_serialize_fns.serialize)(&new_message, writer)
    } else {
        // SAFETY: the Ptr was created for the message of type M
        let message = unsafe { message.deref::<M>() };
        (typed_serialize_fns.serialize)(message, writer)
    }
}

#[cfg(feature = "std")]
/// Default serialize function using bincode
fn default_serialize<M: Serialize>(
    message: &M,
    buffer: &mut Writer,
) -> Result<(), SerializationError> {
    let _ = bincode::serde::encode_into_std_write(message, buffer, bincode::config::standard())?;
    Ok(())
}


#[cfg(not(feature = "std"))]
/// Default serialize function using bincode
fn default_serialize<M: Message + Serialize>(
    message: &M,
    buffer: &mut Writer,
) -> Result<(), SerializationError> {
    let _ = bincode::serde::encode_into_writer(message, buffer, bincode::config::standard())?;
    Ok(())
}

#[cfg(feature = "std")]
/// Default deserialize function using bincode
fn default_deserialize<M: DeserializeOwned>(
    buffer: &mut Reader,
) -> Result<M, SerializationError> {
    let data = bincode::serde::decode_from_std_read(buffer, bincode::config::standard())?;
    Ok(data)
}

#[cfg(not(feature = "std"))]
/// Default deserialize function using bincode
fn default_deserialize<M: Message + DeserializeOwned>(
    buffer: &mut Reader,
) -> Result<M, SerializationError> {
    let data = bincode::serde::decode_from_reader(buffer, bincode::config::standard())?;
    Ok(data)
}

fn erased_clone<M: Clone>(message: &M) -> M {
    message.clone()
}

/// SAFETY: the PtrMut must be a valid pointer to a value of type M
unsafe fn erased_map_entities<M: MapEntities + 'static>(
    message: PtrMut,
    entity_map: &mut EntityMap,
) {
    // SAFETY: the PtrMut must be a valid pointer to a value of type M
    let message = unsafe { message.deref_mut::<M>() };
    M::map_entities(message, entity_map);
}

/// SAFETY: the PtrMut must be a valid pointer to a value of type M
unsafe fn erased_send_map_entities<M: MapEntities + 'static>(
    message: PtrMut,
    entity_map: &mut SendEntityMap,
) {
    // SAFETY: the PtrMut must be a valid pointer to a value of type M
    let message = unsafe { message.deref_mut::<M>() };
    M::map_entities(message, entity_map);
}

/// SAFETY: the PtrMut must be a valid pointer to a value of type M
unsafe fn erased_receive_map_entities<M: MapEntities + 'static>(
    message: PtrMut,
    entity_map: &mut ReceiveEntityMap,
) {
    // SAFETY: the PtrMut must be a valid pointer to a value of type M
    let message = unsafe { message.deref_mut::<M>() };
    M::map_entities(message, entity_map);
}

impl ErasedSerializeFns {
    pub fn new<M: Serialize + DeserializeOwned + 'static>() -> Self {
        Self::new_custom_serde(SerializeFns::<M>::default())
    }

    pub fn new_custom_serde<M: 'static>(serialize_fns: SerializeFns<M>) -> Self {
        Self {
            type_id: TypeId::of::<M>(),
            type_name: core::any::type_name::<M>(),
            erased_serialize: erased_serialize_fn::<M>,
            serialize: unsafe { core::mem::transmute(serialize_fns.serialize) },
            deserialize: unsafe { core::mem::transmute(serialize_fns.deserialize) },
            erased_clone: None,
            map_entities: None,
            send_map_entities: None,
            receive_map_entities: None,
        }
    }

    pub unsafe fn typed<M: 'static>(&self) -> SerializeFns<M> {
        debug_assert_eq!(
            self.type_id,
            TypeId::of::<M>(),
            "The erased message fns were created for type {}, but we are trying to convert to type {}",
            self.type_name,
            core::any::type_name::<M>(),
        );
        SerializeFns {
            serialize: unsafe { core::mem::transmute(self.serialize) },
            deserialize: unsafe { core::mem::transmute(self.deserialize) },
        }
    }

    // We need to be able to clone the data, because when serialize we:
    // - clone the data
    // - map the entities
    // - serialize the cloned data
    // Note that this is fairly inefficient because in most cases (when there is no authority transfer)
    // there is no entity mapping done on the serialization side, just on the deserialization side.
    // However, components that contain other entities should be small in general.
    pub fn add_map_entities<M: Clone + MapEntities + 'static>(&mut self) {
        self.map_entities = Some(erased_map_entities::<M>);
        self.send_map_entities = Some(erased_send_map_entities::<M>);
        self.receive_map_entities = Some(erased_receive_map_entities::<M>);
        let clone_fn: fn(&M) -> M = erased_clone::<M>;
        self.erased_clone = Some(unsafe { core::mem::transmute(clone_fn) });
    }

    pub fn map_entities<M: 'static>(&self, message: &mut M, entity_map: &mut EntityMap) {
        let ptr = PtrMut::from(message);
        if let Some(map_entities_fn) = self.map_entities {
            unsafe {
                map_entities_fn(ptr, entity_map);
            }
        }
    }

    /// Serialize the message into the writer.
    /// If available, we try to map the entities in the message from local to remote.
    ///
    /// SAFETY: the ErasedSerializeFns must be created for the type M
    pub unsafe fn serialize<M: 'static>(
        &self,
        message: &M,
        writer: &mut Writer,
        entity_map: &mut SendEntityMap,
    ) -> Result<(), SerializationError> {
        // SAFETY: the Ptr must be a valid pointer to a value of type M
        unsafe { erased_serialize_fn::<M>(self, Ptr::from(message), writer, entity_map) }
    }

    /// Deserialize the message value from the reader
    ///
    /// SAFETY: the ErasedSerializeFns must be created for the type M
    pub unsafe fn deserialize<M: 'static>(
        &self,
        reader: &mut Reader,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<M, SerializationError> {
        let fns = unsafe { self.typed::<M>() };
        let mut message = (fns.deserialize)(reader)?;
        if let Some(map_entities) = self.receive_map_entities {
            // SAFETY: the PtrMut must be a valid pointer to a value of type M
            unsafe { map_entities(PtrMut::from(&mut message), entity_map); }
        }
        Ok(message)
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::{Entity, Reflect, Resource};
    use bevy::ptr::Ptr;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect, MapEntities)]
    pub struct EntityMessage(#[entities] pub Entity);

    #[derive(Resource, Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
    pub struct Message1(pub f32);

    /// Test serializing/deserializing using the ErasedSerializeFns
    #[test]
    fn test_erased_serde() {
        let mut registry = ErasedSerializeFns::new::<EntityMessage>();
        registry.add_map_entities::<EntityMessage>();

        let message = EntityMessage(Entity::from_raw(1));
        let mut writer = Writer::default();
        let _ = unsafe {
            erased_serialize_fn::<EntityMessage>(
                &registry,
                Ptr::from(&message),
                &mut writer,
                &mut SendEntityMap::default(),
            )
        };

        let data = writer.to_bytes();
        let mut reader = Reader::from(data);
        let new_message = unsafe {
            registry.deserialize::<EntityMessage>(&mut reader, &mut ReceiveEntityMap::default())
        }
        .unwrap();
        // we deserialize the entity as a placeholder when we don't find it in the ReceiveEntityMap
        assert_eq!(
            new_message,
            EntityMessage(Entity::PLACEHOLDER)
        );
    }

    /// Test serializing/deserializing using the ErasedSerializeFns and applying entity mapping
    /// We do the mapping on the send size
    #[test]
    fn test_erased_serde_map_entities() {
        let mut registry = ErasedSerializeFns::new::<EntityMessage>();
        registry.add_map_entities::<EntityMessage>();

        let message = EntityMessage(Entity::from_raw(1));
        let mut writer = Writer::default();
        let mut entity_map = SendEntityMap::default();
        entity_map.insert(Entity::from_raw(1), Entity::from_raw(2));
        let _ = unsafe {
            erased_serialize_fn::<EntityMessage>(
                &registry,
                Ptr::from(&message),
                &mut writer,
                &mut entity_map,
            )
        };

        let data = writer.to_bytes();
        let mut reader = Reader::from(data);
        let new_message = unsafe {
            registry.deserialize::<EntityMessage>(&mut reader, &mut ReceiveEntityMap::default())
        }
        .unwrap();
        assert_eq!(
            new_message,
            EntityMessage(Entity::from_raw(2))
        );
    }
}
