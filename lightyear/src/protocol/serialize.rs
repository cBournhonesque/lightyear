use crate::prelude::{ComponentRegistry, Message, MessageRegistry};
use crate::serialize::{reader::Reader, writer::Writer, SerializationError};
use crate::shared::replication::entity_map::{EntityMap, ReceiveEntityMap, SendEntityMap};
use bevy::app::App;
use bevy::ecs::entity::MapEntities;
use bevy::ptr::{Ptr, PtrMut};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::any::TypeId;

/// Stores function pointers related to serialization and deserialization
#[derive(Clone, Debug, PartialEq)]
pub struct ErasedSerializeFns {
    pub(crate) type_id: TypeId,
    pub(crate) type_name: &'static str,
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
    /// Called to serialize the type into the writer with entity mapping.
    ///
    /// Can be set to [`None`] if entity mapping is not required, but *will*
    /// be assumed to exist and be called if mapping is enabled.
    pub serialize_map_entities: Option<SerializeMapEntitiesFn<M>>,
}

type ErasedSerializeFn = unsafe fn(
    erased_serialize_fn: &ErasedSerializeFns,
    message: Ptr,
    writer: &mut Writer,
    entity_map: Option<&mut SendEntityMap>,
) -> Result<(), SerializationError>;

/// Type of the serialize function without entity mapping
type SerializeFn<M> = fn(message: &M, writer: &mut Writer) -> Result<(), SerializationError>;
/// Type of the deserialize function without entity mapping
type DeserializeFn<M> = fn(reader: &mut Reader) -> Result<M, SerializationError>;

type CloneFn<M> = fn(&M) -> M;

type SerializeMapEntitiesFn<M> = fn(
    &M,
    &mut Writer,
    &mut SendEntityMap,
    CloneFn<M>,
    ErasedSendMapEntitiesFn,
    SerializeFn<M>,
) -> Result<(), SerializationError>;

/// Type of the entity mapping function
pub(crate) type ErasedMapEntitiesFn =
    for<'a> unsafe fn(message: PtrMut<'a>, entity_map: &mut EntityMap);
/// Type of the entity mapping function used for serialiaztion
pub(crate) type ErasedSendMapEntitiesFn =
    for<'a> unsafe fn(message: PtrMut<'a>, entity_map: &mut SendEntityMap);
/// Type of the entity mapping function used for deserialiaztion
pub(crate) type ErasedReceiveMapEntitiesFn =
    for<'a> unsafe fn(message: PtrMut<'a>, entity_map: &mut ReceiveEntityMap);

unsafe fn erased_serialize_fn<M: Message>(
    erased_serialize_fn: &ErasedSerializeFns,
    message: Ptr,
    writer: &mut Writer,
    entity_map: Option<&mut SendEntityMap>,
) -> Result<(), SerializationError> {
    let typed_serialize_fns = erased_serialize_fn.typed::<M>();
    if let Some(map_entities) = erased_serialize_fn.send_map_entities {
        let serialize_map_entities = typed_serialize_fns.serialize_map_entities.unwrap();
        serialize_map_entities(
            message.deref::<M>(),
            writer,
            entity_map.expect("EntityMap is required to serialize this message"),
            std::mem::transmute(erased_serialize_fn.erased_clone.unwrap()),
            map_entities,
            typed_serialize_fns.serialize,
        )
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

pub(crate) fn serialize_map_entities<M>(
    message: &M,
    writer: &mut Writer,
    entity_map: &mut SendEntityMap,
    clone_fn: CloneFn<M>,
    erased_map_entities_fn: ErasedSendMapEntitiesFn,
    serialize_fn: SerializeFn<M>,
) -> Result<(), SerializationError> {
    let mut new_message = clone_fn(message);
    unsafe {
        erased_map_entities_fn(PtrMut::from(&mut new_message), entity_map);
    }
    serialize_fn(&new_message, writer)
}

fn erased_clone<M: Clone>(message: &M) -> M {
    message.clone()
}

/// SAFETY: the PtrMut must be a valid pointer to a value of type M
unsafe fn erased_map_entities<M: MapEntities + 'static>(
    message: PtrMut,
    entity_map: &mut EntityMap,
) {
    let message = message.deref_mut::<M>();
    M::map_entities(message, entity_map);
}

/// SAFETY: the PtrMut must be a valid pointer to a value of type M
unsafe fn erased_send_map_entities<M: MapEntities + 'static>(
    message: PtrMut,
    entity_map: &mut SendEntityMap,
) {
    let message = message.deref_mut::<M>();
    M::map_entities(message, entity_map);
}

/// SAFETY: the PtrMut must be a valid pointer to a value of type M
unsafe fn erased_receive_map_entities<M: MapEntities + 'static>(
    message: PtrMut,
    entity_map: &mut ReceiveEntityMap,
) {
    let message = message.deref_mut::<M>();
    M::map_entities(message, entity_map);
}

