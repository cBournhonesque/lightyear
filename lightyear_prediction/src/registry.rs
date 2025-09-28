use crate::SyncComponent;
use crate::plugin::{
    add_non_networked_rollback_systems, add_prediction_systems, add_resource_rollback_systems,
};
use crate::predicted_history::PredictionHistory;
#[cfg(feature = "metrics")]
use alloc::format;
use bevy_app::App;
use bevy_ecs::component::ComponentId;
use bevy_ecs::prelude::*;
use bevy_ecs::ptr::PtrMut;
use bevy_ecs::world::FilteredEntityMut;
use bevy_math::{
    Curve,
    curve::{Ease, EaseFunction, EasingCurve},
};
use core::fmt::Debug;
use bevy_utils::prelude::DebugName;
use lightyear_core::history_buffer::HistoryState;
use lightyear_core::tick::Tick;
use lightyear_replication::components::Confirmed;
use lightyear_replication::delta::Diffable;
use lightyear_replication::prelude::ComponentRegistration;
use lightyear_replication::registry::registry::{ComponentRegistry, LerpFn};
use lightyear_replication::registry::{ComponentError, ComponentKind};
use lightyear_utils::collections::HashMap;
use tracing::{debug, trace, trace_span};

fn lerp<C: Ease + Clone>(start: C, other: C, t: f32) -> C {
    let curve = EasingCurve::new(start, other, EaseFunction::Linear);
    curve.sample_unchecked(t)
}

#[derive(Debug, Clone)]
pub struct PredictionMetadata {
    /// Id of the [`PredictionHistory<C>`] component
    pub history_id: Option<ComponentId>,
    pub(crate) correction: Option<unsafe fn()>,
    /// Function used to compare the confirmed component with the predicted component's history
    /// to determine if a rollback is needed. Returns true if we should do a rollback.
    /// Will default to a PartialEq::ne implementation, but can be overridden.
    pub(crate) should_rollback: unsafe fn(),
    pub(crate) check_rollback: CheckRollbackFn,
    #[cfg(feature = "deterministic")]
    /// Function to call `pop_until_tick` on the [`PredictionHistory<C>`] component.
    pub pop_until_tick_and_hash: Option<PopUntilTickAndHashFn>,
}

impl PredictionMetadata {
    #[cfg(feature = "deterministic")]
    pub fn pop_until_tick_and_hash(&self) -> Option<PopUntilTickAndHashFn> {
        self.pop_until_tick_and_hash
    }
}

/// Function that will check if we should do a rollback by comparing the confirmed component value
/// with the predicted component's history.
type CheckRollbackFn =
    fn(&PredictionRegistry, confirmed_tick: Tick, entity_mut: &mut FilteredEntityMut) -> bool;

/// Type-erased function for calling `pop_until_tick` and then `hash` on a [`PredictionHistory<C>`] component.
/// The function fn should be of type fn(&C, &mut seahash::SeaHasher) and will be called with the value popped from the history.
pub type PopUntilTickAndHashFn = fn(PtrMut, Tick, &mut seahash::SeaHasher, fn());

impl PredictionMetadata {
    fn new<C: SyncComponent>(history_id: ComponentId) -> Self {
        let should_rollback: ShouldRollbackFn<C> = <C as PartialEq>::ne;
        Self {
            history_id: Some(history_id),
            correction: None,
            should_rollback: unsafe {
                core::mem::transmute::<for<'a, 'b> fn(&'a C, &'b C) -> bool, unsafe fn()>(
                    should_rollback,
                )
            },
            check_rollback: PredictionRegistry::check_rollback::<C>,
            #[cfg(feature = "deterministic")]
            pop_until_tick_and_hash: Some(PredictionRegistry::pop_until_tick_and_hash::<C>),
        }
    }
}

/// Function called when comparing the confirmed component value (received from the remote) with the
/// predicted component value (from the local [`PredictionHistory`]).
///
/// In general we use [`PartialEq::ne`] by default, but you can provide your own function with [`ComponentRegistration::add_should_rollback`] to customize
/// the rollback behavior. (for example, you might want to ignore small floating point differences)
pub type ShouldRollbackFn<C> = fn(confirmed: &C, predicted: &C) -> bool;

#[derive(Resource, Default, Debug)]
pub struct PredictionRegistry {
    pub prediction_map: HashMap<ComponentKind, PredictionMetadata>,
}

impl PredictionRegistry {
    fn register<C: SyncComponent>(&mut self, history_id: ComponentId) {
        let kind = ComponentKind::of::<C>();
        self.prediction_map
            .entry(kind)
            .or_insert_with(|| PredictionMetadata::new::<C>(history_id));
    }

