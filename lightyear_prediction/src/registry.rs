use crate::SyncComponent;
use crate::plugin::{
    add_non_networked_rollback_systems, add_prediction_systems, add_resource_rollback_systems,
};
use crate::predicted_history::PredictionHistory;
#[cfg(feature = "metrics")]
use alloc::format;
use bevy_app::App;
use bevy_ecs::component::{ComponentId};
use bevy_ecs::prelude::*;
use bevy_ecs::ptr::PtrMut;
use bevy_ecs::world::FilteredEntityMut;
use bevy_math::{
    Curve,
    curve::{Ease, EaseFunction, EasingCurve},
};
use bevy_utils::prelude::DebugName;
use core::fmt::Debug;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::prelude::{AppMarkerExt, RuleFns};
use bevy_replicon::shared::replication::command_markers::MarkerConfig;
use bevy_replicon::shared::replication::deferred_entity::DeferredEntity;
use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, WriteCtx};
use lightyear_core::tick::Tick;
use lightyear_replication::delta::Diffable;
use lightyear_replication::registry::{ComponentError, ComponentKind, ComponentRegistry, LerpFn};
use lightyear_utils::collections::HashMap;
use tracing::{debug, trace, trace_span};
use lightyear_core::prediction::Predicted;
use lightyear_replication::registry::replication::ComponentRegistration;
use crate::manager::{PredictionResource, RollbackMode, StateRollbackMetadata};
use crate::prelude::PredictionManager;

fn lerp<C: Ease + Clone>(start: C, other: C, t: f32) -> C {
    let curve = EasingCurve::new(start, other, EaseFunction::Linear);
    curve.sample_unchecked(t)
}

#[derive(Debug, Clone)]
pub struct PredictionMetadata {
    /// Id of the [`PredictionHistory<C>`] component
    pub history_id: ComponentId,
    pub(crate) correction: bool,
    /// Custom interpolation function used to interpolate the rollback error
    pub(crate) correction_fn: Option<unsafe fn()>,
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
            history_id,
            correction: false,
            correction_fn: None,
            should_rollback: unsafe {
                core::mem::transmute::<for<'a, 'b> fn(&'a C, &'b C) -> bool, unsafe fn()>(
                    should_rollback,
                )
            },
            check_rollback: PredictionRegistry::check_rollback_empty_mutate::<C>,
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

    #[doc(hidden)]
    pub fn apply_correction<C: SyncComponent, D: Default>(&self, error: D, r: f32) -> Option<D> {
        self.prediction_map
            .get(&ComponentKind::of::<C>())
            .expect("The component has not been registered for prediction. Did you call `.add_prediction(PredictionMode::Full)`")
            .correction_fn
            .map(|correction_fn| {
            // SAFETY: the correction_fn was registered as a LerpFn<D>
            let lerp_fn = unsafe { core::mem::transmute::<unsafe fn(), LerpFn<D>>(correction_fn) };
            lerp_fn(D::default(), error, r)
        })
    }

    fn enable_correction<C: SyncComponent>(&mut self) {
        self.prediction_map
            .get_mut(&ComponentKind::of::<C>())
            .expect("The component has not been registered for prediction. Did you call `.add_prediction(PredictionMode::Full)`").correction = true;
    }

    fn set_correction_fn<C: SyncComponent, D>(&mut self, correction_fn: LerpFn<D>) {
        let metadata = self.prediction_map
            .get_mut(&ComponentKind::of::<C>())
            .expect("The component has not been registered for prediction. Did you call `.add_prediction(PredictionMode::Full)`");
        metadata.correction = true;
        metadata.correction_fn = Some(unsafe { core::mem::transmute(correction_fn) });
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
            .is_some_and(|metadata| metadata.correction)
    }

    #[doc(hidden)]
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

