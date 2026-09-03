use crate::Predicted;
use crate::despawn::PredictionDisable;
use crate::registry::{
    CheckRollbackFn, PendingDiffTickFn, PredictionRegistry, PrepareRollbackFn,
    PruneHistoryDiffReceiverFn, RollbackMetadata, SnapToConfirmedFn,
    UpdateFrameInterpolationPostRollbackFn,
};
use crate::rollback::{DeterministicPredicted, DisableRollback};
use alloc::vec::Vec;
use bevy_ecs::{
    archetype::{Archetype, ArchetypeGeneration, ArchetypeId, Archetypes},
    change_detection::Tick as ChangeTick,
    component::{ComponentId, Components, StorageType},
    entity_disabling::DefaultQueryFilters,
    prelude::ResMut,
    query::{FilteredAccess, FilteredAccessSet},
    resource::Resource,
    system::{SystemMeta, SystemParam, SystemParamValidationError},
    world::{FromWorld, World, unsafe_world_cell::UnsafeWorldCell},
};
use core::marker::PhantomData;
use lightyear_replication::prelude::ConfirmHistory;
use lightyear_replication::registry::{ComponentKind, ComponentRegistry};

/// Low-level world access paired with the shared incremental prediction-archetype cache.
///
/// `MODE` only selects the component access needed by one type-erased prediction dispatcher.
pub(crate) struct PredictionWorld<'w, 's, const MODE: u8> {
    pub(crate) world: UnsafeWorldCell<'w>,
    cache: ResMut<'w, PredictedArchetypes>,
    marker: PhantomData<&'s ()>,
}

const CHECK_ROLLBACK: u8 = 0;
const PREPARE_ROLLBACK: u8 = 1;
const SNAP_TO_CONFIRMED: u8 = 2;
const UPDATE_FRAME_INTERPOLATION_POST_ROLLBACK: u8 = 3;
const PRUNE_DIFF_HISTORY: u8 = 4;

pub(crate) type CheckRollbackWorld<'w, 's> = PredictionWorld<'w, 's, CHECK_ROLLBACK>;
pub(crate) type PrepareRollbackWorld<'w, 's> = PredictionWorld<'w, 's, PREPARE_ROLLBACK>;
pub(crate) type SnapToConfirmedWorld<'w, 's> = PredictionWorld<'w, 's, SNAP_TO_CONFIRMED>;
pub(crate) type UpdateFrameInterpolationPostRollbackWorld<'w, 's> =
    PredictionWorld<'w, 's, UPDATE_FRAME_INTERPOLATION_POST_ROLLBACK>;
pub(crate) type PruneDiffHistoryWorld<'w, 's> = PredictionWorld<'w, 's, PRUNE_DIFF_HISTORY>;

impl<const MODE: u8> PredictionWorld<'_, '_, MODE> {
    /// Adds metadata for archetypes created since the previous scan.
    pub(crate) fn update_archetypes(
        &mut self,
        prediction_registry: &PredictionRegistry,
        component_registry: &ComponentRegistry,
    ) {
        self.cache.update(
            self.world.archetypes(),
            self.world.components(),
            prediction_registry,
            component_registry,
        );
    }

    /// Iterates archetypes containing at least one prediction-history component.
    pub(crate) fn predicted_archetypes(
        &self,
    ) -> impl Iterator<Item = (&Archetype, &CachedPredictionArchetype)> {
        self.cache.archetypes.iter().filter_map(move |cached| {
            (!cached.predicted_components.is_empty())
                .then(|| self.world.archetypes().get(cached.id).map(|a| (a, cached)))
                .flatten()
        })
    }

    /// Iterates archetypes containing at least one diff-predicted component history.
    pub(crate) fn diff_archetypes(
        &self,
    ) -> impl Iterator<Item = (&Archetype, &CachedPredictionArchetype)> {
        self.cache.archetypes.iter().filter_map(move |cached| {
            (!cached.diff_components.is_empty())
                .then(|| self.world.archetypes().get(cached.id).map(|a| (a, cached)))
                .flatten()
        })
    }

    /// Returns whether the old rollback-check query would contain at least one entity.
    pub(crate) fn has_check_entities(&self) -> bool {
        self.cache.archetypes.iter().any(|cached| {
            cached.check_target
                && self
                    .world
                    .archetypes()
                    .get(cached.id)
                    .is_some_and(|archetype| !archetype.entities().is_empty())
        })
    }
}

