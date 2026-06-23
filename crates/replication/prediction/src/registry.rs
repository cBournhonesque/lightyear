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
use bevy_replicon::postcard_utils;
use bevy_replicon::prelude::{AppMarkerExt, RepliconTick, RuleFns};
use bevy_replicon::shared::replication::deferred_entity::DeferredEntity;
use bevy_replicon::shared::replication::diff::{Diffable as RepliconDiffable, WireDiff};
use bevy_replicon::shared::replication::receive_markers::MarkerConfig;
use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, WriteCtx};
use bevy_replicon::shared::replication::storage::EntityStorageCtx;
use bevy_utils::prelude::DebugName;
use core::fmt::Debug;
use lightyear_core::history_buffer::HistoryState;
use lightyear_core::prediction::Predicted;
use lightyear_core::prelude::{ConfirmedHistory, LocalTimeline};
use lightyear_core::tick::Tick;
use lightyear_replication::checkpoint::resolve_message_tick;
use lightyear_replication::delta::Diffable;
use lightyear_replication::diff_history::ConfirmedHistoryPatchReceiver;
use lightyear_replication::prelude::PreSpawned;
use lightyear_replication::registry::replication::{ComponentRegistration, ComponentRegistrator};
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
    pub prediction_history_id: ComponentId,
    /// Id of the [`ConfirmedHistory<C>`] component
    pub confirmed_history_id: ComponentId,
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
type CheckRollbackFn = unsafe fn(
    &PredictionRegistry,
    confirmed_tick: Tick,
    entity_mut: &mut FilteredEntityMut,
) -> bool;

/// Type-erased function for hashing the value in a [`PredictionHistory<C>`] component at a tick.
/// The function fn should be of type fn(&C, &mut seahash::SeaHasher) and will be called with the
/// value returned by [`PredictionHistory::get`].
pub type PopUntilTickAndHashFn = fn(PtrMut, Tick, &mut seahash::SeaHasher, fn());

impl PredictionMetadata {
    fn new<C: SyncComponent>(
        prediction_history_id: ComponentId,
        confirmed_history_id: ComponentId,
    ) -> Self {
        let should_rollback: ShouldRollbackFn<C> = <C as PartialEq>::ne;
        Self {
            prediction_history_id,
            confirmed_history_id,
            correction: false,
            correction_fn: None,
            should_rollback: unsafe {
                core::mem::transmute::<for<'a, 'b> fn(&'a C, &'b C) -> bool, unsafe fn()>(
                    should_rollback,
                )
            },
            check_rollback: PredictionRegistry::check_rollback_for_unchanged_component::<C>,
            #[cfg(feature = "deterministic")]
            pop_until_tick_and_hash: Some(PredictionRegistry::pop_until_tick_and_hash::<C>),
        }
    }
}