impl ErasedSerializeFns {
    pub(crate) fn new<M: Message + Serialize + DeserializeOwned>() -> Self {
        let serialize_fns = SerializeFns {
            serialize: default_serialize::<M>,
            deserialize: default_deserialize::<M>,
            serialize_map_entities: None,
        };
        Self {
            type_id: TypeId::of::<M>(),
            type_name: std::any::type_name::<M>(),
            erased_serialize: erased_serialize_fn::<M>,
            serialize: unsafe { std::mem::transmute(serialize_fns.serialize) },
            deserialize: unsafe { std::mem::transmute(serialize_fns.deserialize) },
            erased_clone: None,
            map_entities: None,
            send_map_entities: None,
            receive_map_entities: None,
        }
    }

    pub(crate) fn new_custom_serde<M: Message>(serialize_fns: SerializeFns<M>) -> Self {
        Self {
            type_id: TypeId::of::<M>(),
            type_name: std::any::type_name::<M>(),
            erased_serialize: erased_serialize_fn::<M>,
            serialize: unsafe { std::mem::transmute(serialize_fns.serialize) },
            deserialize: unsafe { std::mem::transmute(serialize_fns.deserialize) },
            erased_clone: None,
            map_entities: None,
            send_map_entities: None,
            receive_map_entities: None,
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
        let serialize_map_entities: fn(
            &M,
            &mut Writer,
            &mut SendEntityMap,
            CloneFn<M>,
            ErasedSendMapEntitiesFn,
            SerializeFn<M>,
        ) -> Result<(), SerializationError> = serialize_map_entities::<M>;
        SerializeFns {
            serialize: unsafe { std::mem::transmute(self.serialize) },
            deserialize: unsafe { std::mem::transmute(self.deserialize) },
            serialize_map_entities: Some(unsafe { std::mem::transmute(serialize_map_entities) }),
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
        self.send_map_entities = Some(erased_send_map_entities::<M>);
        self.receive_map_entities = Some(erased_receive_map_entities::<M>);
        let clone_fn: fn(&M) -> M = erased_clone::<M>;
        self.erased_clone = Some(unsafe { std::mem::transmute(clone_fn) });
    }

    pub(crate) fn map_entities<M: 'static>(&self, message: &mut M, entity_map: &mut EntityMap) {
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
    pub(crate) unsafe fn serialize<M: 'static>(
        &self,
        message: &M,
        writer: &mut Writer,
        entity_map: Option<&mut SendEntityMap>,
    ) -> Result<(), SerializationError> {
        let fns = unsafe { self.typed::<M>() };
        if let Some(map_entities) = self.send_map_entities {
            let serialize_map_entities = fns.serialize_map_entities.unwrap();
            serialize_map_entities(
                message,
                writer,
                entity_map.expect("EntityMap is required to serialize this message"),
                std::mem::transmute(self.erased_clone.unwrap()),
                map_entities,
                fns.serialize,
            )
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
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<M, SerializationError> {
        let fns = unsafe { self.typed::<M>() };
        let mut message = (fns.deserialize)(reader)?;
        if let Some(map_entities) = self.receive_map_entities {
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

#[cfg(test)]
mod tests {
    use crate::protocol::serialize::{erased_serialize_fn, ErasedSerializeFns};
    use crate::serialize::reader::Reader;
    use crate::serialize::writer::Writer;
    use crate::shared::replication::authority::AuthorityChange;
    use crate::shared::replication::entity_map::{ReceiveEntityMap, SendEntityMap};
    use bevy::prelude::Entity;
    use bevy::ptr::Ptr;

    #[test]
    fn test_erased_serde() {
        let mut registry = ErasedSerializeFns::new::<AuthorityChange>();
        registry.add_map_entities::<AuthorityChange>();

        let message = AuthorityChange {
            entity: Entity::from_raw(1),
            gain_authority: true,
        };
        let mut writer = Writer::default();
        let _ = unsafe {
            erased_serialize_fn::<AuthorityChange>(
                &registry,
                Ptr::from(&message),
                &mut writer,
                Some(&mut SendEntityMap::default()),
            )
        };

        let data = writer.to_bytes();
        let mut reader = Reader::from(data);
        let new_message = unsafe {
            registry.deserialize::<AuthorityChange>(&mut reader, &mut ReceiveEntityMap::default())
        }
        .unwrap();
        assert_eq!(new_message, message);
    }

    #[test]
    fn test_erased_serde_map_entities() {
        let mut registry = ErasedSerializeFns::new::<AuthorityChange>();
        registry.add_map_entities::<AuthorityChange>();

        let message = AuthorityChange {
            entity: Entity::from_raw(1),
            gain_authority: true,
        };
        let mut writer = Writer::default();
        let mut entity_map = SendEntityMap::default();
        entity_map.insert(Entity::from_raw(1), Entity::from_raw(2));
        let _ = unsafe {
            erased_serialize_fn::<AuthorityChange>(
                &registry,
                Ptr::from(&message),
                &mut writer,
                Some(&mut entity_map),
            )
        };

        let data = writer.to_bytes();
        let mut reader = Reader::from(data);
        let new_message = unsafe {
            registry.deserialize::<AuthorityChange>(&mut reader, &mut ReceiveEntityMap::default())
        }
        .unwrap();
        assert_eq!(
            new_message,
            AuthorityChange {
                entity: Entity::from_raw(2),
                gain_authority: true,
            }
        );
    }
}