    fn set_should_rollback<C: SyncComponent>(&mut self, should_rollback: ShouldRollbackFn<C>) {
        self.prediction_map
                .get_mut(&ComponentKind::of::<C>())
                .expect("The component has not been registered for prediction. Did you call `.add_prediction(PredictionMode::Full)`")
                .should_rollback = unsafe {
                core::mem::transmute::<for<'a, 'b> fn(&'a C, &'b C) -> bool, unsafe fn()>(
                    should_rollback,
                )
            };
    }

    fn set_correction<C: SyncComponent + PartialEq>(&mut self, correction_fn: LerpFn<C>) {
        self.prediction_map
                .get_mut(&ComponentKind::of::<C>())
                .expect("The component has not been registered for prediction. Did you call `.add_prediction(PredictionMode::Full)`")
                .correction = Some(unsafe {
                core::mem::transmute(
                    correction_fn,
                )
            });
    }

    /// Returns true if the component is predicted
    pub(crate) fn predicted_id(
        &self,
        id: ComponentId,
        component_registry: &ComponentRegistry,
    ) -> Result<bool, ComponentError> {
        let kind = component_registry
            .component_id_to_kind
            .get(&id)
            .ok_or(ComponentError::NotRegistered)?;
        Ok(self.prediction_map.get(kind).is_some())
    }

    /// Returns true if the component is predicted
    pub(crate) fn predicted<C: Component>(&self) -> bool {
        let kind = ComponentKind::of::<C>();
        self.prediction_map.get(&kind).is_some()
    }

    pub(crate) fn has_correction<C: Component>(&self) -> bool {
        let kind = ComponentKind::of::<C>();
        self.prediction_map
            .get(&kind)
            .is_some_and(|metadata| metadata.correction.is_some())
    }

    /// Returns true if we should do a rollback
    pub(crate) fn should_rollback<C: Component>(&self, this: &C, that: &C) -> bool {
        let kind = ComponentKind::of::<C>();
        let prediction_metadata = self
            .prediction_map
            .get(&kind)
            .expect("the component is not part of the protocol");
        let should_rollback_fn: ShouldRollbackFn<C> =
            unsafe { core::mem::transmute(prediction_metadata.should_rollback) };
        should_rollback_fn(this, that)
    }

    /// Returns true if we should rollback
    fn check_rollback<C: SyncComponent>(
        &self,
        confirmed_tick: Tick,
        entity_mut: &mut FilteredEntityMut,
    ) -> bool {
        let entity = entity_mut.entity();
        let name = DebugName::type_name::<C>();
        let _span = trace_span!(
            "check_rollback",
            ?name,
            %entity,
            ?confirmed_tick
        )
        .entered();

        // TODO: if the history is not present on the entity, but the confirmed component is present, we need to rollback
        let Some(mut predicted_history) = entity_mut.get_mut::<PredictionHistory<C>>() else {
            // TODO: if the history is not present on the entity, but the confirmed component is present, we need to rollback!
            //  requires mutable aliasing
            return false;
        };
        #[cfg(feature = "metrics")]
        metrics::gauge!(format!(
            "prediction::rollbacks::history::{:?}::num_values",
            DebugName::type_name::<C>()
        ))
        .set(predicted_history.len() as f64);
        let history_value = predicted_history.pop_until_tick(confirmed_tick);
        let confirmed_component = entity_mut.get::<Confirmed<C>>();

        debug!(?history_value, ?confirmed_component, "check");
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
                        DebugName::type_name::<C>()
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
                        DebugName::type_name::<C>()
                    ))
                    .increment(1);
                    true
                },
                |history_value| match history_value {
                    HistoryState::Updated(history_value) => {
                        let should = self.should_rollback(&c.0, &history_value);
                        if should {
                            debug!(
                                "Should Rollback! Confirmed value {c:?} is different from history value {history_value:?}",
                            );
                            #[cfg(feature = "metrics")]
                            metrics::counter!(format!(
                                "prediction::rollbacks::causes::{}::value_mismatch",
                                DebugName::type_name::<C>()
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
                            DebugName::type_name::<C>()
                        ))
                        .increment(1);
                        true
                    }
                },
            ),
        }
    }

    /// Type-erased function for calling `pop_until_tick` on a [`PredictionHistory<C>`] component.
    ///
    /// Safety
    ///
    /// - The PtrMut must point to a valid [`PredictionHistory<C>`] component.
    /// - The function `f` must be a valid function of type `fn(&C, &mut seahash::SeaHasher)`.
    fn pop_until_tick_and_hash<C: Debug + Clone + 'static>(
        ptr: PtrMut,
        tick: Tick,
        hasher: &mut seahash::SeaHasher,
        f: fn(),
    ) {
        // SAFETY: the caller must ensure that the function has the correct type
        let f = unsafe { core::mem::transmute::<fn(), fn(&C, &mut seahash::SeaHasher)>(f) };
        // SAFETY: the caller must ensure that the pointer is valid and points to a PredictionHistory<C>
        let history = unsafe { ptr.deref_mut::<PredictionHistory<C>>() };
        if let Some(HistoryState::Updated(v)) = history.pop_until_tick(tick) {
            trace!(
                "Popped value from PredictionHistory<{:?}? at tick {:?}: {:?} for hashing",
                DebugName::type_name::<C>(),
                tick,
                v
            );
            f(&v, hasher);
        }
    }
}