unsafe impl<const MODE: u8> SystemParam for PredictionWorld<'_, '_, MODE> {
    type State = <ResMut<'static, PredictedArchetypes> as SystemParam>::State;
    type Item<'world, 'state> = PredictionWorld<'world, 'state, MODE>;

    fn init_state(world: &mut World) -> Self::State {
        world.init_resource::<PredictedArchetypes>();
        <ResMut<'static, PredictedArchetypes> as SystemParam>::init_state(world)
    }

    fn init_access(
        state: &Self::State,
        system_meta: &mut SystemMeta,
        component_access_set: &mut FilteredAccessSet,
        world: &mut World,
    ) {
        <ResMut<'static, PredictedArchetypes> as SystemParam>::init_access(
            state,
            system_meta,
            component_access_set,
            world,
        );

        let mut access = FilteredAccess::default();

        for component_id in &world.resource::<PredictedArchetypes>().filter_component_ids {
            access.add_read(*component_id);
        }

        match MODE {
            CHECK_ROLLBACK => {
                for metadata in world
                    .resource::<PredictionRegistry>()
                    .prediction_map
                    .values()
                {
                    access.add_read(metadata.rollback.prediction_history_id);
                    access.add_write(metadata.rollback.confirmed_history_id);
                }
            }
            PREPARE_ROLLBACK => {
                for (kind, metadata) in world.resource::<PredictionRegistry>().rollback_metadata() {
                    let component_id = world
                        .components()
                        .get_id(kind.0)
                        .expect("rollback component should be registered in the world");
                    access.add_write(component_id);
                    access.add_write(metadata.prediction_history_id);
                    access.add_read(metadata.confirmed_history_id);
                }
            }
            SNAP_TO_CONFIRMED => {
                for (kind, metadata) in &world.resource::<PredictionRegistry>().prediction_map {
                    if metadata.snap_to_confirmed.is_none() {
                        continue;
                    }
                    let component_id = world
                        .components()
                        .get_id(kind.0)
                        .expect("predicted component should be registered in the world");
                    access.add_write(component_id);
                    access.add_read(metadata.rollback.confirmed_history_id);
                }
            }
            UPDATE_FRAME_INTERPOLATION_POST_ROLLBACK => {
                for (kind, metadata) in world.resource::<PredictionRegistry>().rollback_metadata() {
                    let component_id = world
                        .components()
                        .get_id(kind.0)
                        .expect("rollback component should be registered in the world");
                    access.add_read(component_id);
                    access.add_read(metadata.prediction_history_id);
                    access.add_write(metadata.frame_interpolation_history_id);
                }
            }
            PRUNE_DIFF_HISTORY => {
                for metadata in world
                    .resource::<PredictionRegistry>()
                    .prediction_map
                    .values()
                    .filter(|metadata| metadata.prune_history_diff_receiver.is_some())
                {
                    access.add_read(metadata.rollback.confirmed_history_id);
                }
            }
            _ => unreachable!("unknown prediction world access mode"),
        }

        component_access_set.add(access);
    }

    unsafe fn get_param<'world, 'state>(
        state: &'state mut Self::State,
        system_meta: &SystemMeta,
        world: UnsafeWorldCell<'world>,
        change_tick: ChangeTick,
    ) -> Result<Self::Item<'world, 'state>, SystemParamValidationError> {
        // SAFETY: `init_access` delegates resource access to `ResMut<PredictedArchetypes>`, and
        // the caller guarantees that this is the same World used by `init_state`.
        let cache = unsafe {
            <ResMut<'static, PredictedArchetypes> as SystemParam>::get_param(
                state,
                system_meta,
                world,
                change_tick,
            )
        }?;
        Ok(PredictionWorld {
            world,
            cache,
            marker: PhantomData,
        })
    }
}

/// Shared cache of the archetypes and components used by prediction systems.
#[derive(Resource)]
pub(crate) struct PredictedArchetypes {
    generation: ArchetypeGeneration,
    predicted_component_id: ComponentId,
    confirm_history_component_id: ComponentId,
    deterministic_predicted_component_id: ComponentId,
    disable_rollback_component_id: ComponentId,
    prediction_disable_component_id: ComponentId,
    disabling_component_ids: Vec<ComponentId>,
    filter_component_ids: Vec<ComponentId>,
    archetypes: Vec<CachedPredictionArchetype>,
}

