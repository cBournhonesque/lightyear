use crate::SyncComponent;
use crate::manager::{PredictionResource, RollbackMode, StateRollbackMetadata};
use crate::plugin::{
    add_non_networked_rollback_systems, add_prediction_systems, add_resource_rollback_systems,
};
use crate::predicted_history::PredictionHistory;
use crate::prelude::PredictionManager;
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
use bevy_replicon::bytes::Bytes;
use bevy_replicon::prelude::{AppMarkerExt, RepliconTick, RuleFns};
use bevy_replicon::shared::replication::deferred_entity::DeferredEntity;
use bevy_replicon::shared::replication::receive_markers::MarkerConfig;
use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, WriteCtx};
use bevy_utils::prelude::DebugName;
use core::fmt::Debug;
use lightyear_core::prediction::Predicted;
use lightyear_core::tick::Tick;
use lightyear_replication::checkpoint::resolve_message_tick;
use lightyear_replication::delta::Diffable;
use lightyear_replication::prelude::PreSpawned;
use lightyear_replication::registry::replication::ComponentRegistration;
use lightyear_replication::registry::{ComponentError, ComponentKind, ComponentRegistry, LerpFn};
use lightyear_utils::collections::HashMap;
use tracing::{debug, error, trace, trace_span};

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
    /// Function to hash the value in [`PredictionHistory<C>`] at a given tick.
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

/// Type-erased function for hashing the value in a [`PredictionHistory<C>`] component at a tick.
/// The function fn should be of type fn(&C, &mut seahash::SeaHasher) and will be called with the
/// value returned by [`PredictionHistory::get`].
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
    fn oldest_retained_tick<C>(history: &PredictionHistory<C>) -> Option<Tick> {
        history.oldest().map(|(tick, _)| *tick)
    }

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

    pub fn should_rollback_check<C: SyncComponent>(
        &self,
        confirmed: Option<&C>,
        predicted: Option<&C>,
    ) -> bool {
        match (confirmed, predicted) {
            (Some(c), Some(p)) => {
                let should = self.should_rollback(c, p);
                if should {
                    debug!(
                        "Should Rollback! Confirmed value {c:?} is different from predicted value {p:?}",
                    );
                    trace!(
                        target: "lightyear_debug::prediction",
                        kind = "rollback_value_mismatch",
                        component = ?DebugName::type_name::<C>(),
                        confirmed = ?c,
                        predicted = ?p,
                        "confirmed value differs from prediction history"
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
                trace!(
                    target: "lightyear_debug::prediction",
                    kind = "rollback_missing_on_predicted",
                    component = ?DebugName::type_name::<C>(),
                    confirmed = ?c,
                    "confirmed component missing from prediction history"
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
                trace!(
                    target: "lightyear_debug::prediction",
                    kind = "rollback_missing_on_confirmed",
                    component = ?DebugName::type_name::<C>(),
                    predicted = ?p,
                    "predicted component missing from confirmed state"
                );
                #[cfg(feature = "metrics")]
                metrics::counter!(format!(
                    "prediction::rollbacks::causes::{}::missing_on_confirmed",
                    DebugName::type_name::<C>()
                ))
                .increment(1);
                true
            }
            (None, None) => false,
        }
    }

    /// Check for rollback on entities that didn't receive an explicit update.
    ///
    /// This is called when the completed mutate tick advances and an entity didn't
    /// receive a mutation. Since completed mutate tick T guarantees we have
    /// complete information for all entities at tick T, we know this entity's value at T
    /// equals its last confirmed value.
    ///
    /// This function:
    /// 1. Compares the last confirmed value with what we predicted at `confirmed_tick`
    /// 2. Marks the last confirmed value as confirmed at `confirmed_tick` in the history
    ///
    /// # Arguments
    /// * `confirmed_tick` - Latest authoritative tick with complete mutate messages.
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
            trace!(
                "No confirmed value in history for entity {:?}, skipping rollback check",
                entity
            );
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
            trace!(
                target: "lightyear_debug::prediction",
                kind = "confirmed_history_insert",
                entity = ?entity,
                component = ?name,
                confirmed_tick = confirmed_tick.0,
                check_mismatch,
                should_rollback = check_mismatch,
                value = ?confirmed_component.as_ref(),
                "created prediction history from confirmed value"
            );
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

        // Check for mismatch if requested. Authoritative mutations can be
        // applied out of order when Replicon keeps marker history enabled:
        // after rollback preparation prunes history at a newer confirmed tick,
        // an older mutation may still be delivered. We should keep the
        // confirmed sample, but a pruned prediction cannot prove a mismatch.
        let oldest_retained_tick = Self::oldest_retained_tick(&predicted_history);
        let history_was_pruned_past_confirmed =
            oldest_retained_tick.is_some_and(|oldest_tick| oldest_tick > confirmed_tick);
        let should_rollback = if check_mismatch && !history_was_pruned_past_confirmed {
            let history_value = predicted_history.get(confirmed_tick);
            self.should_rollback_check(confirmed_component.as_ref(), history_value)
        } else {
            false
        };
        if check_mismatch && history_was_pruned_past_confirmed {
            trace!(
                target: "lightyear_debug::prediction",
                kind = "confirmed_history_stale_skip_mismatch",
                entity = ?entity,
                component = ?name,
                confirmed_tick = confirmed_tick.0,
                oldest_retained_tick = oldest_retained_tick.map(|tick| tick.0),
                "skipping rollback check for confirmed tick older than retained prediction history"
            );
        }

        // Always add confirmed value to history - this value will be preserved during rollback
        trace!(
            target: "lightyear_debug::prediction",
            kind = "confirmed_history_update",
            entity = ?entity,
            component = ?name,
            confirmed_tick = confirmed_tick.0,
            check_mismatch,
            should_rollback,
            history_len = predicted_history.len(),
            value = ?confirmed_component.as_ref(),
            "recorded confirmed value in prediction history"
        );
        predicted_history.add_confirmed(confirmed_tick, confirmed_component);
        should_rollback
    }

    /// Type-erased function for hashing the value in a [`PredictionHistory<C>`] at `tick`.
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
        if let Some(v) = history.get(tick) {
            trace!(
                "Read value from PredictionHistory<{:?}> at tick {:?}: {:?} for hashing",
                DebugName::type_name::<C>(),
                tick,
                v
            );
            f(v, hasher);
        }
    }
}