/// Function called when comparing the confirmed component value (received from the remote) with the
/// predicted component value (from the local [`PredictionHistory`]).
///
/// In general we use [`PartialEq::ne`] by default, but you can provide your own function with [`PredictedComponentRegistration::with_rollback_condition`] to customize
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

    fn register<C: SyncComponent>(
        &mut self,
        prediction_history_id: ComponentId,
        confirmed_history_id: ComponentId,
    ) {
        let kind = ComponentKind::of::<C>();
        self.prediction_map.entry(kind).or_insert_with(|| {
            PredictionMetadata::new::<C>(prediction_history_id, confirmed_history_id)
        });
    }

    fn set_should_rollback<C: SyncComponent>(&mut self, should_rollback: ShouldRollbackFn<C>) {
        self.prediction_map
            .get_mut(&ComponentKind::of::<C>())
            .expect(
                "The component has not been registered for prediction. Did you call `.predict()`?",
            )
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
            .expect(
                "The component has not been registered for prediction. Did you call `.predict()`?",
            )
            .correction_fn
            .map(|correction_fn| {
                // SAFETY: the correction_fn was registered as a LerpFn<D>
                let lerp_fn =
                    unsafe { core::mem::transmute::<unsafe fn(), LerpFn<D>>(correction_fn) };
                lerp_fn(D::default(), error, r)
            })
    }

    fn enable_correction<C: SyncComponent>(&mut self) {
        self.prediction_map
            .get_mut(&ComponentKind::of::<C>())
            .expect(
                "The component has not been registered for prediction. Did you call `.predict()`?",
            )
            .correction = true;
    }

    fn set_correction_fn<C: SyncComponent, D>(&mut self, correction_fn: LerpFn<D>) {
        let metadata = self
            .prediction_map
            .get_mut(&ComponentKind::of::<C>())
            .expect(
                "The component has not been registered for prediction. Did you call `.predict()`?",
            );
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

    /// Check rollback for a component that was unchanged at a completed server mutate tick.
    ///
    /// A completed mutate tick T guarantees complete information for all replicated
    /// components at T. The caller must already have ruled out entities whose Replicon
    /// [`ConfirmHistory`](lightyear_replication::prelude::ConfirmHistory) contains T,
    /// so this component did not change at T and its state equals its last confirmed
    /// value before T.
    ///
    /// This function:
    /// 1. Compares the authoritative value with what we predicted at `confirmed_tick`.
    /// 2. Materializes an unchanged sample at `confirmed_tick`.
    ///
    /// # Safety
    ///
    /// The caller must know that this entity was not explicitly updated at `confirmed_tick`.
    /// In practice, `confirmed_tick` must be the latest server-completed mutate tick and the
    /// entity's Replicon `ConfirmHistory` must not contain the corresponding Replicon tick.
    ///
    /// # Arguments
    /// * `confirmed_tick` - Latest authoritative tick with complete mutate messages.
    unsafe fn check_rollback_for_unchanged_component<C: SyncComponent>(
        &self,
        confirmed_tick: Tick,
        entity_mut: &mut FilteredEntityMut,
    ) -> bool {
        let entity = entity_mut.id();
        let name = DebugName::type_name::<C>();
        let _span = trace_span!(
            "check_rollback_for_unchanged_component",
            ?name,
            %entity,
            ?confirmed_tick
        )
        .entered();
        let confirmed_value = {
            let Some(mut confirmed_history) = entity_mut.get_mut::<ConfirmedHistory<C>>() else {
                // No confirmed history means no authoritative value to compare against.
                return false;
            };

            let Some(last_confirmed_state) =
                confirmed_history.get_state_at_or_before(confirmed_tick)
            else {
                // No confirmed value in history - we can't check for rollback.
                // This can happen for entities that were just spawned and haven't received
                // any replication updates yet.
                trace!(
                    "No confirmed value in history for entity {:?}, skipping rollback check",
                    entity
                );
                return false;
            };

            let confirmed_value = last_confirmed_state.value().cloned();
            confirmed_history.add_unchanged(confirmed_tick);
            confirmed_value
        };

        let Some(prediction_history) = entity_mut.get::<PredictionHistory<C>>() else {
            // No prediction history means no predicted state to compare against.
            return false;
        };

        // The unchanged-component path is a completion-time consistency check.
        // If the prediction history has no retained state at this tick, we
        // cannot prove a mismatch; this can happen for newly spawned predicted
        // entities whose local history starts after the completed server tick.
        //
        // Do not use `PredictionHistory::get` here: `None` would conflate "no
        // retained sample" with an explicit predicted removal. An explicit
        // [`HistoryState::Removed`] must still be checked and can roll back
        // against a present confirmed value.
        let Some(predicted_state) = prediction_history.get_state(confirmed_tick) else {
            trace!(
                ?entity,
                ?confirmed_tick,
                component = ?name,
                "No predicted state retained for unchanged rollback check"
            );
            return false;
        };
        self.should_rollback_check(confirmed_value.as_ref(), predicted_state.value())
    }

    /// Add the confirmed value to confirmed history, and optionally check for rollback.
    ///
    /// This function:
    /// 1. Always adds the confirmed value to confirmed history.
    /// 2. If `check_mismatch` is true and the tick is already locally checkable,
    ///    compares with the predicted value and returns true if there's a mismatch.
    fn record_confirmed_and_maybe_check<C: SyncComponent>(
        &self,
        confirmed_tick: Tick,
        confirmed_component: Option<C>,
        entity_mut: &mut DeferredEntity,
        check_mismatch: bool,
        current_tick: Tick,
    ) -> bool {
        let entity = entity_mut.id();
        let name = DebugName::type_name::<C>();
        let _span = trace_span!(
            "record_confirmed_and_maybe_check",
            ?name,
            %entity,
            ?confirmed_tick,
            ?check_mismatch
        )
        .entered();

        let predicted_history = entity_mut.get::<PredictionHistory<C>>();

        #[cfg(feature = "metrics")]
        if let Some(predicted_history) = predicted_history.as_ref() {
            metrics::gauge!(format!(
                "prediction::rollbacks::history::{:?}::num_values",
                DebugName::type_name::<C>()
            ))
            .set(predicted_history.len() as f64);
        }

        // Check for mismatch if requested. Authoritative mutations can be
        // applied out of order when Replicon keeps marker history enabled:
        // after rollback preparation prunes history at a newer confirmed tick,
        // an older mutation may still be delivered. We should keep the
        // confirmed sample, but a pruned prediction cannot prove a mismatch.
        let oldest_retained_tick = predicted_history
            .as_ref()
            .and_then(|history| Self::oldest_retained_tick(history));
        let history_was_pruned_past_confirmed =
            oldest_retained_tick.is_some_and(|oldest_tick| oldest_tick > confirmed_tick);
        let should_rollback = if check_mismatch
            && confirmed_tick < current_tick
            && !history_was_pruned_past_confirmed
        {
            let history_value = predicted_history
                .as_ref()
                .and_then(|history| history.get(confirmed_tick));
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
        if check_mismatch && confirmed_tick >= current_tick {
            trace!(
                target: "lightyear_debug::prediction",
                kind = "confirmed_history_future_skip_mismatch",
                entity = ?entity,
                component = ?name,
                confirmed_tick = confirmed_tick.0,
                current_tick = current_tick.0,
                "skipping rollback check until local prediction reaches confirmed tick"
            );
        }
        // Always add confirmed value to confirmed history - this value will be preserved during rollback
        trace!(
            target: "lightyear_debug::prediction",
            kind = "confirmed_history_update",
            entity = ?entity,
            component = ?name,
            confirmed_tick = confirmed_tick.0,
            check_mismatch,
            should_rollback,
            value = ?confirmed_component.as_ref(),
            "recorded confirmed value in confirmed history"
        );
        let confirmed_state = match confirmed_component {
            Some(component) => HistoryState::Updated(component),
            None => HistoryState::Removed,
        };

        if let Some(mut confirmed_history) = entity_mut.get_mut::<ConfirmedHistory<C>>() {
            confirmed_history.insert(confirmed_tick, confirmed_state);
        } else {
            let mut history = ConfirmedHistory::<C>::default();
            history.insert(confirmed_tick, confirmed_state);
            entity_mut.insert(history);
        }
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
    #[deprecated(note = "use `app.component::<C>().predict()` instead")]
    fn add_prediction(self) -> Self
    where
        C: SyncComponent;

    /// Enable prediction for a component replicated with Replicon's patch-based diff mode.
    #[deprecated(note = "use `app.component::<C>().replicate_diff().predict_diff()` instead")]
    fn add_prediction_diff(self) -> Self
    where
        C: SyncComponent + RepliconDiffable;

    /// Register `write_history` as the default replicon receive function for
    /// this component, so that replicated values are stored in
    /// `ConfirmedHistory<C>` as authoritative state (and optionally trigger a
    /// state rollback) rather than overwriting the component directly.
    ///
    /// Use this alongside `local_rollback` when the component is normally
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
    ///
    /// Kept for backwards compatibility. Prefer
    /// [`PredictionBuilderExt::predict`] or
    /// [`PredictionAppRegistrationExt::local_rollback`] followed by
    /// `with_rollback_condition`, so the call order is explicit in the type.
    #[deprecated(
        note = "use `.predict().with_rollback_condition(...)` or `local_rollback::<C>().with_rollback_condition(...)` instead"
    )]
    fn add_should_rollback(self, should_rollback: ShouldRollbackFn<C>) -> Self
    where
        C: SyncComponent;
}