/// Prediction operations resolved for one archetype.
pub(crate) struct CachedPredictionArchetype {
    id: ArchetypeId,
    /// Whether this archetype is included by Bevy's default query filters.
    pub(crate) default_query_target: bool,
    /// Whether this archetype contains [`Predicted`].
    pub(crate) contains_predicted_marker: bool,
    /// Whether this archetype contains [`DisableRollback`].
    pub(crate) has_disable_rollback: bool,
    /// Whether this archetype matches the entity-level rollback-check filters.
    pub(crate) check_target: bool,
    /// Every rollback component whose prediction history is present in this archetype.
    pub(crate) predicted_components: Vec<CachedPredictionComponent>,
    /// Every diff-predicted component with a prediction or confirmed history in this archetype.
    pub(crate) diff_components: Vec<CachedDiffComponent>,
}

/// Type-erased component access resolved for one archetype.
#[derive(Clone, Copy)]
pub(crate) struct CachedPredictionComponent {
    pub(crate) component_id: ComponentId,
    pub(crate) component_storage: Option<StorageType>,
    pub(crate) prediction_history_id: ComponentId,
    pub(crate) prediction_history_storage: Option<StorageType>,
    pub(crate) confirmed_history_id: ComponentId,
    pub(crate) confirmed_history_storage: Option<StorageType>,
    pub(crate) frame_interpolation_history_storage: Option<StorageType>,
    pub(crate) check_rollback: Option<CheckRollbackFn>,
    pub(crate) prepare_rollback: PrepareRollbackFn,
    pub(crate) snap_to_confirmed: Option<SnapToConfirmedFn>,
    pub(crate) update_frame_interpolation_post_rollback: UpdateFrameInterpolationPostRollbackFn,
    pub(crate) has_correction: bool,
}

/// Diff-prediction operations resolved for one archetype.
#[derive(Clone, Copy)]
pub(crate) struct CachedDiffComponent {
    pub(crate) prediction_history_storage: Option<StorageType>,
    pub(crate) confirmed_history_id: ComponentId,
    pub(crate) confirmed_history_storage: Option<StorageType>,
    pub(crate) pending_diff_tick: PendingDiffTickFn,
    pub(crate) prune_history_diff_receiver: PruneHistoryDiffReceiverFn,
}

impl FromWorld for PredictedArchetypes {
    fn from_world(world: &mut World) -> Self {
        let predicted_component_id = world.register_component::<Predicted>();
        let confirm_history_component_id = world.register_component::<ConfirmHistory>();
        let deterministic_predicted_component_id =
            world.register_component::<DeterministicPredicted>();
        let disable_rollback_component_id = world.register_component::<DisableRollback>();
        let prediction_disable_component_id = world.register_component::<PredictionDisable>();
        let disabling_component_ids = world
            .resource::<DefaultQueryFilters>()
            .disabling_ids()
            .collect::<Vec<_>>();
        let mut filter_component_ids = disabling_component_ids.clone();
        for component_id in [
            predicted_component_id,
            confirm_history_component_id,
            deterministic_predicted_component_id,
            disable_rollback_component_id,
            prediction_disable_component_id,
        ] {
            if !filter_component_ids.contains(&component_id) {
                filter_component_ids.push(component_id);
            }
        }
        Self {
            generation: ArchetypeGeneration::initial(),
            predicted_component_id,
            confirm_history_component_id,
            deterministic_predicted_component_id,
            disable_rollback_component_id,
            prediction_disable_component_id,
            disabling_component_ids,
            filter_component_ids,
            archetypes: Vec::new(),
        }
    }
}