pub trait PredictionRegistrationExt<C> {
    /// Enable prediction for this component.
    fn add_prediction(self) -> Self
    where
        C: SyncComponent;

    /// Register `write_history` as the default replicon receive function for
    /// this component, so that replicated values are stored in
    /// `PredictionHistory<C>` as confirmed state (and optionally trigger a
    /// state rollback) rather than overwriting the component directly.
    ///
    /// Use this alongside `add_rollback` when the component is normally
    /// non-networked (computed from deterministic inputs) but needs an initial
    /// value from replication (e.g. `replicate_once` on a physics component
    /// for late-joining clients).
    ///
    /// Unlike marker-gated write functions, this fires for every replicated
    /// update of the component — including init messages where marker
    /// components haven't been applied yet to the newly-spawned entity.
    fn add_confirmed_write(self) -> Self
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
    fn add_confirmed_write(self) -> Self
    where
        C: SyncComponent,
    {
        if !self.app.world().contains_resource::<PredictionRegistry>() {
            return self;
        }
        // Gate keyed on both `AwaitingCatchUpSnapshot` and
        // `DeterministicPredicted` (backwards-compatible). The Awaiting
        // marker is important for init messages: Replicon chooses the
        // receive function before applying the incoming components, so a
        // late-join catch-up snapshot must be routed to history while the
        // entity is still awaiting catch-up, before `DeterministicPredicted`
        // is inserted by user code.
        //
        // - StateBasedCatchUp: while the client is expecting the bundled
        //   snapshot, user code inserts `AwaitingCatchUpSnapshot` on the
        //   catch-up-gated entity. Writes land in `PredictionHistory<C>`.
        //   `request_forced_rollback_to_catch_up_tick` removes the marker
        //   once the forced rollback is scheduled.
        //
        // - InputOnly: `DeterministicPredicted` is present from spawn but
        //   `AwaitingCatchUpSnapshot` is never inserted. The initial
        //   `replicate_once` value lands directly on the live component
        //   via the default replicon write path.
        //
        // The `DeterministicPredicted` marker remains registered for older
        // flows where the entity is already deterministic when the
        // authoritative value arrives.
        use crate::rollback::AwaitingCatchUpSnapshot;
        use crate::rollback::DeterministicPredicted;
        self.app
            .register_marker_with::<AwaitingCatchUpSnapshot>(MarkerConfig {
                priority: 110,
                need_history: true,
            });
        self.app.set_marker_fns::<AwaitingCatchUpSnapshot, C>(
            write_history_gated_by_catchup::<C>,
            remove_history_gated_by_catchup::<C>,
        );
        self.app
            .register_marker_with::<DeterministicPredicted>(MarkerConfig {
                priority: 100,
                need_history: true,
            });
        self.app.set_marker_fns::<DeterministicPredicted, C>(
            write_history_gated_by_catchup::<C>,
            remove_history_gated_by_catchup::<C>,
        );
        self
    }

