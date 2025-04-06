//! An IndexMap wrapper for storing unique entities.
//!
//! Provides fast iteration, insertion, and removal of entities.

use bevy::ecs::entity::{EntitySet, EntitySetIterator};
use bevy::platform_support::hash::FixedHasher;
use bevy::prelude::Entity;

pub struct UniqueEntities(indexmap::IndexSet<Entity, FixedHasher>);


impl EntitySet for UniqueEntities {}
unsafe impl EntitySetIterator for UniqueEntitiesIterator {}

struct UniqueEntitiesIterator<'a> {
   iter: indexmap::set::Iter<'a, Entity>,
}

impl UniqueEntities {
    pub fn new() -> Self {
        UniqueEntities(indexmap::IndexSet::with_hasher(FixedHasher))
    }

    pub fn insert(&mut self, entity: Entity) {
        self.0.insert(entity);
    }

    pub fn remove(&mut self, entity: &Entity) -> bool {
        self.0.remove(entity)
    }

    pub fn contains(&self, entity: &Entity) -> bool {
        self.0.contains(entity)
    }

    pub fn iter(&self) -> impl Iterator<Item=Entity> {
        UniqueEntitiesIterator {
            iter: self.0.iter(),
        }
    }
}

impl IntoIterator for UniqueEntities {
    type Item = Entity;
    type IntoIter = UniqueEntitiesIterator<'a>;

    fn into_iter(self) -> Self::IntoIter {
        UniqueEntitiesIterator {
            iter: self.0.into_iter(),
        }
    }
}