use std::error::Error;
use bevy_ecs::entity::Entity;

use lightyear_serde::{BitReader, BitWrite, BitWriter, Serde, SerdeErr};

use crate::shared::{
    component::{property::Property, property_mutate::PropertyMutator},
    entity::net_entity::NetEntity,
};

#[derive(Clone)]
pub struct EntityProperty {
    handle_prop: Property<Option<Entity>>,
}

impl EntityProperty {
    pub fn new(mutator_index: u8) -> Self {
        Self {
            handle_prop: Property::<Option<Entity>>::new(None, mutator_index),
        }
    }

    pub fn new_empty() -> Self {
        Self {
            handle_prop: Property::<Option<Entity>>::new(None, 0),
        }
    }

    pub fn mirror(&mut self, other: &EntityProperty) {
        *self.handle_prop = other.handle();
    }

    pub fn handle(&self) -> Option<Entity> {
        *self.handle_prop
    }

    // Serialization / deserialization

    pub fn write(&self, writer: &mut dyn BitWrite, converter: &dyn NetEntityConverter) {
        (*self.handle_prop)
            .map(|handle| converter.handle_to_net_entity(&handle))
            .ser(writer);
    }

    pub fn new_read(
        reader: &mut BitReader,
        mutator_index: u8,
        converter: &dyn NetEntityConverter,
    ) -> Result<Self, SerdeErr> {
        if let Some(net_entity) = Option::<NetEntity>::de(reader)? {
            if let Ok(handle) = converter.net_entity_to_handle(&net_entity) {
                let mut new_prop = Self::new(mutator_index);
                *new_prop.handle_prop = Some(handle);
                Ok(new_prop)
            } else {
                panic!("Could not find Entity to associate with incoming EntityProperty value!");
            }
        } else {
            let mut new_prop = Self::new(mutator_index);
            *new_prop.handle_prop = None;
            Ok(new_prop)
        }
    }

    pub fn read_write(reader: &mut BitReader, writer: &mut BitWriter) -> Result<(), SerdeErr> {
        Option::<NetEntity>::de(reader)?.ser(writer);
        Ok(())
    }

    pub fn read(
        &mut self,
        reader: &mut BitReader,
        converter: &dyn NetEntityConverter,
    ) -> Result<(), SerdeErr> {
        if let Some(net_entity) = Option::<NetEntity>::de(reader)? {
            if let Ok(handle) = converter.net_entity_to_handle(&net_entity) {
                *self.handle_prop = Some(handle);
            } else {
                panic!("Could not find Entity to associate with incoming EntityProperty value!");
            }
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

    pub fn get(&self) -> Option<Entity> {
        *self.handle_prop
    }

    pub fn set(&mut self, entity: Entity) {
        *self.handle_prop = Some(entity);
    }
}


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