    fn add_prediction(self) -> Self
    where
        C: SyncComponent,
    {
        if !self.app.world().contains_resource::<PredictionRegistry>() {
            trace!(
                "Skipping prediction registration for component {:?} because PredictionPlugin is not present",
                DebugName::type_name::<C>()
            );
            return self;
        }
        self.app.register_marker_with::<Predicted>(MarkerConfig {
            priority: 100,
            need_history: true,
        });
        self.app
            .set_marker_fns::<Predicted, C>(write_history::<C>, remove_history::<C>);
        // A prespawned entity can receive replicated component data before the
        // server match has inserted `Predicted`. Keep that authoritative data in
        // history so it cannot overwrite the live locally-predicted component.
        self.app.register_marker_with::<PreSpawned>(MarkerConfig {
            priority: 100,
            need_history: true,
        });
        self.app
            .set_marker_fns::<PreSpawned, C>(write_history::<C>, remove_history::<C>);
        let history_id = self
            .app
            .world_mut()
            .register_component::<PredictionHistory<C>>();
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
    let (tick, should_rollback) =
        add_confirmed_to_history(ctx.message_tick, Some(component), entity, true)?;
    if should_rollback {
        // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access
        unsafe { entity.world_mut() }
            .resource_mut::<StateRollbackMetadata>()
            .record_mismatch(tick);
    }
    Ok(())
}

fn add_confirmed_to_history<C: SyncComponent>(
    message_tick: RepliconTick,
    confirmed_component: Option<C>,
    entity: &mut DeferredEntity,
    check_state_rollback: bool,
) -> Result<(Tick, bool)> {
    // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access.
    // We extract all needed values and drop the world borrow before using `entity` again.
    let (registry, checkpoints, should_check) = {
        let world = unsafe { entity.world_mut() };
        let registry = world.resource::<PredictionRegistry>() as *const PredictionRegistry;
        let checkpoints = world
            .resource::<lightyear_replication::checkpoint::ReplicationCheckpointMap>()
            as *const lightyear_replication::checkpoint::ReplicationCheckpointMap;
        let prediction_link = world.resource::<PredictionResource>().link_entity;
        let should_check = world
            .get::<PredictionManager>(prediction_link)
            .is_some_and(|m| matches!(m.rollback_policy.state, RollbackMode::Check));
        // SAFETY: registry lives in the World and won't be moved/dropped during this function
        (
            unsafe { &*registry },
            unsafe { &*checkpoints },
            should_check,
        )
    };
    let Some(tick) = resolve_message_tick(checkpoints, message_tick) else {
        error!(
            ?message_tick,
            "missing authoritative checkpoint mapping while writing prediction history"
        );
        debug_assert!(
            false,
            "missing authoritative checkpoint mapping while writing prediction history"
        );
        return Ok((Tick(0), false));
    };

    // Always add confirmed values to history (needed for rollback in any mode).
    // If RollbackMode::Check, also check for mismatch.
    let should_rollback = registry.add_confirmed_and_check_rollback(
        tick,
        confirmed_component,
        entity,
        check_state_rollback && should_check,
    );
    Ok((tick, should_rollback))
}

/// Removes component `C` and records the removal in history.
///
/// This function:
/// 1. Always adds the confirmed removal to the prediction history (needed for rollback in any mode)
/// 2. If `RollbackMode::Check`, also checks for mismatch and records it
fn remove_history<C: SyncComponent>(ctx: &mut RemoveCtx, entity: &mut DeferredEntity) {
    // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access.
    // We extract all needed values and drop the world borrow before using `entity` again.
    let (registry, checkpoints, should_check) = {
        let world = unsafe { entity.world_mut() };
        let registry = world.resource::<PredictionRegistry>() as *const PredictionRegistry;
        let checkpoints = world
            .resource::<lightyear_replication::checkpoint::ReplicationCheckpointMap>()
            as *const lightyear_replication::checkpoint::ReplicationCheckpointMap;
        let prediction_link = world.resource::<PredictionResource>().link_entity;
        let should_check = world
            .get::<PredictionManager>(prediction_link)
            .is_some_and(|m| matches!(m.rollback_policy.state, RollbackMode::Check));
        // SAFETY: registry lives in the World and won't be moved/dropped during this function
        (
            unsafe { &*registry },
            unsafe { &*checkpoints },
            should_check,
        )
    };
    let Some(tick) = resolve_message_tick(checkpoints, ctx.message_tick) else {
        error!(
            message_tick = ?ctx.message_tick,
            "missing authoritative checkpoint mapping while removing prediction history"
        );
        debug_assert!(
            false,
            "missing authoritative checkpoint mapping while removing prediction history"
        );
        return;
    };

    // Always add confirmed removal to history (needed for rollback in any mode).
    // If RollbackMode::Check, also check for mismatch.
    let should_rollback =
        registry.add_confirmed_and_check_rollback::<C>(tick, None, entity, should_check);
    if should_rollback {
        // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access
        unsafe { entity.world_mut() }
            .resource_mut::<StateRollbackMetadata>()
            .record_mismatch(tick);
    }
}

/// Variant of [`write_history`] used by `add_confirmed_write`.
///
/// Checks for `AwaitingCatchUpSnapshot` on the entity at write time:
/// - If absent (e.g. InputOnly mode, or post-catch-up), performs a normal
///   replicon default write directly to the live component.
/// - If present (StateBasedCatchUp bundled snapshot en route), records the
///   write in `PredictionHistory<C>` and updates the live component so
///   activation systems can see that the bundled component has landed.
fn write_history_gated_by_catchup<C: SyncComponent>(
    ctx: &mut WriteCtx,
    rule_fns: &RuleFns<C>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> Result<()> {
    if !entity.contains::<crate::rollback::AwaitingCatchUpSnapshot>() {
        if let Some(mut component) = entity.get_mut::<C>() {
            rule_fns.deserialize_in_place(ctx, &mut *component, message)?;
        } else {
            let component: C = rule_fns.deserialize(ctx, message)?;
            entity.insert(component);
        }
        return Ok(());
    }
    let component: C = rule_fns.deserialize(ctx, message)?;
    let live_component = component.clone();
    let (_, should_rollback) =
        add_confirmed_to_history(ctx.message_tick, Some(component), entity, false)?;
    debug_assert!(!should_rollback);
    if let Some(mut component) = entity.get_mut::<C>() {
        *component = live_component;
    } else {
        entity.insert(live_component);
    }
    Ok(())
}

/// Variant of [`remove_history`] used by `add_confirmed_write`.
///
/// Mirrors [`write_history_gated_by_catchup`], but treats removals from
/// deterministic catch-up-gated components as visibility-reset noise.
/// Replicon resends `replicate_once` components by hiding and showing them;
/// the hide half can arrive as a component removal even though the server did
/// not authoritatively remove the simulation component.
fn remove_history_gated_by_catchup<C: SyncComponent>(
    ctx: &mut RemoveCtx,
    entity: &mut DeferredEntity,
) {
    let awaiting_catchup = entity.contains::<crate::rollback::AwaitingCatchUpSnapshot>();
    if awaiting_catchup || entity.contains::<crate::rollback::DeterministicPredicted>() {
        trace!(
            component = ?DebugName::type_name::<C>(),
            entity = ?entity.id(),
            message_tick = ?ctx.message_tick,
            awaiting_catchup,
            "Ignoring deterministic catch-up component removal"
        );
        return;
    }
    entity.remove::<C>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::hash::Hasher;

    #[derive(Clone, PartialEq, Debug)]
    struct TestComponent(u32);

    fn hash_test_component(value: &TestComponent, hasher: &mut seahash::SeaHasher) {
        hasher.write_u32(value.0);
    }

    #[test]
    fn oldest_retained_tick_tracks_history_pruning() {
        let mut history = PredictionHistory::<TestComponent>::default();
        history.add_predicted(Tick(10), Some(TestComponent(10)));
        history.add_predicted(Tick(11), Some(TestComponent(11)));
        history.add_predicted(Tick(12), Some(TestComponent(12)));

        history.clear_until_tick(Tick(11));

        assert_eq!(
            PredictionRegistry::oldest_retained_tick(&history),
            Some(Tick(11))
        );
        assert!(
            PredictionRegistry::oldest_retained_tick(&history).unwrap() > Tick(10),
            "a confirmed update for tick 10 is older than retained prediction history"
        );
    }

    #[test]
    fn deterministic_hash_does_not_prune_prediction_history() {
        let mut history = PredictionHistory::<TestComponent>::default();
        history.add_predicted(Tick(10), Some(TestComponent(10)));
        history.add_predicted(Tick(11), Some(TestComponent(11)));
        history.add_predicted(Tick(12), Some(TestComponent(12)));

        let before_len = history.len();
        let mut hasher = seahash::SeaHasher::default();
        let hash_fn = unsafe {
            core::mem::transmute::<fn(&TestComponent, &mut seahash::SeaHasher), fn()>(
                hash_test_component,
            )
        };
        PredictionRegistry::pop_until_tick_and_hash::<TestComponent>(
            PtrMut::from(&mut history),
            Tick(11),
            &mut hasher,
            hash_fn,
        );

        assert_ne!(hasher.finish(), 0);
        assert_eq!(history.len(), before_len);
        assert_eq!(history.get(Tick(10)).unwrap().0, 10);
        assert_eq!(history.get(Tick(11)).unwrap().0, 11);
        assert_eq!(history.get(Tick(12)).unwrap().0, 12);
    }
}
