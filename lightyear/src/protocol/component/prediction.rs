use crate::client::components::{ComponentSyncMode, SyncComponent};
use crate::client::prediction::predicted_history::PredictionHistory;
use crate::client::prediction::resource::PredictionManager;
use crate::prelude::{ComponentRegistry, HistoryState, Linear, Tick};
use crate::protocol::component::registry::LerpFn;
use crate::protocol::component::{ComponentError, ComponentKind};
use bevy::ecs::component::ComponentId;
use bevy::ecs::world::{FilteredEntityMut, FilteredEntityRef};
use bevy::prelude::*;

#[derive(Debug, Clone, PartialEq)]
pub struct PredictionMetadata {
    /// Id of the PredictionHistory<C> component
    pub history_id: Option<ComponentId>,
    pub sync_mode: ComponentSyncMode,
    pub correction: Option<unsafe fn()>,
    /// Function used to compare the confirmed component with the predicted component's history
    /// to determine if a rollback is needed. Returns true if we should do a rollback.
    /// Will default to a PartialEq::ne implementation, but can be overriden.
    pub should_rollback: unsafe fn(),
    pub buffer_sync: SyncFn,
    pub check_rollback: CheckRollbackFn,
}

type SyncFn = fn(&mut ComponentRegistry, confirmed: Entity, predicted: Entity, &World);

type CheckRollbackFn = fn(
    &ComponentRegistry,
    confirmed_tick: Tick,
    confirmed_ref: &FilteredEntityRef,
    predicted_mut: &mut FilteredEntityMut,
) -> bool;

impl PredictionMetadata {
    fn default_from<C: SyncComponent>(
        history_id: Option<ComponentId>,
        mode: ComponentSyncMode,
    ) -> Self {
        let should_rollback: ShouldRollbackFn<C> = <C as PartialEq>::ne;
        Self {
            history_id,
            sync_mode: mode,
            correction: None,
            should_rollback: unsafe {
                core::mem::transmute::<for<'a, 'b> fn(&'a C, &'b C) -> bool, unsafe fn()>(
                    should_rollback,
                )
            },
            buffer_sync: ComponentRegistry::buffer_sync::<C>,
            check_rollback: ComponentRegistry::check_rollback::<C>,
        }
    }
}

/// Function that returns true if a rollback is needed, by comparing the server's value with the client's predicted value.
/// Defaults to PartialEq::ne
pub type ShouldRollbackFn<C> = fn(this: &C, that: &C) -> bool;