/// Registration state returned after prediction has been enabled for a component.
///
/// New code should prefer:
///
/// ```rust,ignore
/// app.component::<Position>()
///     .predict()
///     .with_rollback_condition(position_should_rollback);
/// ```
///
/// This makes it clear that custom rollback comparison is only meaningful after
/// prediction has been enabled. Most registration extension traits can operate
/// on this builder state directly; [`Self::into_component_registration`] is
/// kept as an escape hatch for custom integrations.
pub struct PredictedComponentRegistration<'a, C> {
    registration: ComponentRegistration<'a, C>,
}

impl<'a, C> PredictedComponentRegistration<'a, C> {
    fn new(registration: ComponentRegistration<'a, C>) -> Self {
        Self { registration }
    }

    /// Add a custom comparison function to determine if we should rollback by
    /// comparing the confirmed component with the predicted component's history.
    #[allow(deprecated)]
    pub fn with_rollback_condition(mut self, should_rollback: ShouldRollbackFn<C>) -> Self
    where
        C: SyncComponent,
    {
        self.registration = self.registration.add_should_rollback(should_rollback);
        self
    }

    /// Backwards-compatible spelling for [`Self::with_rollback_condition`].
    #[deprecated(note = "use `.with_rollback_condition(...)` instead")]
    pub fn should_rollback(self, should_rollback: ShouldRollbackFn<C>) -> Self
    where
        C: SyncComponent,
    {
        self.with_rollback_condition(should_rollback)
    }

    /// Backwards-compatible spelling for [`Self::with_rollback_condition`].
    #[deprecated(note = "use `.with_rollback_condition(...)` instead")]
    pub fn add_should_rollback(self, should_rollback: ShouldRollbackFn<C>) -> Self
    where
        C: SyncComponent,
    {
        self.with_rollback_condition(should_rollback)
    }

    /// Enables correction for this component, without adding the correction systems.
    pub fn enable_correction(mut self) -> Self
    where
        C: SyncComponent,
    {
        self.registration = self.registration.enable_correction();
        self
    }

    /// Add correction for this component where interpolation uses the
    /// [`Ease`] trait.
    pub fn add_linear_correction_fn<D>(mut self) -> Self
    where
        C: SyncComponent + Diffable<D>,
        D: Ease + Debug + Clone + Default + Send + Sync + 'static,
    {
        self.registration = self.registration.add_linear_correction_fn::<D>();
        self
    }

    /// Add correction for this component using a custom interpolation function.
    pub fn add_correction_fn<D>(mut self, correction_fn: LerpFn<D>) -> Self
    where
        C: SyncComponent + Diffable<D>,
        D: Ease + Debug + Clone + Default + Send + Sync + 'static,
    {
        self.registration = self.registration.add_correction_fn::<D>(correction_fn);
        self
    }