impl PredictedArchetypes {
    fn update(
        &mut self,
        archetypes: &Archetypes,
        components: &Components,
        prediction_registry: &PredictionRegistry,
        component_registry: &ComponentRegistry,
    ) {
        let old_generation = core::mem::replace(&mut self.generation, archetypes.generation());

        for archetype in &archetypes[old_generation..] {
            let excluded_by_default_filter = self
                .disabling_component_ids
                .iter()
                .any(|id| archetype.contains(*id));
            let excluded_from_check = self
                .disabling_component_ids
                .iter()
                .any(|id| *id != self.prediction_disable_component_id && archetype.contains(*id));
            let default_query_target = !excluded_by_default_filter;
            let contains_predicted_marker = archetype.contains(self.predicted_component_id);
            let has_disable_rollback = archetype.contains(self.disable_rollback_component_id);

            let mut cached = CachedPredictionArchetype {
                id: archetype.id(),
                default_query_target,
                contains_predicted_marker,
                has_disable_rollback,
                check_target: false,
                predicted_components: Vec::new(),
                diff_components: Vec::new(),
            };

            for (kind, metadata) in prediction_registry.rollback_metadata() {
                let prediction_history_storage =
                    archetype.get_storage_type(metadata.prediction_history_id);
                let confirmed_history_storage =
                    archetype.get_storage_type(metadata.confirmed_history_id);
                if prediction_history_storage.is_none() && confirmed_history_storage.is_none() {
                    continue;
                }
                cached.predicted_components.push(Self::component(
                    archetype,
                    components,
                    kind,
                    prediction_history_storage,
                    confirmed_history_storage,
                    prediction_registry,
                    metadata,
                    component_registry,
                ));
            }

            let has_replicated_prediction_history =
                cached.predicted_components.iter().any(|component| {
                    component.check_rollback.is_some()
                        && component.prediction_history_storage.is_some()
                });
            cached.check_target = contains_predicted_marker
                && archetype.contains(self.confirm_history_component_id)
                && !archetype.contains(self.deterministic_predicted_component_id)
                && !has_disable_rollback
                && !excluded_from_check
                && has_replicated_prediction_history;

            for metadata in prediction_registry.prediction_map.values() {
                let (Some(pending_diff_tick), Some(prune_history_diff_receiver)) = (
                    metadata.pending_diff_tick,
                    metadata.prune_history_diff_receiver,
                ) else {
                    continue;
                };
                let prediction_history_storage =
                    archetype.get_storage_type(metadata.rollback.prediction_history_id);
                let confirmed_history_storage =
                    archetype.get_storage_type(metadata.rollback.confirmed_history_id);
                if prediction_history_storage.is_none() && confirmed_history_storage.is_none() {
                    continue;
                }
                cached.diff_components.push(CachedDiffComponent {
                    prediction_history_storage,
                    confirmed_history_id: metadata.rollback.confirmed_history_id,
                    confirmed_history_storage,
                    pending_diff_tick,
                    prune_history_diff_receiver,
                });
            }

            if !cached.predicted_components.is_empty() || !cached.diff_components.is_empty() {
                self.archetypes.push(cached);
            }
        }
    }

    fn component(
        archetype: &Archetype,
        components: &Components,
        kind: ComponentKind,
        prediction_history_storage: Option<StorageType>,
        confirmed_history_storage: Option<StorageType>,
        prediction_registry: &PredictionRegistry,
        rollback: &RollbackMetadata,
        component_registry: &ComponentRegistry,
    ) -> CachedPredictionComponent {
        let prediction = prediction_registry.prediction_map.get(&kind);
        let component_id = components
            .get_id(kind.0)
            .expect("rollback component should be registered in the world");
        CachedPredictionComponent {
            component_id,
            component_storage: archetype.get_storage_type(component_id),
            prediction_history_id: rollback.prediction_history_id,
            prediction_history_storage,
            confirmed_history_id: rollback.confirmed_history_id,
            confirmed_history_storage,
            frame_interpolation_history_storage: archetype
                .get_storage_type(rollback.frame_interpolation_history_id),
            check_rollback: prediction
                .filter(|_| {
                    component_registry
                        .component_metadata_map
                        .contains_key(&kind)
                })
                .map(|metadata| metadata.check_rollback),
            prepare_rollback: rollback.prepare_rollback,
            snap_to_confirmed: prediction.and_then(|metadata| metadata.snap_to_confirmed),
            update_frame_interpolation_post_rollback: rollback
                .update_frame_interpolation_post_rollback,
            has_correction: prediction.is_some_and(|metadata| {
                metadata.custom_correction || metadata.correction.is_some()
            }),
        }
    }
}
