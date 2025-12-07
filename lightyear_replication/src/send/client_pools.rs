use alloc::vec::Vec;
use core::ops::Range;

use crate::messages::updates::EntityUpdates;
use crate::registry::component_mask::ComponentMask;
use crate::registry::registry::ComponentIndex;
use bevy_ecs::prelude::*;

/// Pools for various client components to reuse allocated capacity.
///
/// All data is cleared before the insertion.
#[derive(Resource, Default)]
pub(crate) struct ClientPools {
    /// Entities with bitvecs for components
    entities: Vec<Vec<(Entity, ComponentMask)>>,
    /// Bitvecs for components
    ///
    /// Only heap-allocated instances are stored.
    components: Vec<ComponentMask>,
    /// Ranges from [`Updates`] and [`Actions`].
    ranges: Vec<Vec<Range<usize>>>,
    /// List of components removals from [`Actions`]
    removals: Vec<Vec<ComponentIndex>>,
    /// Entities from [`Actions`].
    mutations: Vec<Vec<EntityUpdates>>,
}

impl ClientPools {
    pub(crate) fn recycle_entities(&mut self, mut entities: Vec<(Entity, ComponentMask)>) {
        for (_, components) in entities.drain(..) {
            self.recycle_components(components);
        }
        self.entities.push(entities);
    }

    pub(crate) fn recycle_components(&mut self, mut components: ComponentMask) {
        if components.is_heap() {
            components.clear();
            self.components.push(components);
        }
    }

    pub(crate) fn recycle_removals(&mut self, removals: impl Iterator<Item = Vec<ComponentIndex>>) {
        self.removals.extend(removals.map(|mut removals| {
            removals.clear();
            removals
        }));
    }

    pub(crate) fn recycle_ranges(&mut self, ranges: impl Iterator<Item = Vec<Range<usize>>>) {
        self.ranges.extend(ranges.map(|mut ranges| {
            ranges.clear();
            ranges
        }));
    }

    pub(crate) fn recycle_mutations(
        &mut self,
        mutations: impl Iterator<Item = Vec<EntityUpdates>>,
    ) {
        self.mutations.extend(mutations.map(|mut mutations| {
            mutations.clear();
            mutations
        }));
    }

    pub(crate) fn take_entities(&mut self) -> Vec<(Entity, ComponentMask)> {
        self.entities.pop().unwrap_or_default()
    }

    pub(crate) fn take_components(&mut self) -> ComponentMask {
        self.components.pop().unwrap_or_default()
    }

    pub(crate) fn take_removals(&mut self) -> Vec<ComponentIndex> {
        self.removals.pop().unwrap_or_default()
    }

    pub(crate) fn take_ranges(&mut self) -> Vec<Range<usize>> {
        self.ranges.pop().unwrap_or_default()
    }

    pub(crate) fn take_mutations(&mut self) -> Vec<EntityUpdates> {
        self.mutations.pop().unwrap_or_default()
    }
}
