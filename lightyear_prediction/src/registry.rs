use crate::manager::{PredictionManager, PredictionResource};
use crate::plugin::{add_non_networked_rollback_systems, add_prediction_systems};
use crate::predicted_history::PredictionHistory;
use crate::{PredictionMode, SyncComponent};
use bevy::ecs::component::{ComponentId, Mutable};
use bevy::ecs::world::{FilteredEntityMut, FilteredEntityRef};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use lightyear_core::history_buffer::HistoryState;
use lightyear_core::tick::Tick;
use lightyear_replication::prelude::ComponentRegistration;
use lightyear_replication::registry::buffered::BufferedChanges;
use lightyear_replication::registry::registry::{ComponentRegistry, LerpFn};
use lightyear_replication::registry::{ComponentError, ComponentKind};

fn lerp<C: Ease + Clone>(start: C, other: C, t: f32) -> C {
    let curve = EasingCurve::new(start, other, EaseFunction::Linear);
    curve.sample_unchecked(t)
}

#[derive(Debug, Clone, PartialEq)]
pub struct PredictionMetadata {
    /// Id of the PredictionHistory<C> component
    pub history_id: Option<ComponentId>,
    pub sync_mode: PredictionMode,
    pub correction: Option<unsafe fn()>,
    /// Function used to compare the confirmed component with the predicted component's history
    /// to determine if a rollback is needed. Returns true if we should do a rollback.
    /// Will default to a PartialEq::ne implementation, but can be overriden.
    pub should_rollback: unsafe fn(),
    pub buffer_sync: SyncFn,
    pub check_rollback: CheckRollbackFn,
}

type SyncFn = fn(
    &PredictionRegistry,
    &ComponentRegistry,
    confirmed: Entity,
    predicted: Entity,
    &World,
    &mut BufferedChanges,
);

type CheckRollbackFn = fn(
    &PredictionRegistry,
    confirmed_tick: Tick,
    confirmed_ref: &FilteredEntityRef,
    predicted_mut: &mut FilteredEntityMut,
) -> bool;

impl PredictionMetadata {
    fn default_from<C: SyncComponent>(
        history_id: Option<ComponentId>,
        mode: PredictionMode,
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
            buffer_sync: PredictionRegistry::buffer_sync::<C>,
            check_rollback: PredictionRegistry::check_rollback::<C>,
        }
    }
}

/// Function that returns true if a rollback is needed, by comparing the server's value with the client's predicted value.
/// Defaults to PartialEq::ne
pub type ShouldRollbackFn<C> = fn(this: &C, that: &C) -> bool;

#[derive(Resource, Default, Debug)]
pub struct PredictionRegistry {
    pub prediction_map: HashMap<ComponentKind, PredictionMetadata>,
}

impl PredictionRegistry {
    pub fn set_prediction_mode<C: SyncComponent>(
        &mut self,
        history_id: Option<ComponentId>,
        mode: PredictionMode,
    ) {
        let kind = ComponentKind::of::<C>();
        self.prediction_map
            .entry(kind)
            .or_insert_with(|| PredictionMetadata::default_from::<C>(history_id, mode));
    }

    pub fn set_should_rollback<C: SyncComponent>(&mut self, should_rollback: ShouldRollbackFn<C>) {
        self.prediction_map
                .get_mut(&ComponentKind::of::<C>())
                .expect("The component has not been registered for prediction. Did you call `.add_prediction(PredictionMode::Full)`")
                .should_rollback = unsafe {
                core::mem::transmute::<for<'a, 'b> fn(&'a C, &'b C) -> bool, unsafe fn()>(
                    should_rollback,
                )
            };
    }

    pub fn set_linear_correction<C: SyncComponent + Ease + PartialEq>(&mut self) {
        self.set_correction(lerp::<C>);
    }

    pub fn set_correction<C: SyncComponent + PartialEq>(&mut self, correction_fn: LerpFn<C>) {
        self.prediction_map
                .get_mut(&ComponentKind::of::<C>())
                .expect("The component has not been registered for prediction. Did you call `.add_prediction(PredictionMode::Full)`")
                .correction = Some(unsafe {
                core::mem::transmute(
                    correction_fn,
                )
            });
    }