impl ComponentRegistry {
    pub fn predicted_component_ids(&self) -> impl Iterator<Item = ComponentId> + use<'_> {
        self.prediction_map
            .keys()
            .filter_map(|kind| self.kind_to_component_id.get(kind).copied())
    }

    pub fn predicted_component_ids_with_mode(
        &self,
        mode: ComponentSyncMode,
    ) -> impl Iterator<Item = ComponentId> + use<'_> {
        self.prediction_map
            .iter()
            .filter(move |(_, m)| m.sync_mode == mode)
            .filter_map(|(kind, _)| self.kind_to_component_id.get(kind).copied())
    }

    pub fn set_prediction_mode<C: SyncComponent>(
        &mut self,
        history_id: Option<ComponentId>,
        mode: ComponentSyncMode,
    ) {
        let kind = ComponentKind::of::<C>();
        self.prediction_map
            .entry(kind)
            .or_insert_with(|| PredictionMetadata::default_from::<C>(history_id, mode));
    }

    pub fn set_should_rollback<C: SyncComponent + PartialEq>(
        &mut self,
        should_rollback: ShouldRollbackFn<C>,
    ) {
        self.prediction_map
                .get_mut(&ComponentKind::of::<C>())
                .expect("The component has not been registered for prediction. Did you call `.add_prediction(ComponentSyncMode::Full)`")
                .should_rollback = unsafe {
                core::mem::transmute::<for<'a, 'b> fn(&'a C, &'b C) -> bool, unsafe fn()>(
                    should_rollback,
                )
            };
    }

    pub fn set_linear_correction<C: SyncComponent + Linear + PartialEq>(&mut self) {
        self.set_correction(<C as Linear>::lerp);
    }

    pub fn set_correction<C: SyncComponent + PartialEq>(&mut self, correction_fn: LerpFn<C>) {
        self.prediction_map
                .get_mut(&ComponentKind::of::<C>())
                .expect("The component has not been registered for prediction. Did you call `.add_prediction(ComponentSyncMode::Full)`")
                .correction = Some(unsafe {
                core::mem::transmute::<for<'a, 'b> fn(&'a C, &'b C, f32) -> C, unsafe fn()>(
                    correction_fn,
                )
            });
    }

    pub fn get_prediction_mode(
        &self,
        id: ComponentId,
    ) -> Result<ComponentSyncMode, ComponentError> {
        let kind = self
            .component_id_to_kind
            .get(&id)
            .ok_or(ComponentError::NotRegistered)?;
        Ok(self
            .prediction_map
            .get(kind)
            .map_or(ComponentSyncMode::None, |metadata| metadata.sync_mode))
    }

    pub fn prediction_mode<C: Component>(&self) -> ComponentSyncMode {
        let kind = ComponentKind::of::<C>();
        self.prediction_map
            .get(&kind)
            .map_or(ComponentSyncMode::None, |metadata| metadata.sync_mode)
    }

    pub fn has_correction<C: Component>(&self) -> bool {
        let kind = ComponentKind::of::<C>();
        self.prediction_map
            .get(&kind)
            .is_some_and(|metadata| metadata.correction.is_some())
    }

    /// Returns true if we should do a rollback
    pub fn should_rollback<C: Component>(&self, this: &C, that: &C) -> bool {
        let kind = ComponentKind::of::<C>();
        let prediction_metadata = self
            .prediction_map
            .get(&kind)
            .expect("the component is not part of the protocol");
        let should_rollback_fn: ShouldRollbackFn<C> =
            unsafe { core::mem::transmute(prediction_metadata.should_rollback) };
        should_rollback_fn(this, that)
    }

    pub fn correct<C: Component>(&self, predicted: &C, corrected: &C, t: f32) -> C {
        let kind = ComponentKind::of::<C>();
        let prediction_metadata = self
            .prediction_map
            .get(&kind)
            .expect("the component is not part of the protocol");
        let correction_fn: LerpFn<C> =
            unsafe { core::mem::transmute(prediction_metadata.correction.unwrap()) };
        correction_fn(predicted, corrected, t)
    }

    /// Clone the components from the confirmed entity to the predicted entity
    /// All the cloned components are inserted at once.
    pub fn batch_sync(
        &mut self,
        component_ids: &[ComponentId],
        confirmed: Entity,
        predicted: Entity,
        world: &mut World,
    ) {
        // clone each component to be synced into a temporary buffer
        component_ids.iter().for_each(|component_id| {
            let kind = self.component_id_to_kind.get(component_id).unwrap();
            let prediction_metadata = self
                .prediction_map
                .get(kind)
                .expect("the component is not part of the protocol");
            (prediction_metadata.buffer_sync)(self, confirmed, predicted, world);
        });
        // insert all the components in the predicted entity
        if let Ok(mut entity_world_mut) = world.get_entity_mut(predicted) {
            // SAFETY: we call `buffer_insert_raw_pts` inside the `buffer_sync` function
            unsafe { self.temp_write_buffer.batch_insert(&mut entity_world_mut) };
        };
    }

    /// Sync a component value from the confirmed entity to the predicted entity
    pub fn buffer_sync<C: SyncComponent>(
        &mut self,
        confirmed: Entity,
        predicted: Entity,
        world: &World,
    ) {
        let kind = ComponentKind::of::<C>();
        let prediction_metadata = self
            .prediction_map
            .get(&kind)
            .expect("the component is not part of the protocol");

        // NOTE: this is not needed because we have an observer that inserts the History as soon as C is inserted.
        // // for Full components, also insert a PredictionHistory component
        // // no need to add any value to it because otherwise it would contain a value with the wrong tick
        // // since we are running this outside of FixedUpdate
        // if prediction_metadata.prediction_mode == ComponentSyncMode::Full {
        //     // if the predicted entity already had a PredictionHistory component (for example
        //     // if the entity was PreSpawned entity), we don't want to overwrite it.
        //     if world.get::<PredictionHistory<C>>(predicted).is_none() {
        //         unsafe {
        //             self.temp_write_buffer.buffer_insert_raw_ptrs(
        //                 PredictionHistory::<C>::default(),
        //                 world
        //                     .component_id::<PredictionHistory<C>>()
        //                     .expect("PredictionHistory not registered"),
        //             )
        //         };
        //     }
        // }

        // TODO: add a test for this! For PreSpawned/PrePredicted we don't want to sync from Confirmed to Predicted
        // TODO: does this interact well with cases where the component is removed on the predicted entity?
        // if the predicted entity already has the component, we don't want to sync it:
        // - if the predicted entity is Predicted/PrePredicted/PreSpawned, we would be overwriting the predicted value, instead
        //   of letting the rollback systems work
        // - if the component is ComponentSyncMode::Once, we only need to sync it once
        // - if the component is ComponentSyncMode::Simple, every component update will be synced via a separate system
        if world.get::<C>(predicted).is_some() {
            return;
        }
        if let Some(value) = world.get::<C>(confirmed) {
            let mut clone = value.clone();
            world
                .resource::<PredictionManager>()
                .map_entities(&mut clone, self)
                .unwrap();
            unsafe {
                self.temp_write_buffer
                    .buffer_insert_raw_ptrs(clone, world.component_id::<C>().unwrap())
            };
        }
    }

    /// Returns true if we should rollback
    pub fn check_rollback<C: SyncComponent>(
        &self,
        confirmed_tick: Tick,
        confirmed_ref: &FilteredEntityRef,
        predicted_mut: &mut FilteredEntityMut,
    ) -> bool {
        let confirmed_component = confirmed_ref.get::<C>();
        let Some(mut predicted_history) = predicted_mut.get_mut::<PredictionHistory<C>>() else {
            // if the history is not present on the entity, but the confirmed component is present, we need to rollback
            return confirmed_component.is_some();
        };

        #[cfg(feature = "metrics")]
        metrics::gauge!(format!(
            "prediction::rollbacks::history::{:?}::num_values",
            core::any::type_name::<C>()
        ))
        .set(predicted_history.buffer.len() as f64);

        let history_value = predicted_history.pop_until_tick(confirmed_tick);
        let predicted_exist = history_value.is_some();
        let confirmed_exist = confirmed_component.is_some();
        match confirmed_component {
            // TODO: history-value should not be empty here; should we panic if it is?
            // confirm does not exist. rollback if history value is not Removed
            None => {
                let should = history_value
                    .is_some_and(|history_value| history_value != HistoryState::Removed);
                if should {
                    #[cfg(feature = "metrics")]
                    metrics::counter!(format!(
                        "prediction::rollbacks::causes::{}::missing_on_confirmed",
                        core::any::type_name::<C>()
                    ))
                    .increment(1)
                }
                should
            }
            // confirm exist. rollback if history value is different
            Some(c) => history_value.map_or_else(
                || {
                    #[cfg(feature = "metrics")]
                    metrics::counter!(format!(
                        "prediction::rollbacks::causes::{}::missing_on_predicted",
                        core::any::type_name::<C>()
                    ))
                    .increment(1);
                    true
                },
                |history_value| match history_value {
                    HistoryState::Updated(history_value) => {
                        let should = self.should_rollback(&history_value, c);
                        if should {
                            #[cfg(feature = "metrics")]
                            metrics::counter!(format!(
                                "prediction::rollbacks::causes::{}::value_mismatch",
                                core::any::type_name::<C>()
                            ))
                            .increment(1);
                        }
                        should
                    }
                    HistoryState::Removed => {
                        #[cfg(feature = "metrics")]
                        metrics::counter!(format!(
                            "prediction::rollbacks::causes::{}::removed_on_predicted",
                            core::any::type_name::<C>()
                        ))
                        .increment(1);
                        true
                    }
                },
            ),
        }
    }
}