    pub fn should_rollback_check<C: SyncComponent>(&self, confirmed: Option<&C>, predicted: Option<&C>) -> bool {
        match (confirmed, predicted) {
            (Some(c), Some(p)) => {
                let should = self.should_rollback(c, p);
                if should {
                    debug!(
                        "Should Rollback! Confirmed value {c:?} is different from predicted value {p:?}",
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
            (Some(c), None) => {
                 debug!(
                        "Should Rollback! Confirmed component exists ({c:?}), but predicted value does not exists",
                    );
                    #[cfg(feature = "metrics")]
                    metrics::counter!(format!(
                        "prediction::rollbacks::causes::{}::missing_on_predicted",
                        DebugName::type_name::<C>()
                    ))
                    .increment(1);
                    true
            }
            (None, Some(p)) => {
                debug!(
                    "Should Rollback! Confirmed component does not exist, but predicted value exists ({p:?})",
                );
                #[cfg(feature = "metrics")]
                metrics::counter!(format!(
                    "prediction::rollbacks::causes::{}::missing_on_confirmed",
                    DebugName::type_name::<C>()
                ))
                .increment(1);
                true
            }
            (None, None) => false
        }
    }


    /// Check for rollback on entities that didn't receive an explicit update.
    ///
    /// This is called when `ServerMutateTicks.last_tick` advances and an entity didn't
    /// receive a mutation. Since `ServerMutateTicks.last_tick = T` guarantees we have
    /// complete information for all entities at tick T, we know this entity's value at T
    /// equals its last confirmed value.
    ///
    /// This function:
    /// 1. Compares the last confirmed value with what we predicted at `confirmed_tick`
    /// 2. Marks the last confirmed value as confirmed at `confirmed_tick` in the history
    ///
    /// # Arguments
    /// * `confirmed_tick` - Should be `ServerMutateTicks.last_tick()`
    fn check_rollback_empty_mutate<C: SyncComponent>(
        &self,
        confirmed_tick: Tick,
        entity_mut: &mut FilteredEntityMut,
    ) -> bool {
        let entity = entity_mut.id();
        let name = DebugName::type_name::<C>();
        let _span = trace_span!(
            "check_rollback_empty_mutate",
            ?name,
            %entity,
            ?confirmed_tick
        )
        .entered();
        let Some(mut prediction_history) = entity_mut.get_mut::<PredictionHistory<C>>() else {
            // No history means no predicted value to compare against
            return false;
        };

        // Find the last confirmed value in the history.
        // Since this entity didn't receive a mutation, its confirmed value at `confirmed_tick`
        // is the same as its last explicitly confirmed value.
        let Some(last_confirmed_state) = prediction_history.last_confirmed() else {
            // No confirmed value in history - we can't check for rollback.
            // This can happen for entities that were just spawned and haven't received
            // any replication updates yet.
            trace!("No confirmed value in history for entity {:?}, skipping rollback check", entity);
            return false;
        };

        // Clone the confirmed value to use for comparison and insertion
        let confirmed_value: Option<C> = last_confirmed_state.value().cloned();

        // The predicted value at confirmed_tick
        let predicted_value = prediction_history.get(confirmed_tick);
        let should_rollback = self.should_rollback_check(confirmed_value.as_ref(), predicted_value);

        // Mark this value as confirmed at confirmed_tick.
        // This is safe because we know the value at confirmed_tick = last confirmed value.
        // Use add_confirmed which will insert at the correct position (not overwriting
        // any future confirmed values that might already exist).
        prediction_history.add_confirmed(confirmed_tick, confirmed_value);
        prediction_history.clear_until_tick(confirmed_tick);

        should_rollback
    }

    /// Add the confirmed value to the prediction history, and optionally check for rollback.
    ///
    /// This function:
    /// 1. Always adds the confirmed value to the history (needed for rollback in any mode)
    /// 2. If `check_mismatch` is true, compares with the predicted value and returns true if there's a mismatch
    ///
    /// The confirmed value is stored in the history as `Confirmed`, which means it will be preserved
    /// during rollback (we know the real server value).
    fn add_confirmed_and_check_rollback<C: SyncComponent>(
        &self,
        confirmed_tick: Tick,
        confirmed_component: Option<C>,
        entity_mut: &mut DeferredEntity,
        check_mismatch: bool,
    ) -> bool {
        let entity = entity_mut.id();
        let name = DebugName::type_name::<C>();
        let _span = trace_span!(
            "add_confirmed_and_check_rollback",
            ?name,
            %entity,
            ?confirmed_tick,
            ?check_mismatch
        )
        .entered();

        let Some(mut predicted_history) = entity_mut.get_mut::<PredictionHistory<C>>() else {
            let mut history = PredictionHistory::<C>::default();
            // Mark as confirmed since this came from the server
            history.add_confirmed(confirmed_tick, confirmed_component);
            entity_mut.insert(history);
            // If there was no history, we can't compare, so we should rollback to be safe
            return check_mismatch;
        };

        #[cfg(feature = "metrics")]
        metrics::gauge!(format!(
            "prediction::rollbacks::history::{:?}::num_values",
            DebugName::type_name::<C>()
        ))
        .set(predicted_history.len() as f64);

        // Check for mismatch if requested
        let should_rollback = if check_mismatch {
            let history_value = predicted_history.get(confirmed_tick);
            self.should_rollback_check(confirmed_component.as_ref(), history_value)
        } else {
            false
        };

        // Always add confirmed value to history - this value will be preserved during rollback
        predicted_history.add_confirmed(confirmed_tick, confirmed_component);
        should_rollback
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
        if let Some(state) = history.pop_until_tick(tick) {
            if let Some(v) = state.value() {
                trace!(
                    "Popped value from PredictionHistory<{:?}? at tick {:?}: {:?} for hashing",
                    DebugName::type_name::<C>(),
                    tick,
                    v
                );
                f(v, hasher);
            }
        }
    }
}

pub trait PredictionRegistrationExt<C> {
    /// Enable prediction for this component.
    fn add_prediction(self) -> Self
    where
        C: SyncComponent;

    /// Enables correction for this component, without adding the correction systems.
    ///
    /// This can be useful if you want to implement the Correction logic yourself,
    /// for example if Prediction/Rotation are replicated but Correction/FrameInterpolation are applied
    /// on Transform
    fn enable_correction(self) -> Self
    where
        C: SyncComponent;

    /// Add correction for this component where the interpolation will done using the lerp function
    /// provided by the [`Ease`] trait.
    fn add_linear_correction_fn<D>(self) -> Self
    where
        C: SyncComponent + Diffable<D>,
        D: Ease + Debug + Clone + Default + Send + Sync + 'static;

    /// Add correction for this component where the interpolation will done using the lerp function
    /// provided by the [`Ease`] trait.
    ///
    /// The generic type `D` represents the type of the delta that will be applied to `C` to smooth the
    /// rollback error.
    fn add_correction_fn<D>(self, correction_fn: LerpFn<D>) -> Self
    where
        C: SyncComponent + Diffable<D>,
        D: Ease + Debug + Clone + Default + Send + Sync + 'static;

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
        self.app.register_marker_with::<Predicted>(MarkerConfig {
            priority: 100,
            need_history: true,
        });
        self.app.set_marker_fns::<Predicted, C>(write_history::<C>, remove_history::<C>);
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
        let metadata = registry
            .component_metadata_map
            .get_mut(&ComponentKind::of::<C>())
            .unwrap();
        metadata.replication.as_mut().unwrap().set_predicted(true);
        // metadata.serialization.as_mut().unwrap().add_clone::<C>();
        self
    }

    fn enable_correction(self) -> Self
    where
        C: SyncComponent,
    {
        let has_prediction_registry = self
            .app
            .world()
            .get_resource::<PredictionRegistry>()
            .is_some();
        if !has_prediction_registry {
            return self;
        }
        self.app
            .world_mut()
            .resource_mut::<PredictionRegistry>()
            .enable_correction::<C>();
        self
    }

    fn add_linear_correction_fn<D>(self) -> Self
    where
        C: SyncComponent + Diffable<D>,
        D: Ease + Debug + Clone + Default + Send + Sync + 'static,
    {
        self.add_correction_fn(lerp::<D>)
    }

    fn add_correction_fn<D>(self, correction_fn: LerpFn<D>) -> Self
    where
        C: SyncComponent + Diffable<D>,
        D: Ease + Debug + Clone + Default + Send + Sync + 'static,
    {
        let has_prediction_registry = self
            .app
            .world()
            .get_resource::<PredictionRegistry>()
            .is_some();
        if !has_prediction_registry {
            return self;
        }
        crate::correction::add_correction_systems::<C, D>(self.app);
        self.app
            .world_mut()
            .resource_mut::<PredictionRegistry>()
            .set_correction_fn::<C, D>(correction_fn);
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


// TODO: ideally we would update the LastConfirmedTick at this point?
/// Instead of writing into a component directly, it writes data into [`PredictionHistory<C>`].
///
/// This function:
/// 1. Always adds the confirmed value to the prediction history (needed for rollback in any mode)
/// 2. If `RollbackMode::Check`, also checks for mismatch and records it
fn write_history<C: SyncComponent>(
    ctx: &mut WriteCtx,
    rule_fns: &RuleFns<C>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> Result<()> {
    let component: C = rule_fns.deserialize(ctx, message)?;
    let tick: Tick = ctx.message_tick.get().into();
    // SAFETY: we are not aliasing with the DeferredEntity or Entities
    let registry = unsafe { ctx.world_cell.world() }.resource::<PredictionRegistry>();
    let prediction_link = unsafe { ctx.world_cell.world() }.resource::<PredictionResource>().link_entity;
    let mut metadata = unsafe { ctx.world_cell.world_mut() }.resource_mut::<StateRollbackMetadata>();

    let should_check = unsafe { ctx.world_cell.world() }
        .get::<PredictionManager>(prediction_link)
        .is_some_and(|m| matches!(m.rollback_policy.state, RollbackMode::Check));

    // Always add confirmed values to history (needed for rollback in any mode).
    // If RollbackMode::Check, also check for mismatch.
    let should_rollback = registry.add_confirmed_and_check_rollback(tick, Some(component), entity, should_check);
    if should_rollback {
        metadata.record_mismatch(tick);
    }
    Ok(())
}

/// Removes component `C` and records the removal in history.
///
/// This function:
/// 1. Always adds the confirmed removal to the prediction history (needed for rollback in any mode)
/// 2. If `RollbackMode::Check`, also checks for mismatch and records it
fn remove_history<C: SyncComponent>(ctx: &mut RemoveCtx, entity: &mut DeferredEntity) {
    let tick: Tick = ctx.message_tick.get().into();
    // SAFETY: we are not aliasing with the DeferredEntity
    let registry = unsafe { ctx.world_cell.world() }.resource::<PredictionRegistry>();
    let prediction_link = unsafe { ctx.world_cell.world() }.resource::<PredictionResource>().link_entity;
    let mut metadata = unsafe { ctx.world_cell.world_mut() }.resource_mut::<StateRollbackMetadata>();

    let should_check = unsafe { ctx.world_cell.world() }
        .get::<PredictionManager>(prediction_link)
        .is_some_and(|m| matches!(m.rollback_policy.state, RollbackMode::Check));

    // Always add confirmed removal to history (needed for rollback in any mode).
    // If RollbackMode::Check, also check for mismatch.
    let should_rollback = registry.add_confirmed_and_check_rollback::<C>(tick, None, entity, should_check);
    if should_rollback {
        metadata.record_mismatch(tick);
    }
}