    /// Return to the general component registration builder.
    pub fn into_component_registration(self) -> ComponentRegistration<'a, C> {
        self.registration
    }
}

impl<'a, C> ComponentRegistrator<'a, C> for PredictedComponentRegistration<'a, C> {
    fn into_component_registration(self) -> ComponentRegistration<'a, C> {
        self.registration
    }

    fn from_component_registration(registration: ComponentRegistration<'a, C>) -> Self {
        Self::new(registration)
    }
}

/// Extension trait for the new prediction registration builder.
pub trait PredictionBuilderExt<'a, C>: ComponentRegistrator<'a, C> {
    /// Enable prediction and return a state that exposes prediction-only
    /// configuration methods.
    fn predict(self) -> PredictedComponentRegistration<'a, C>
    where
        C: SyncComponent;

    /// Enable prediction for a component replicated with Replicon's
    /// patch-based diff mode.
    fn predict_diff(self) -> PredictedComponentRegistration<'a, C>
    where
        C: SyncComponent + RepliconDiffable;
}

impl<'a, C, R> PredictionBuilderExt<'a, C> for R
where
    R: ComponentRegistrator<'a, C>,
{
    #[allow(deprecated)]
    fn predict(self) -> PredictedComponentRegistration<'a, C>
    where
        C: SyncComponent,
    {
        PredictedComponentRegistration::new(self.into_component_registration().add_prediction())
    }

    #[allow(deprecated)]
    fn predict_diff(self) -> PredictedComponentRegistration<'a, C>
    where
        C: SyncComponent + RepliconDiffable,
    {
        PredictedComponentRegistration::new(
            self.into_component_registration().add_prediction_diff(),
        )
    }
}

/// Registration state returned after local rollback has been enabled for a
/// non-networked component.
pub struct LocalRollbackComponentRegistration<'a, C> {
    registration: ComponentRegistration<'a, C>,
}

impl<'a, C> LocalRollbackComponentRegistration<'a, C> {
    fn new(registration: ComponentRegistration<'a, C>) -> Self {
        Self { registration }
    }

    /// Add a custom comparison function to determine if we should rollback by
    /// comparing the confirmed component with the predicted component's history.
    #[allow(deprecated)]
    pub fn with_rollback_condition(mut self, should_rollback: ShouldRollbackFn<C>) -> Self
    where
        C: SyncComponent,
    {
        self.registration = self.registration.add_should_rollback(should_rollback);
        self
    }

    /// Backwards-compatible spelling for [`Self::with_rollback_condition`].
    #[deprecated(note = "use `.with_rollback_condition(...)` instead")]
    pub fn should_rollback(self, should_rollback: ShouldRollbackFn<C>) -> Self
    where
        C: SyncComponent,
    {
        self.with_rollback_condition(should_rollback)
    }

    /// Backwards-compatible spelling for [`Self::with_rollback_condition`].
    #[deprecated(note = "use `.with_rollback_condition(...)` instead")]
    pub fn add_should_rollback(self, should_rollback: ShouldRollbackFn<C>) -> Self
    where
        C: SyncComponent,
    {
        self.with_rollback_condition(should_rollback)
    }

    /// Route replicated writes into confirmed history while an entity is
    /// waiting for deterministic catch-up.
    pub fn add_confirmed_write(mut self) -> Self
    where
        C: SyncComponent,
    {
        self.registration = self.registration.add_confirmed_write();
        self
    }

    /// Return to the general component registration builder.
    pub fn into_component_registration(self) -> ComponentRegistration<'a, C> {
        self.registration
    }
}

impl<'a, C> ComponentRegistrator<'a, C> for LocalRollbackComponentRegistration<'a, C> {
    fn into_component_registration(self) -> ComponentRegistration<'a, C> {
        self.registration
    }

    fn from_component_registration(registration: ComponentRegistration<'a, C>) -> Self {
        Self::new(registration)
    }
}

