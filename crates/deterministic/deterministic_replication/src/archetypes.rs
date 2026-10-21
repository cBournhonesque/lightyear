use crate::Deterministic;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use bevy_ecs::archetype::{Archetype, ArchetypeGeneration, ArchetypeId};
use bevy_ecs::change_detection::Tick;
use bevy_ecs::component::{ComponentId, StorageType};
use bevy_ecs::prelude::World;
use bevy_ecs::query::FilteredAccess;
use bevy_ecs::system::{
    ReadOnlySystemParam, SystemAccess, SystemMeta, SystemParam, SystemParamValidationError,
};
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use lightyear_prediction::prelude::PredictionRegistry;
use lightyear_prediction::registry::PopUntilTickAndHashFn;
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
    ///
    /// Only archetypes that carry the [`Deterministic`] marker component are
    /// considered. Using the marker — rather than "archetype happens to
    /// contain a component with a deterministic hash fn" — keeps the server
    /// and client hashing the same *set* of entities. Avian requires
    /// `Position` on every `RigidBody`, so without this filter a peer would
    /// silently hash walls only on the side where they end up in an
    /// archetype that qualifies, producing a constant XOR delta in the
    /// checksum.
    pub(crate) fn update_archetypes(&mut self) {
        let archetypes = self.world.archetypes();
        let old_generation = core::mem::replace(
            &mut self.state.archetype_generation,
            archetypes.generation(),
        );
        let marker_id = self.state.marker_id;

        for archetype in &archetypes[old_generation..] {
            if !archetype.contains(marker_id) {
                continue;
            }
            // NOTE: do NOT filter by `DisableRollback`. That marker is
            // inserted on freshly-spawned `DeterministicPredicted`
            // entities during the first rollback that reaches back
            // before their spawn tick; it means "rollback must not
            // rewrite this entity's state", not "do not hash this
            // entity". The server (no rollback) never inserts it, so
            // gating on it only on the client would desync the set of
            // hashed entities between peers. `pop_until_tick` returning
            // `None` is the right guard for "no history yet at this
            // tick", and it's hit naturally on freshly-spawned entities.
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
    pub(super) unsafe fn iter_archetypes(
        &self,
    ) -> impl Iterator<Item = (&Archetype, &ChecksumArchetype)> {
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
        let registry = world.resource::<ComponentRegistry>();
        let hash_fns = if !HISTORY {
            let registry = world.resource::<ComponentRegistry>();
            registry
                .component_metadata_map
                .values()
                .filter_map(|m| {
                    m.deterministic
                        .as_ref()
                        .map(|d| (m.component_id, (*d, None)))
                })
                .collect()
        } else {
            let prediction_registry = world.resource::<PredictionRegistry>();
            prediction_registry
                .prediction_map
                .iter()
                .filter_map(|(kind, pred)| {
                    // TODO: for non-full components, just fetch the component value directly
                    let history_id = pred.rollback.prediction_history_id;
                    registry.component_metadata_map.get(kind).and_then(|m| {
                        m.deterministic
                            .as_ref()
                            .map(|d| (history_id, (*d, pred.pop_until_tick_and_hash())))
                    })
                })
                .collect()
        };
        trace!("HashFns used for ChecksumState: {:?}", hash_fns);
        ChecksumState {
            marker_id,
            archetypes: Default::default(),
            hash_fns,
            archetype_generation: ArchetypeGeneration::initial(),
        }
    }

    fn init_access(
        state: &Self::State,
        _system_meta: &mut SystemMeta,
        system_access: &mut SystemAccess,
        _world: &mut World,
    ) {
        let mut filtered_access = FilteredAccess::default();
        filtered_access.and_with(state.marker_id);
        // Hashed components are optional per archetype. Requiring them all would hide
        // conflicts on entities carrying only a subset, including DisableRollback entities.
        state
            .hash_fns
            .iter()
            .for_each(|(component_id, (_, pop_fn))| {
                if pop_fn.is_some() {
                    // the component is a PredictionHistory
                    // TODO: for non-full components, just fetch the component value directly
                    // The type-erased history hash function takes an exclusive component borrow.
                    filtered_access.access_mut().add_write(*component_id);
                } else {
                    filtered_access.access_mut().add_read(*component_id);
                }
            });
        let access = filtered_access;
        system_access.try_add(access).expect(
            "error[B0001]: dynamic component access conflicts with a previous system parameter",
        );
    }

    unsafe fn get_param<'world, 'state>(
        state: &'state mut Self::State,
        _system_meta: &SystemMeta,
        world: UnsafeWorldCell<'world>,
        _change_tick: Tick,
    ) -> Result<Self::Item<'world, 'state>, SystemParamValidationError> {
        Ok(ChecksumWorld { world, state })
    }
}

// SAFETY: Current-state checksums only read components; history checksums borrow histories mutably.
unsafe impl ReadOnlySystemParam for ChecksumWorld<'_, '_, false> {}

pub(crate) struct ChecksumState {
    /// ComponentId for the `Deterministic` marker component.
    pub(crate) marker_id: ComponentId,
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

#[cfg(test)]
mod tests {
    use super::ChecksumWorld;
    use bevy_app::App;
    use bevy_ecs::prelude::{Component, Query, With, Without};
    use bevy_ecs::system::SystemState;
    use lightyear_prediction::prelude::{
        DisableRollback, PredictionAppRegistrationExt, PredictionHistory, PredictionRegistry,
    };
    use lightyear_replication::prelude::AppComponentExt;

    #[derive(Component, Clone, Debug, Hash, PartialEq)]
    struct HashedComponent(u32);

    #[derive(Component, Clone, Debug, Hash, PartialEq)]
    struct OtherHashedComponent(u32);

    #[test]
    #[should_panic(expected = "error[B0001]")]
    fn history_checksum_rejects_conflicting_query_on_disabled_partial_entity() {
        let mut app = App::new();
        app.init_resource::<PredictionRegistry>();
        app.component::<HashedComponent>().add_default_hash();
        app.local_rollback::<HashedComponent>();
        app.component::<OtherHashedComponent>().add_default_hash();
        app.local_rollback::<OtherHashedComponent>();

        // Histories are accessed even with DisableRollback and without other hashed
        // components. Reject aliasing at initialization, before fetching either parameter.
        SystemState::<(
            ChecksumWorld<true>,
            Query<
                &mut PredictionHistory<HashedComponent>,
                (
                    With<DisableRollback>,
                    Without<PredictionHistory<OtherHashedComponent>>,
                ),
            >,
        )>::new(app.world_mut());
    }
}