    pub fn get_prediction_mode(
        &self,
        id: ComponentId,
        component_registry: &ComponentRegistry,
    ) -> Result<PredictionMode, ComponentError> {
        let kind = component_registry
            .component_id_to_kind
            .get(&id)
            .ok_or(ComponentError::NotRegistered)?;
        Ok(self
            .prediction_map
            .get(kind)
            .map_or(PredictionMode::None, |metadata| metadata.sync_mode))
    }

    pub fn prediction_mode<C: Component>(&self) -> PredictionMode {
        let kind = ComponentKind::of::<C>();
        self.prediction_map
            .get(&kind)
            .map_or(PredictionMode::None, |metadata| metadata.sync_mode)
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

    pub fn correct<C: Component>(&self, predicted: C, corrected: C, t: f32) -> C {
        let kind = ComponentKind::of::<C>();
        let prediction_metadata = self
            .prediction_map
            .get(&kind)
            .expect("the component is not part of the protocol");
        let correction_fn: LerpFn<C> =
            unsafe { core::mem::transmute(prediction_metadata.correction.unwrap()) };
        correction_fn(predicted, corrected, t)
    }

    // TODO: also sync removals!
    /// Clone the components from the confirmed entity to the predicted entity
    /// All the cloned components are inserted at once.
    pub fn batch_sync(
        &self,
        component_registry: &ComponentRegistry,
        component_ids: &[ComponentId],
        confirmed: Entity,
        predicted: Entity,
        world: &mut World,
        buffer: &mut BufferedChanges,
    ) {
        // clone each component to be synced into a temporary buffer
        component_ids.iter().for_each(|component_id| {
            let kind = component_registry
                .component_id_to_kind
                .get(component_id)
                .unwrap();
            let prediction_metadata = self
                .prediction_map
                .get(kind)
                .expect("the component is not part of the protocol");
            (prediction_metadata.buffer_sync)(
                self,
                component_registry,
                confirmed,
                predicted,
                world,
                buffer,
            );
        });
        // insert all the components in the predicted entity
        if let Ok(mut entity_world_mut) = world.get_entity_mut(predicted) {
            buffer.apply(&mut entity_world_mut);
        };
    }

    /// Sync a component value from the confirmed entity to the predicted entity
    pub fn buffer_sync<C: SyncComponent>(
        &self,
        component_registry: &ComponentRegistry,
        confirmed: Entity,
        predicted: Entity,
        world: &World,
        buffer: &mut BufferedChanges,
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
        // if prediction_metadata.prediction_mode == PredictionMode::Full {
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
        // - if the component is PredictionMode::Once, we only need to sync it once
        // - if the component is PredictionMode::Simple, every component update will be synced via a separate system
        if world.get::<C>(predicted).is_some() {
            return;
        }
        if let Some(value) = world.get::<C>(confirmed) {
            let mut clone = value.clone();
            let prediction_entity = world.resource::<PredictionResource>().link_entity;
            world
                .get::<PredictionManager>(prediction_entity)
                .unwrap()
                .map_entities(&mut clone, component_registry)
                .unwrap();
            // SAFETY: the component_id matches the component of type C
            unsafe {
                buffer.insert::<C>(clone, world.component_id::<C>().unwrap());
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
        let predicted_entity = predicted_mut.entity();
        let confirmed_entity = confirmed_ref.entity();
        let name = core::any::type_name::<C>();
        let _span = trace_span!(
            "check_rollback",
            ?name,
            %predicted_entity,
            %confirmed_entity,
            ?confirmed_tick
        )
        .entered();
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
        .set(predicted_history.len() as f64);

        let history_value = predicted_history.pop_until_tick(confirmed_tick);
        debug!(?history_value, ?confirmed_component, "check");
        let predicted_exist = history_value.is_some();
        let confirmed_exist = confirmed_component.is_some();
        match confirmed_component {
            // TODO: history-value should not be empty here; should we panic if it is?
            // confirm does not exist. rollback if history value is not Removed
            None => {
                let should = history_value
                    .is_some_and(|history_value| history_value != HistoryState::Removed);

                if should {
                    debug!(
                        "Should Rollback! Confirmed component does not exist, but history value exists",
                    );
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
                    debug!(
                        "Should Rollback! Confirmed component exists, but history value does not exists",
                    );
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
                            debug!(
                                "Should Rollback! Confirmed value is different from history value",
                            );
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
                        debug!(
                            "Should Rollback! Confirmed component exists, but history value does not exists",
                        );
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

pub trait PredictionRegistrationExt<C> {
    fn add_prediction(self, prediction_mode: PredictionMode) -> Self
    where
        C: SyncComponent;
    fn add_linear_correction_fn(self) -> Self
    where
        C: SyncComponent + Ease;
    fn add_correction_fn(self, correction_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent;
    fn add_should_rollback(self, should_rollback: ShouldRollbackFn<C>) -> Self
    where
        C: SyncComponent;
}

impl<C> PredictionRegistrationExt<C> for ComponentRegistration<'_, C> {
    fn add_prediction(self, prediction_mode: PredictionMode) -> Self
    where
        C: SyncComponent,
    {
        let history_id = (prediction_mode == PredictionMode::Full).then(|| {
            self.app
                .world_mut()
                .register_component::<PredictionHistory<C>>()
        });
        // skip if there is no PredictionRegistry (i.e. the PredictionPlugin wasn't added)
        // NOTE: this means that the protocol registration needs to happen after other plugins
        let Some(mut registry) = self
            .app
            .world_mut()
            .get_resource_mut::<PredictionRegistry>()
        else {
            return self;
        };
        trace!(
            "Adding prediction for component {:?} with mode {:?}",
            core::any::type_name::<C>(),
            prediction_mode
        );
        registry.set_prediction_mode::<C>(history_id, prediction_mode);
        // TODO: how do we avoid the server adding the prediction systems?
        //   do we need to make sure that the Protocol runs after the client/server plugins are added?
        add_prediction_systems::<C>(self.app, prediction_mode);
        self
    }

    fn add_linear_correction_fn(self) -> Self
    where
        C: SyncComponent + Ease,
    {
        // skip if there is no PredictionRegistry (i.e. the PredictionPlugin wasn't added)
        let Some(mut registry) = self
            .app
            .world_mut()
            .get_resource_mut::<PredictionRegistry>()
        else {
            return self;
        };
        registry.set_linear_correction::<C>();
        self
    }

    fn add_correction_fn(self, correction_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent,
    {
        // skip if there is no PredictionRegistry (i.e. the PredictionPlugin wasn't added)
        let Some(mut registry) = self
            .app
            .world_mut()
            .get_resource_mut::<PredictionRegistry>()
        else {
            return self;
        };
        registry.set_correction::<C>(correction_fn);
        self
    }

    fn add_should_rollback(self, should_rollback: ShouldRollbackFn<C>) -> Self
    where
        C: SyncComponent,
    {
        // skip if there is no PredictionRegistry (i.e. the PredictionPlugin wasn't added)
        let Some(mut registry) = self
            .app
            .world_mut()
            .get_resource_mut::<PredictionRegistry>()
        else {
            return self;
        };
        registry.set_should_rollback::<C>(should_rollback);
        self
    }
}

pub trait PredictionAppRegistrationExt {
    /// Enable rollbacks for a component even if the component is not networked
    fn add_rollback<C: Component<Mutability = Mutable> + PartialEq + Clone>(&mut self);
}

impl PredictionAppRegistrationExt for App {
    fn add_rollback<C: Component<Mutability = Mutable> + PartialEq + Clone>(&mut self) {
        let is_client = self.world().get_resource::<PredictionRegistry>().is_some();
        if is_client {
            add_non_networked_rollback_systems::<C>(self);
        }
    }
}