impl<C> PredictionRegistrationExt<C> for ComponentRegistration<'_, C> {
    fn add_confirmed_write(self) -> Self
    where
        C: SyncComponent,
    {
        if !self.app.world().contains_resource::<PredictionRegistry>() {
            return self;
        }
        // Only `CatchUpGated` routes replicated component state into history.
        // While it is present, we care about authoritative state replication
        // because the late-join flow will run a forced state rollback and
        // materialize that history onto the live entity. `CatchUpGated` is
        // registered as a marker so it takes precedence over Replicon's default
        // component write while the entity is awaiting catch-up.
        //
        // `DeterministicPredicted` is intentionally not registered here. Outside
        // catch-up there is no forced state rollback that would insert a
        // history-only component, so those one-shot replicated components should
        // use Replicon's normal live write/remove behavior.
        use crate::rollback::CatchUpGated;
        self.app.register_marker_with::<CatchUpGated>(MarkerConfig {
            priority: 110,
            need_history: true,
        });
        self.app
            .set_marker_fns::<CatchUpGated, C>(write_history::<C>, remove_history::<C>);
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
        let prediction_history_id = self
            .app
            .world_mut()
            .register_component::<PredictionHistory<C>>();
        let confirmed_history_id = self
            .app
            .world_mut()
            .register_component::<ConfirmedHistory<C>>();
        let mut registry = self.app.world_mut().resource_mut::<PredictionRegistry>();
        trace!(
            "Adding prediction for component {:?}",
            DebugName::type_name::<C>()
        );
        registry.register::<C>(prediction_history_id, confirmed_history_id);
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

    fn add_prediction_diff(self) -> Self
    where
        C: SyncComponent + RepliconDiffable,
    {
        if !self.app.world().contains_resource::<PredictionRegistry>() {
            trace!(
                "Skipping diff prediction registration for component {:?} because PredictionPlugin is not present",
                DebugName::type_name::<C>()
            );
            return self;
        }
        self.app.register_marker_with::<Predicted>(MarkerConfig {
            priority: 100,
            need_history: true,
        });
        self.app
            .set_marker_fns::<Predicted, C>(write_history_diff::<C>, remove_history_diff::<C>);
        self.app.register_marker_with::<PreSpawned>(MarkerConfig {
            priority: 100,
            need_history: true,
        });
        self.app
            .set_marker_fns::<PreSpawned, C>(write_history_diff::<C>, remove_history_diff::<C>);
        let prediction_history_id = self
            .app
            .world_mut()
            .register_component::<PredictionHistory<C>>();
        let confirmed_history_id = self
            .app
            .world_mut()
            .register_component::<ConfirmedHistory<C>>();
        let mut registry = self.app.world_mut().resource_mut::<PredictionRegistry>();
        trace!(
            "Adding diff prediction for component {:?}",
            DebugName::type_name::<C>()
        );
        registry.register::<C>(prediction_history_id, confirmed_history_id);
        add_prediction_systems::<C>(self.app);
        crate::plugin::add_prediction_diff_systems::<C>(self.app);

        let mut registry = self.app.world_mut().resource_mut::<ComponentRegistry>();
        let metadata = registry
            .component_metadata_map
            .get_mut(&ComponentKind::of::<C>())
            .unwrap();
        metadata.replication.as_mut().unwrap().set_predicted(true);
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
        let prediction_history_id = self
            .app
            .world_mut()
            .register_component::<PredictionHistory<C>>();
        let confirmed_history_id = self
            .app
            .world_mut()
            .register_component::<ConfirmedHistory<C>>();
        // skip if there is no PredictionRegistry (i.e. the PredictionPlugin wasn't added)
        let Some(mut registry) = self
            .app
            .world_mut()
            .get_resource_mut::<PredictionRegistry>()
        else {
            return self;
        };
        registry.register::<C>(prediction_history_id, confirmed_history_id);
        registry.set_should_rollback::<C>(should_rollback);
        self
    }
}

pub trait PredictionAppRegistrationExt {
    /// Enable rollback for a component that is local-only and is not replicated
    /// by Replicon.
    fn local_rollback<C: SyncComponent>(&mut self) -> LocalRollbackComponentRegistration<'_, C>;

    /// Enable rollbacks for a component that is not networked.
    #[deprecated(note = "use `app.local_rollback::<C>()` instead")]
    fn add_rollback<C: SyncComponent>(&mut self) -> ComponentRegistration<'_, C>;

    fn add_resource_rollback<R: Resource + Clone>(&mut self);
}

fn add_local_rollback<C: SyncComponent>(app: &mut App) -> ComponentRegistration<'_, C> {
    let prediction_history_id = app.world_mut().register_component::<PredictionHistory<C>>();
    let confirmed_history_id = app.world_mut().register_component::<ConfirmedHistory<C>>();
    // skip if there is no PredictionRegistry (i.e. the PredictionPlugin wasn't added)
    let Some(mut registry) = app.world_mut().get_resource_mut::<PredictionRegistry>() else {
        return ComponentRegistration::<C>::new(app);
    };
    registry.register::<C>(prediction_history_id, confirmed_history_id);
    add_non_networked_rollback_systems::<C>(app);
    ComponentRegistration::<C>::new(app)
}

impl PredictionAppRegistrationExt for App {
    fn local_rollback<C: SyncComponent>(&mut self) -> LocalRollbackComponentRegistration<'_, C> {
        LocalRollbackComponentRegistration::new(add_local_rollback::<C>(self))
    }

    fn add_rollback<C: SyncComponent>(&mut self) -> ComponentRegistration<'_, C> {
        add_local_rollback::<C>(self)
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
/// Instead of writing into a component directly, it writes data into [`ConfirmedHistory<C>`].
///
/// This function:
/// 1. Always adds the confirmed value to confirmed history (needed for rollback in any mode)
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

