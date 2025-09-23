use crate::Deterministic;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use bevy_ecs::archetype::{Archetype, ArchetypeGeneration, ArchetypeId};
use bevy_ecs::component::{ComponentId, StorageType, Tick};
use bevy_ecs::prelude::World;
use bevy_ecs::query::{FilteredAccess, FilteredAccessSet};
use bevy_ecs::system::{ReadOnlySystemParam, SystemMeta, SystemParam};
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use lightyear_prediction::prelude::PredictionRegistry;
use lightyear_prediction::registry::PopUntilTickAndHashFn;
use lightyear_prediction::rollback::DisableRollback;
use lightyear_replication::prelude::ComponentRegistry;
use lightyear_replication::registry::deterministic::DeterministicFns;
use tracing::trace;

/// A [`SystemParam`] that holds the list of archetypes in the world that should be hashed
/// for checksum calculation.
///
/// Only entities with the [`Deterministic`] marker component are considered, and we will
/// only iterate through their components that have a hash function registered.
///
/// HISTORY: if True, the archetypes will contain the [`PredictionHistory<C>`](lightyear_prediction::prelude::PredictionHistory) instead of C.
/// THis is useful on the client-side where we want the checksum to use the history value at the LastConfirmedTick.
pub(crate) struct ChecksumWorld<'w, 's, const HISTORY: bool> {
    pub(crate) world: UnsafeWorldCell<'w>,
    pub(crate) state: &'s mut ChecksumState,
}

impl<'w, const HISTORY: bool> ChecksumWorld<'w, '_, HISTORY> {
     /// Go through new archetypes in the world and cache the ones that should be included as [`ChecksumArchetype`]
     pub(crate) fn update_archetypes(&mut self) {
         let archetypes = self.world.archetypes();
         let old_generation =
             core::mem::replace(&mut self.state.archetype_generation, archetypes.generation());

         for archetype in &archetypes[old_generation..] {
             let mut checksum_archetype = ChecksumArchetype::new(archetype.id());
             self.state.hash_fns.keys().for_each(|component_id| {
                    if archetype.contains(*component_id) {
                        trace!("found component {:?} in archetype", component_id);
                        // SAFETY: archetype contains this component.
                        let storage =
                            unsafe { archetype.get_storage_type(*component_id).unwrap_unchecked() };
                        checksum_archetype.components.push((*component_id, storage));
                    }
            });
            // Store for future iteration.
            self.state.archetypes.push(checksum_archetype);
         }
     }

    /// Return iterator over checksum archetypes.
    ///
    /// Safety: `Self::update_archetypes` must be called before calling this function to ensure the archetypes are up to date.
    pub(super) unsafe fn iter_archetypes(&self) -> impl Iterator<Item = (&Archetype, &ChecksumArchetype)> {
        self.state.archetypes.iter().map(|checksum_archetype| {
            // SAFETY: the id is valid because it was obtained from an existing archetype in `new_archetype`.
            let archetype = unsafe {
                self.world
                    .archetypes()
                    .get(checksum_archetype.id)
                    .unwrap_unchecked()
            };

            (archetype, checksum_archetype)
        })
    }
}

unsafe impl<const HISTORY: bool> SystemParam for ChecksumWorld<'_, '_, HISTORY> {
    type State = ChecksumState;
    type Item<'world, 'state> = ChecksumWorld<'world, 'state, HISTORY>;

    fn init_state(world: &mut World) -> Self::State {
        let marker_id = world.register_component::<Deterministic>();
        let disable_rollback_id = world.register_component::<DisableRollback>();
        let registry = world.resource::<ComponentRegistry>();
        let hash_fns = if !HISTORY {
            let registry = world.resource::<ComponentRegistry>();
            registry
            .component_metadata_map
            .values()
            .filter_map(| m| m.deterministic
                .as_ref()
                .map(|d| {
                    (m.component_id, (*d, None))
                }))
            .collect()
        } else {
            let prediction_registry = world.resource::<PredictionRegistry>();
            prediction_registry
                .prediction_map
                .iter()
                .filter_map(|(kind, pred)| {
                    // TODO: for non-full components, just fetch the component value directly
                    let history_id = pred.history_id?;
                    registry.component_metadata_map
                        .get(kind)
                        .and_then(|m| m.deterministic.as_ref().map(|d| (history_id, (*d, pred.pop_until_tick_and_hash()))))
                })
                .collect()
        };
        trace!("HashFns used for ChecksumState: {:?}", hash_fns);
        ChecksumState {
            marker_id,
            disable_rollback_id,
            archetypes: Default::default(),
            hash_fns,
            archetype_generation: ArchetypeGeneration::initial(),
        }
    }

    fn init_access(
        state: &Self::State,
        system_meta: &mut SystemMeta,
        component_access_set: &mut FilteredAccessSet,
        world: &mut World,
    ) {
        let mut filtered_access = FilteredAccess::default();
        filtered_access.add_component_read(state.marker_id);
        // Exclude entities with DisableRollback from the checksum calculation since they won't have a PredictionHistory?
        if HISTORY {
            filtered_access.and_without(state.disable_rollback_id);
        }
        let combined_access = component_access_set.combined_access();
        state.hash_fns.iter().for_each(|(component_id, (_, pop_fn))| {
            if pop_fn.is_some() {
                // the component is a PredictionHistory
                // TODO: for non-full components, just fetch the component value directly
                // We need write access because we will call `pop_until_tick` on the history component
                filtered_access.add_component_write(*component_id);
                assert!(
                    !combined_access.has_component_read(*component_id),
                    "replicated component `{}` in system `{}` shouldn't be in conflict with other system parameters",
                    world.components().get_name(*component_id).unwrap(),
                    system_meta.name(),
                );
            } else {
                filtered_access.add_component_read(*component_id);
                assert!(
                    !combined_access.has_component_write(*component_id),
                    "replicated component `{}` in system `{}` shouldn't be in conflict with other system parameters",
                    world.components().get_name(*component_id).unwrap(),
                    system_meta.name(),
                );
            }
        });
        // SAFETY: used only to extend access.
        component_access_set.add(filtered_access);
    }

    unsafe fn get_param<'world, 'state>(
        state: &'state mut Self::State,
        _system_meta: &SystemMeta,
        world: UnsafeWorldCell<'world>,
        _change_tick: Tick,
    ) -> Self::Item<'world, 'state> {
        ChecksumWorld { world, state }
    }
}

unsafe impl<const HISTORY: bool> ReadOnlySystemParam for ChecksumWorld<'_, '_, HISTORY> {}

pub(crate) struct ChecksumState {
    /// ComponentId for the `Deterministic` marker component.
    pub(crate) marker_id: ComponentId,
    /// ComponentId for the `DisableRollback` marker component.
    ///
    /// We will not compute the checksum for entities that have this component, as it could mess up the rollback logic
    /// since we are removing elements from the history.
    pub(crate) disable_rollback_id: ComponentId,
    pub(crate) archetypes: Vec<ChecksumArchetype>,
    pub(crate) hash_fns: BTreeMap<ComponentId, (DeterministicFns, Option<PopUntilTickAndHashFn>)>,
    pub(crate) archetype_generation: ArchetypeGeneration,
}

pub(crate) struct ChecksumArchetype {
    /// The ID of the archetype.
    pub(crate) id: ArchetypeId,
    /// Components in this archetype that have a hash function registered.
    pub(crate) components: Vec<(ComponentId, StorageType)>,
}

impl ChecksumArchetype {
    fn new(id: ArchetypeId) -> Self {
        Self {
            id,
            components: Default::default(),
        }
    }
}
