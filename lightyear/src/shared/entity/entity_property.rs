use std::error::Error;
use bevy_ecs::entity::Entity;

use lightyear_serde::{BitReader, BitWrite, BitWriter, Serde, SerdeErr};

use crate::shared::{
    entity::net_entity::NetEntity,
};

#[derive(Clone)]
pub struct EntityProperty {
    handle_prop: Property<Option<NetEntity>>,
}

impl EntityProperty {
    pub fn new(mutator_index: u8) -> Self {
        Self {
            handle_prop: Property::<Option<NetEntity>>::new(None, mutator_index),
        }
    }

    pub fn new_empty() -> Self {
        Self {
            handle_prop: Property::<Option<NetEntity>>::new(None, 0),
        }
    }

    pub fn mirror(&mut self, other: &EntityProperty) {
        *self.handle_prop = other.handle();
    }

    pub fn handle(&self) -> Option<NetEntity> {
        *self.handle_prop
    }

    // Serialization / deserialization

    pub fn write(&self, writer: &mut dyn BitWrite) {
        (*self.handle_prop).ser(writer);
    }

    pub fn new_read(
        reader: &mut BitReader,
        mutator_index: u8,
    ) -> Result<Self, SerdeErr> {
        let mut new_prop = Self::new(mutator_index);
        if let Some(net_entity) = Option::<NetEntity>::de(reader)? {
            *new_prop.handle_prop = Some(net_entity);
        } else {
            *new_prop.handle_prop = None;
        }
        Ok(new_prop)
    }

    pub fn read_write(reader: &mut BitReader, writer: &mut BitWriter) -> Result<(), SerdeErr> {
        Option::<NetEntity>::de(reader)?.ser(writer);
        Ok(())
    }

    pub fn read(
        &mut self,
        reader: &mut BitReader,
    ) -> Result<(), SerdeErr> {
        if let Some(net_entity) = Option::<NetEntity>::de(reader)? {
            *self.handle_prop = Some(net_entity);
        } else {
            *self.handle_prop = None;
        }
        Ok(())
    }

    // Comparison

    pub fn equals(&self, other: &EntityProperty) -> bool {
        if let Some(handle) = *self.handle_prop {
            if let Some(other_handle) = *other.handle_prop {
                return handle == other_handle;
            }
            return false;
        }
        other.handle_prop.is_none()
    }

    // Internal

    pub fn set_mutator(&mut self, mutator: &PropertyMutator) {
        self.handle_prop.set_mutator(mutator);
    }

    pub fn get(&self, converter: &dyn NetEntityConverter) -> Option<Entity> {
        *self.handle_prop
    }

    pub fn set(&mut self, entity: Entity, converter: &dyn NetEntityConverter) {
        *self.handle_prop = Some(entity);
    }
}


// TODO: move to net_entity.rs
pub trait NetEntityConverter {
    fn entity_to_net_entity(&self, entity: &Entity) -> NetEntity;
    fn net_entity_to_entity(&self, net_entity: &NetEntity) -> Entity;
}

pub struct FakeEntityConverter;

impl NetEntityConverter for FakeEntityConverter {
    fn entity_to_net_entity(&self, _: &Entity) -> NetEntity {
        NetEntity::from(0)
    }

    fn net_entity_to_entity(&self, _: &NetEntity) -> Result<Entity, EntityDoesNotExistError> {
        Ok(Entity::from_raw(0))
    }
}

pub struct EntityConverter<'b> {
    net_entity_converter: &'b dyn NetEntityConverter,
}

impl<'b> EntityConverter<'b> {
    pub fn new(
        net_entity_converter: &'b dyn NetEntityConverter,
    ) -> Self {
        Self {
            net_entity_converter,
        }
    }
}


#[derive(Debug)]
pub struct EntityDoesNotExistError;
impl Error for EntityDoesNotExistError {}
impl std::fmt::Display for EntityDoesNotExistError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        write!(f, "Error while attempting to look-up an Entity value for conversion: Entity was not found!")
    }
}