fn write_history_diff<C: SyncComponent + RepliconDiffable>(
    ctx: &mut WriteCtx,
    _rule_fns: &RuleFns<C>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> Result<()> {
    let Some((tick, diff)) = client_diff_and_tick::<C>(ctx, entity, message)? else {
        return Ok(());
    };
    match diff {
        WireDiff::Snapshot {
            index,
            mut component,
        } => {
            C::map_entities(&mut component, ctx);
            let receiver = ctx.get_or_default::<ConfirmedHistoryPatchReceiver<C>>();
            receiver.record_cursor(tick, Some(index));
            let should_rollback =
                add_resolved_confirmed_to_history(tick, Some(component), entity, true);
            if should_rollback {
                // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access
                unsafe { entity.world_mut() }
                    .resource_mut::<StateRollbackMetadata>()
                    .record_mismatch(tick);
            }
        }
        WireDiff::Patches { index, patches } => {
            let receiver = ctx.get_or_default::<ConfirmedHistoryPatchReceiver<C>>();
            receiver.queue_patch_diff(tick, index, patches)?;
        }
    }

    while let Some((tick, value)) = {
        let receiver = ctx.get_or_default::<ConfirmedHistoryPatchReceiver<C>>();
        entity
            .get::<ConfirmedHistory<C>>()
            .map(|history| receiver.take_ready_update(history))
            .transpose()?
            .flatten()
    } {
        let should_rollback = add_resolved_confirmed_to_history(tick, Some(value), entity, true);
        if should_rollback {
            // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access
            unsafe { entity.world_mut() }
                .resource_mut::<StateRollbackMetadata>()
                .record_mismatch(tick);
        }
    }
    Ok(())
}

/// Decode the raw Replicon diff bytes and map the Replicon message tick to the
/// corresponding Lightyear server tick.
fn client_diff_and_tick<C: SyncComponent + RepliconDiffable>(
    ctx: &mut WriteCtx,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> Result<Option<(Tick, WireDiff<C>)>> {
    let diff: WireDiff<C> = postcard_utils::from_buf(message)?;
    let checkpoints = {
        // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access.
        let world = unsafe { entity.world_mut() };
        let checkpoints = world
            .resource::<lightyear_replication::checkpoint::ReplicationCheckpointMap>()
            as *const lightyear_replication::checkpoint::ReplicationCheckpointMap;
        unsafe { &*checkpoints }
    };
    let Some(tick) = resolve_message_tick(checkpoints, ctx.message_tick) else {
        error!(
            message_tick = ?ctx.message_tick,
            "missing authoritative checkpoint mapping while writing diff prediction history"
        );
        debug_assert!(
            false,
            "missing authoritative checkpoint mapping while writing diff prediction history"
        );
        return Ok(None);
    };
    Ok(Some((tick, diff)))
}

fn add_confirmed_to_history<C: SyncComponent>(
    message_tick: RepliconTick,
    confirmed_component: Option<C>,
    entity: &mut DeferredEntity,
    check_state_rollback: bool,
) -> Result<(Tick, bool)> {
    let checkpoints = {
        let world = unsafe { entity.world_mut() };
        let checkpoints = world
            .resource::<lightyear_replication::checkpoint::ReplicationCheckpointMap>()
            as *const lightyear_replication::checkpoint::ReplicationCheckpointMap;
        unsafe { &*checkpoints }
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
    let should_rollback =
        add_resolved_confirmed_to_history(tick, confirmed_component, entity, check_state_rollback);
    Ok((tick, should_rollback))
}

fn add_resolved_confirmed_to_history<C: SyncComponent>(
    tick: Tick,
    confirmed_component: Option<C>,
    entity: &mut DeferredEntity,
    check_state_rollback: bool,
) -> bool {
    // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access.
    // We extract all needed values and drop the world borrow before using `entity` again.
    let (registry, should_check, current_tick, state_metadata) = {
        let world = unsafe { entity.world_mut() };
        let registry = world.resource::<PredictionRegistry>() as *const PredictionRegistry;
        let state_metadata =
            world.resource::<StateRollbackMetadata>() as *const StateRollbackMetadata;
        let current_tick = world.resource::<LocalTimeline>().tick();
        let prediction_link = world.resource::<PredictionResource>().link_entity;
        let should_check = world
            .get::<PredictionManager>(prediction_link)
            .is_some_and(|m| matches!(m.rollback_policy.state, RollbackMode::Check));
        (unsafe { &*registry }, should_check, current_tick, unsafe {
            &*state_metadata
        })
    };
    // Always add confirmed values to history (needed for rollback in any mode).
    // If RollbackMode::Check, also check for mismatch unless this tick is
    // already processed or already known mismatched.
    let check_state_rollback =
        check_state_rollback && state_metadata.should_check_mismatch_at(tick);
    registry.record_confirmed_and_maybe_check(
        tick,
        confirmed_component,
        entity,
        check_state_rollback && should_check,
        current_tick,
    )
}

/// Removes component `C` and records the removal in history.
///
/// This function:
/// 1. Always adds the confirmed removal to the prediction history (needed for rollback in any mode)
/// 2. If `RollbackMode::Check`, also checks for mismatch and records it
fn remove_history<C: SyncComponent>(ctx: &mut RemoveCtx, entity: &mut DeferredEntity) {
    // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access.
    // We extract all needed values and drop the world borrow before using `entity` again.
    let (registry, checkpoints, should_check, current_tick, state_metadata) = {
        let world = unsafe { entity.world_mut() };
        let registry = world.resource::<PredictionRegistry>() as *const PredictionRegistry;
        let checkpoints = world
            .resource::<lightyear_replication::checkpoint::ReplicationCheckpointMap>()
            as *const lightyear_replication::checkpoint::ReplicationCheckpointMap;
        let state_metadata =
            world.resource::<StateRollbackMetadata>() as *const StateRollbackMetadata;
        let current_tick = world.resource::<LocalTimeline>().tick();
        let prediction_link = world.resource::<PredictionResource>().link_entity;
        let should_check = world
            .get::<PredictionManager>(prediction_link)
            .is_some_and(|m| matches!(m.rollback_policy.state, RollbackMode::Check));
        // SAFETY: registry lives in the World and won't be moved/dropped during this function
        (
            unsafe { &*registry },
            unsafe { &*checkpoints },
            should_check,
            current_tick,
            unsafe { &*state_metadata },
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
    // If RollbackMode::Check, also check for mismatch unless this tick is
    // already processed or already known mismatched.
    let should_check = should_check && state_metadata.should_check_mismatch_at(tick);
    let should_rollback = registry.record_confirmed_and_maybe_check::<C>(
        tick,
        None,
        entity,
        should_check,
        current_tick,
    );
    if should_rollback {
        // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access
        unsafe { entity.world_mut() }
            .resource_mut::<StateRollbackMetadata>()
            .record_mismatch(tick);
    }
}

fn remove_history_diff<C: SyncComponent + RepliconDiffable>(
    ctx: &mut RemoveCtx,
    entity: &mut DeferredEntity,
) {
    remove_history::<C>(ctx, entity);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::PredictionPlugin;
    use alloc::vec::Vec;
    use bevy_replicon::prelude::{
        AuthMethod, RepliconPlugins, RepliconSharedPlugin, RepliconTick, RuleFns,
    };
    use bevy_replicon::shared::replication::diff::patch_index::PatchIndex;
    use bevy_replicon::shared::replication::registry::ReplicationRegistry;
    use bevy_replicon::shared::replication::registry::test_fns::TestFnsEntityExt;
    use bevy_state::app::StatesPlugin;
    use core::hash::Hasher;
    use lightyear_interpolation::prelude::{InterpolationRegistrationExt, InterpolationRegistry};
    use lightyear_replication::checkpoint::ReplicationCheckpointMap;
    use lightyear_replication::prelude::AppComponentExt;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, PartialEq, Debug)]
    struct TestComponent(u32);

    #[derive(Component, Clone, PartialEq, Debug)]
    struct BuilderComponent(u32);

    #[derive(Component, Clone, PartialEq, Debug)]
    struct LocalRollbackComponent(u32);

    fn prediction_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconSharedPlugin {
                auth_method: AuthMethod::None,
            },
        ));
        app.init_resource::<PredictionRegistry>();
        app
    }

    fn hash_test_component(value: &TestComponent, hasher: &mut seahash::SeaHasher) {
        hasher.write_u32(value.0);
    }

    fn parity_should_rollback(confirmed: &BuilderComponent, predicted: &BuilderComponent) -> bool {
        confirmed.0 % 2 != predicted.0 % 2
    }

    fn local_should_rollback(
        confirmed: &LocalRollbackComponent,
        predicted: &LocalRollbackComponent,
    ) -> bool {
        confirmed.0 / 10 != predicted.0 / 10
    }

    #[test]
    fn predict_builder_enables_prediction_before_rollback_condition() {
        let mut app = prediction_app();

        app.component::<BuilderComponent>()
            .predict()
            .with_rollback_condition(parity_should_rollback);

        let registry = app.world().resource::<PredictionRegistry>();
        assert!(registry.predicted::<BuilderComponent>());
        assert!(!registry.should_rollback(&BuilderComponent(1), &BuilderComponent(3)));
        assert!(registry.should_rollback(&BuilderComponent(1), &BuilderComponent(2)));
    }

    #[test]
    fn predicted_builder_can_add_custom_interpolation() {
        let mut app = prediction_app();

        app.component::<BuilderComponent>()
            .predict()
            .add_custom_interpolation()
            .with_rollback_condition(parity_should_rollback);

        let prediction_registry = app.world().resource::<PredictionRegistry>();
        assert!(prediction_registry.predicted::<BuilderComponent>());

        let interpolation_registry = app.world().resource::<InterpolationRegistry>();
        assert!(interpolation_registry.interpolated::<BuilderComponent>());
        assert!(prediction_registry.should_rollback(&BuilderComponent(1), &BuilderComponent(2)));
    }

    #[test]
    fn interpolated_builder_can_add_prediction() {
        let mut app = prediction_app();

        app.component::<BuilderComponent>()
            .add_custom_interpolation()
            .predict()
            .with_rollback_condition(parity_should_rollback);

        let prediction_registry = app.world().resource::<PredictionRegistry>();
        assert!(prediction_registry.predicted::<BuilderComponent>());

        let interpolation_registry = app.world().resource::<InterpolationRegistry>();
        assert!(interpolation_registry.interpolated::<BuilderComponent>());
        assert!(prediction_registry.should_rollback(&BuilderComponent(1), &BuilderComponent(2)));
    }

    #[test]
    fn local_rollback_builder_registers_non_networked_rollback() {
        let mut app = App::new();
        app.init_resource::<PredictionRegistry>();

        app.local_rollback::<LocalRollbackComponent>()
            .with_rollback_condition(local_should_rollback);

        let registry = app.world().resource::<PredictionRegistry>();
        assert!(registry.predicted::<LocalRollbackComponent>());
        assert!(
            !registry.should_rollback(&LocalRollbackComponent(10), &LocalRollbackComponent(19))
        );
        assert!(registry.should_rollback(&LocalRollbackComponent(10), &LocalRollbackComponent(20)));
        assert!(app.world().get_resource::<ComponentRegistry>().is_none());
    }

    #[derive(Component, Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct TestDiffComponent(u32);

    impl RepliconDiffable for TestDiffComponent {
        type Patch = u32;

        fn apply_patch(&mut self, patch: &Self::Patch) -> bevy_ecs::error::Result<()> {
            self.0 = *patch;
            Ok(())
        }
    }

    #[derive(Serialize)]
    enum TestWireDiff<'a> {
        Snapshot {
            index: PatchIndex,
            component: &'a TestDiffComponent,
        },
        Patches {
            index: PatchIndex,
            patches: &'a [u32],
        },
    }

    fn diff_snapshot(index: u16, component: TestDiffComponent) -> Bytes {
        let mut message = Vec::new();
        let wire = TestWireDiff::Snapshot {
            index: PatchIndex::new(index),
            component: &component,
        };
        postcard_utils::to_extend_mut(&wire, &mut message).unwrap();
        message.into()
    }

    fn diff_patches(index: u16, patches: &[u32]) -> Bytes {
        let mut message = Vec::new();
        let wire = TestWireDiff::Patches {
            index: PatchIndex::new(index),
            patches,
        };
        postcard_utils::to_extend_mut(&wire, &mut message).unwrap();
        message.into()
    }

    fn setup_prediction_diff_app() -> (App, bevy_replicon::shared::replication::registry::FnsId) {
        let mut app = App::new();
        app.add_plugins((StatesPlugin, RepliconPlugins, PredictionPlugin));
        app.insert_resource(LocalTimeline::default());
        app.insert_resource(ReplicationCheckpointMap::default());
        app.world_mut().spawn(PredictionManager::default());
        app.world_mut().flush();
        app.component::<TestDiffComponent>()
            .replicate_diff()
            .predict_diff();

        let fns_id =
            app.world_mut()
                .resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                    let (_, fns_id) =
                        registry.register_rule_fns(world, RuleFns::<TestDiffComponent>::new_diff());
                    fns_id
                });
        (app, fns_id)
    }

    fn record_checkpoint(app: &mut App, tick: u32) -> RepliconTick {
        let replicon_tick = RepliconTick::new(tick);
        app.world_mut()
            .resource_mut::<ReplicationCheckpointMap>()
            .record(replicon_tick, Tick(tick));
        replicon_tick
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

    #[test]
    fn diff_prediction_buffers_newer_patch_until_older_base_arrives() {
        let (mut app, fns_id) = setup_prediction_diff_app();
        let tick0 = record_checkpoint(&mut app, 0);
        let tick3 = record_checkpoint(&mut app, 3);
        let tick5 = record_checkpoint(&mut app, 5);

        let entity = app.world_mut().spawn(Predicted).id();

        app.world_mut().entity_mut(entity).apply_write(
            diff_snapshot(0, TestDiffComponent(0)),
            fns_id,
            tick0,
        );

        app.world_mut()
            .entity_mut(entity)
            .apply_write(diff_patches(5, &[4, 5]), fns_id, tick5);
        {
            let entity_ref = app.world().entity(entity);
            let history = entity_ref
                .get::<ConfirmedHistory<TestDiffComponent>>()
                .unwrap();
            assert!(history.get_state_at(Tick(5)).is_none());
        }

        app.world_mut()
            .entity_mut(entity)
            .apply_write(diff_patches(3, &[1, 2, 3]), fns_id, tick3);

        let entity_ref = app.world().entity(entity);
        let history = entity_ref
            .get::<ConfirmedHistory<TestDiffComponent>>()
            .unwrap();
        assert_eq!(
            history.get_state_at(Tick(3)).and_then(HistoryState::value),
            Some(&TestDiffComponent(3))
        );
        assert_eq!(
            history.get_state_at(Tick(5)).and_then(HistoryState::value),
            Some(&TestDiffComponent(5))
        );
    }
}