pub trait PredictionRegistrationExt<C> {
    /// Enable prediction for this component.
    fn add_prediction(self) -> Self
    where
        C: SyncComponent;

    /// Add correction for this component where the interpolation will done using the lerp function
    /// provided by the [`Ease`] trait.
    fn add_linear_correction_fn(self) -> Self
    where
        C: SyncComponent + Ease + Diffable<Delta = C>;

    /// Add correction for this component where the interpolation will done using the lerp function
    /// provided by the [`Ease`] trait.
    fn add_correction_fn(self, correction_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent + Diffable<Delta = C>;

    /// Add a custom comparison function to determine if we should rollback by comparing the
    /// confirmed component with the predicted component's history.
    fn add_should_rollback(self, should_rollback: ShouldRollbackFn<C>) -> Self
    where
        C: SyncComponent;
}

impl<C> PredictionRegistrationExt<C> for ComponentRegistration<'_, C> {
    fn add_prediction(self) -> Self
    where
        C: SyncComponent,
    {
        let history_id = self
            .app
            .world_mut()
            .register_component::<PredictionHistory<C>>();
        if !self.app.world().contains_resource::<PredictionRegistry>() {
            self.app
                .world_mut()
                .insert_resource(PredictionRegistry::default());
        }
        let mut registry = self.app.world_mut().resource_mut::<PredictionRegistry>();
        trace!(
            "Adding prediction for component {:?}",
            DebugName::type_name::<C>()
        );
        registry.register::<C>(history_id);
        // TODO: how do we avoid the server adding the prediction systems?
        //   do we need to make sure that the Protocol runs after the client/server plugins are added?
        add_prediction_systems::<C>(self.app);

        let mut registry = self.app.world_mut().resource_mut::<ComponentRegistry>();
        registry
            .component_metadata_map
            .get_mut(&ComponentKind::of::<C>())
            .unwrap()
            .replication
            .as_mut()
            .unwrap()
            .set_predicted(true);
        self
    }

    fn add_linear_correction_fn(self) -> Self
    where
        C: SyncComponent + Ease + Diffable<Delta = C>,
    {
        self.add_correction_fn(lerp::<C>)
    }

    fn add_correction_fn(self, correction_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent + Diffable<Delta = C>,
    {
        crate::correction::add_correction_systems::<C>(self.app);

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
        let history_id = self
            .app
            .world_mut()
            .register_component::<PredictionHistory<C>>();
        // skip if there is no PredictionRegistry (i.e. the PredictionPlugin wasn't added)
        let Some(mut registry) = self
            .app
            .world_mut()
            .get_resource_mut::<PredictionRegistry>()
        else {
            return self;
        };
        registry.register::<C>(history_id);
        registry.set_should_rollback::<C>(should_rollback);
        self
    }
}

pub trait PredictionAppRegistrationExt {
    /// Enable rollbacks for a component that is not networked.
    fn add_rollback<C: SyncComponent>(&mut self) -> ComponentRegistration<'_, C>;

    fn add_resource_rollback<R: Resource + Clone>(&mut self);
}

impl PredictionAppRegistrationExt for App {
    fn add_rollback<C: SyncComponent>(&mut self) -> ComponentRegistration<'_, C> {
        let history_id = self
            .world_mut()
            .register_component::<PredictionHistory<C>>();
        // skip if there is no PredictionRegistry (i.e. the PredictionPlugin wasn't added)
        let Some(mut registry) = self.world_mut().get_resource_mut::<PredictionRegistry>() else {
            return ComponentRegistration::<C>::new(self);
        };

        registry.register::<C>(history_id);
        add_non_networked_rollback_systems::<C>(self);
        ComponentRegistration::<C>::new(self)
    }

    fn add_resource_rollback<R: Resource + Clone>(&mut self) {
        // skip if there is no PredictionRegistry (i.e. the PredictionPlugin wasn't added)
        if self.world().get_resource::<PredictionRegistry>().is_none() {
            return;
        }
        add_resource_rollback_systems::<R>(self);
    }
}
