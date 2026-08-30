use crate::plugin::{add_non_networked_rollback_systems, add_prediction_systems};
use crate::predicted_history::PredictionHistory;
use crate::{SyncComponent, correction};
use bevy_app::App;
use bevy_ecs::component::{ComponentId, Mutable};
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
use bevy_replicon::shared::replication::diff::{ComponentDelta, Diffable as RepliconDiffable};
use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, WriteCtx};
use bevy_replicon::shared::replication::storage::{EntityStorageCtx, ReplicationStorage};
use bevy_utils::prelude::DebugName;
use core::fmt::Debug;
use indexmap::IndexMap;
use lightyear_core::history_buffer::HistoryState;
use lightyear_core::prediction::Predicted;
use lightyear_core::prelude::ConfirmedHistory;
use lightyear_core::tick::Tick;
use lightyear_frame_interpolation::FrameInterpolationPlugin;
use lightyear_replication::checkpoint::resolve_message_tick;
use lightyear_replication::diff_history::HistoryDiffReceiver;
use lightyear_replication::diffable::Diffable;
use lightyear_replication::prelude::{PreSpawned, PredictedSend};
use lightyear_replication::registry::replication::{
    AppComponentExt, ComponentRegistration, ComponentRegistrator,
};
use lightyear_replication::registry::{ComponentError, ComponentKind, ComponentRegistry, LerpFn};
#[cfg(feature = "metrics")]
use std::sync::OnceLock;
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
    /// store `PreviousVisual<C>`, but the user owns the actual correction logic.
    pub(crate) custom_correction: bool,
    /// Type-erased handlers used by the generic post-rollback correction system.
    ///
    /// This is only present for components that use Lightyear's built-in
    /// correction pipeline. Components that call `custom_correction` can
    /// still provide a custom correction pipeline elsewhere.
    pub(crate) correction: Option<correction::ErasedPostRollbackCorrection>,
    /// Function used to compare the confirmed component with the predicted component's history
    /// to determine if a rollback is needed. Returns true if we should do a rollback.
    /// Will default to a PartialEq::ne implementation, but can be overridden.
    pub(crate) should_rollback: unsafe fn(),
    pub(crate) check_rollback: CheckRollbackFn,
    /// For a diff-replicated component on one entity, returns its earliest unresolved diff tick at
    /// or before a candidate completed checkpoint.
    pub(crate) pending_diff_tick: Option<PendingDiffTickFn>,
    #[cfg(feature = "metrics")]
    metric_handles: PredictionMetricHandles,
    #[cfg(feature = "deterministic")]
    /// Function to hash the value in [`PredictionHistory<C>`] at a given tick.
    pub pop_until_tick_and_hash: Option<PopUntilTickAndHashFn>,
}

#[cfg(feature = "metrics")]
#[derive(Debug, Clone, Default)]
struct PredictionMetricHandles {
    history: OnceLock<metrics::Gauge>,
    value_mismatch: OnceLock<metrics::Counter>,
    missing_on_predicted: OnceLock<metrics::Counter>,
    missing_on_confirmed: OnceLock<metrics::Counter>,
}

#[cfg(feature = "metrics")]
impl PredictionMetricHandles {
    fn history<C: SyncComponent>(&self) -> &metrics::Gauge {
        self.history.get_or_init(|| {
            metrics::gauge!(
                "prediction/rollback/history_values",
                "component" => core::any::type_name::<C>(),
            )
        })
    }

    fn value_mismatch<C: SyncComponent>(&self) -> &metrics::Counter {
        self.value_mismatch.get_or_init(|| {
            metrics::counter!(
                "prediction/rollback/causes",
                "component" => core::any::type_name::<C>(),
                "cause" => "value_mismatch",
            )
        })
    }

    fn missing_on_predicted<C: SyncComponent>(&self) -> &metrics::Counter {
        self.missing_on_predicted.get_or_init(|| {
            metrics::counter!(
                "prediction/rollback/causes",
                "component" => core::any::type_name::<C>(),
                "cause" => "missing_on_predicted",
            )
        })
    }

    fn missing_on_confirmed<C: SyncComponent>(&self) -> &metrics::Counter {
        self.missing_on_confirmed.get_or_init(|| {
            metrics::counter!(
                "prediction/rollback/causes",
                "component" => core::any::type_name::<C>(),
                "cause" => "missing_on_confirmed",
            )
        })
    }
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

/// Type-erased pending-diff lookup for one predicted diff component on one entity.
pub(crate) type PendingDiffTickFn = fn(&ReplicationStorage, Tick, Entity) -> Option<Tick>;

/// Type-erased function for hashing the value in a [`PredictionHistory<C>`] component at a tick.
/// The function fn should be of type fn(&C, &mut seahash::SeaHasher) and will be called with the
/// value returned by the history buffer lookup.
/// Returns `true` if the history resolves to a component value at that tick and hashes it.
/// Returns `false` if no component value exists at that tick, in which case nothing is hashed.
pub type PopUntilTickAndHashFn = fn(PtrMut, Tick, &mut seahash::SeaHasher, fn()) -> bool;

impl PredictionMetadata {
    fn new<C: SyncComponent>(
        prediction_history_id: ComponentId,
        confirmed_history_id: ComponentId,
    ) -> Self {
        let should_rollback: ShouldRollbackFn<C> = <C as PartialEq>::ne;
        Self {
            prediction_history_id,
            confirmed_history_id,
            custom_correction: false,
            correction: None,
            should_rollback: unsafe {
                core::mem::transmute::<for<'a, 'b> fn(&'a C, &'b C) -> bool, unsafe fn()>(
                    should_rollback,
                )
            },
            check_rollback: PredictionRegistry::check_rollback_at_completed_checkpoint::<C>,
            pending_diff_tick: None,
            #[cfg(feature = "metrics")]
            metric_handles: PredictionMetricHandles::default(),
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
    /// Predicted components in registration order.
    pub prediction_map: IndexMap<ComponentKind, PredictionMetadata>,
}

impl PredictionRegistry {
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

    fn set_pending_diff_tick<C: SyncComponent + RepliconDiffable>(&mut self) {
        self.prediction_map
            .get_mut(&ComponentKind::of::<C>())
            .expect(
                "The component has not been registered for prediction. Did you call `.predict_diff()`?",
            )
            .pending_diff_tick = Some(|storage, candidate, entity| {
            storage
                .get::<HistoryDiffReceiver<C>>(entity)
                .and_then(|receiver| receiver.earliest_pending_tick_at_or_before(candidate))
        });
    }

    fn custom_correction<C: SyncComponent>(&mut self) {
        self.prediction_map
            .get_mut(&ComponentKind::of::<C>())
            .expect(
                "The component has not been registered for prediction. Did you call `.predict()`?",
            )
            .custom_correction = true;
    }

    fn set_correction_fn<C: SyncComponent>(
        &mut self,
        correction: correction::ErasedPostRollbackCorrection,
    ) {
        let metadata = self
            .prediction_map
            .get_mut(&ComponentKind::of::<C>())
            .expect(
                "The component has not been registered for prediction. Did you call `.predict()`?",
            );
        metadata.correction = Some(correction);
    }

    pub(crate) fn post_rollback_corrections(
        &self,
    ) -> impl Iterator<Item = correction::ErasedPostRollbackCorrection> + '_ {
        self.prediction_map
            .values()
            .filter_map(|metadata| metadata.correction)
    }

    pub(crate) fn apply_correction<C: SyncComponent, D: Default>(
        &self,
        error: D,
        ratio: f32,
    ) -> Option<D> {
        self.prediction_map
            .get(&ComponentKind::of::<C>())
            .expect(
                "The component has not been registered for prediction. Did you call `.predict()`?",
            )
            .correction
            .map(|correction| correction.apply_correction(error, ratio))
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
            .is_some_and(|metadata| metadata.custom_correction || metadata.correction.is_some())
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
                    self.prediction_map[&ComponentKind::of::<C>()]
                        .metric_handles
                        .value_mismatch::<C>()
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
                self.prediction_map[&ComponentKind::of::<C>()]
                    .metric_handles
                    .missing_on_predicted::<C>()
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
                self.prediction_map[&ComponentKind::of::<C>()]
                    .metric_handles
                    .missing_on_confirmed::<C>()
                    .increment(1);
                true
            }
            (None, None) => false,
        }
    }

    /// Check rollback for a component at a completed server mutate tick.
    ///
    /// # Safety
    ///
    /// `confirmed_tick` must be a globally completed server mutate tick with no unresolved
    /// predicted diff at or before it. Without those guarantees, the absence of an exact component
    /// sample would not prove that the component was unchanged.
    ///
    /// # Arguments
    /// * `confirmed_tick` - Latest authoritative tick with complete mutate messages.
    unsafe fn check_rollback_at_completed_checkpoint<C: SyncComponent>(
        &self,
        confirmed_tick: Tick,
        entity_mut: &mut FilteredEntityMut,
    ) -> bool {
        let entity = entity_mut.id();
        let name = DebugName::type_name::<C>();
        let _span = trace_span!(
            "check_rollback_at_completed_checkpoint",
            ?name,
            %entity,
            ?confirmed_tick
        )
        .entered();
        let confirmed_value = {
            let Some(mut component_history) = entity_mut.get_mut::<ConfirmedHistory<C>>() else {
                // No confirmed history means no authoritative value to compare against.
                return false;
            };

            let Some(last_confirmed_state) =
                component_history.get_state_at_or_before(confirmed_tick)
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
            // For a diff component, checkpoint selection has already established that no update at
            // or before C remains pending. The authoritative state at C therefore has exactly two
            // possibilities:
            // - a received diff/snapshot was materialized and history already has its exact value;
            // - there was no update at C, so global completion proves the preceding value carried
            //   forward unchanged.
            // `add_unchanged` preserves an exact entry when one exists and records the proven
            // carried-forward state otherwise.
            component_history.add_unchanged(confirmed_tick);
            confirmed_value
        };

        let Some(prediction_history) = entity_mut.get::<PredictionHistory<C>>() else {
            // No prediction history means no predicted state to compare against.
            return false;
        };

        // This is a completion-time consistency check.
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

    /// Add an authoritative value to confirmed history.
    ///
    /// This function:
    /// State mismatch decisions are intentionally deferred to the full scan at a completed
    /// checkpoint. An entity-level confirmation does not mean every predicted component on that
    /// entity was updated; receive-time checks could therefore miss unchanged component
    /// mismatches on the same entity.
    fn record_confirmed<C: SyncComponent>(
        &self,
        confirmed_tick: Tick,
        confirmed_component: Option<C>,
        entity_mut: &mut DeferredEntity,
        current_tick: Tick,
        materialized_initial: bool,
    ) {
        let entity = entity_mut.id();
        let name = DebugName::type_name::<C>();
        let _span = trace_span!(
            "record_confirmed",
            ?name,
            %entity,
            ?confirmed_tick
        )
        .entered();

        let predicted_history = entity_mut.get::<PredictionHistory<C>>();

        // Normally PredictionHistory<C> is added when live C is added. A confirmed
        // insertion on a predicted entity is instead stored in ConfirmedHistory<C>,
        // so the Add<C> observer does not run.
        //
        // prepare_rollback<C> requires PredictionHistory<C> to include the entity.
        // Seed it with the state the client predicted ("absent") so rollback can read
        // the confirmed value and insert C. Do this even if this update does not check
        // for a mismatch, since another component may already have recorded one.
        let never_predicted_insert = confirmed_component.is_some()
            && !materialized_initial
            && predicted_history.is_none()
            && entity_mut.get::<C>().is_none();

        #[cfg(feature = "metrics")]
        if let Some(predicted_history) = predicted_history.as_ref() {
            self.prediction_map[&ComponentKind::of::<C>()]
                .metric_handles
                .history::<C>()
                .set(predicted_history.len() as f64);
        }

        // Always add confirmed value to confirmed history - this value will be preserved during rollback
        trace!(
            target: "lightyear_debug::prediction",
            kind = "confirmed_history_update",
            entity = ?entity,
            component = ?name,
            confirmed_tick = confirmed_tick.0,
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
        if never_predicted_insert {
            // PredictionHistory must remain ordered on the local timeline. A confirmed update can
            // be ahead of the client, so seeding at confirmed_tick could put a future entry before
            // subsequent local predictions. The earlier tick records the known absence without
            // preventing those predictions from being appended in chronological order.
            let seed_tick = confirmed_tick.min(current_tick);
            trace!(
                target: "lightyear_debug::prediction",
                kind = "seed_prediction_history_for_unpredicted_insert",
                entity = ?entity,
                component = ?name,
                confirmed_tick = confirmed_tick.0,
                current_tick = current_tick.0,
                seed_tick = seed_tick.0,
                "seeding PredictionHistory with predicted absence for a confirmed insert of a never-predicted component"
            );
            let mut history = PredictionHistory::<C>::default();
            history.add_state(seed_tick, HistoryState::Removed);
            entity_mut.insert(history);
        }
    }

    /// Type-erased function for hashing the value in a [`PredictionHistory<C>`] at `tick`.
    ///
    /// Returns `true` if the history resolves to a component value at `tick` and hashes it.
    /// Returns `false` if no component value exists at `tick`, in which case nothing is hashed.
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
    ) -> bool {
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
            true
        } else {
            false
        }
    }
}

pub trait PredictionRegistrationExt<C> {
    /// Enable prediction for this component.
    #[deprecated(note = "use `app.component::<C>().predict()` instead")]
    fn add_prediction(self) -> Self
    where
        C: SyncComponent;

    /// Enable prediction for a component replicated with Replicon's diff-based mode.
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

    /// Marks this component as using custom correction logic.
    ///
    /// This stores the pre-rollback visual value, but does not add Lightyear's
    /// built-in correction systems. Use it when correction is applied elsewhere,
    /// for example when `Position`/`Rotation` are predicted but correction and
    /// frame interpolation are applied on `Transform`.
    fn custom_correction(self) -> Self
    where
        C: SyncComponent;

    /// Enables correction for this component, without adding the correction systems.
    #[deprecated(note = "use `custom_correction()` instead")]
    fn enable_correction(self) -> Self
    where
        C: SyncComponent;

    /// Add visual correction for this component using `C` as its own rollback
    /// error type.
    ///
    /// This is the common case for components where `C: Diffable<C>`.
    /// Correction smooths a rollback by storing the pre-rollback visual value
    /// and then decaying the difference between that value and the corrected
    /// state over several frames.
    ///
    /// This does not register an interpolation rule for C. The corrected component
    /// `C` must have an applicable interpolation rule with an interpolation
    /// function when correction runs. That rule may be a component rule for
    /// `C`, or a bundle rule such as `(A, B)` that contains `C`.
    /// Other members of the selected bundle do not need correction enabled or
    /// their own `PreviousVisual`; their repaired predicted frame histories are
    /// still available as inputs when computing the corrected sample for `C`.
    fn add_correction(self) -> Self
    where
        C: SyncComponent + Diffable<C> + Ease + Default;

    /// Add visual correction for this component using linear interpolation on
    /// rollback errors of type `D`.
    ///
    /// Correction smooths a rollback by storing the pre-rollback visual value
    /// and then decaying the difference between that value and the corrected
    /// state over several frames. `D` is the diff type returned by
    /// [`Diffable::diff`] for `C`; the error is decayed from `D::default()` to
    /// the current error using [`Ease`] linear interpolation.
    ///
    /// This does not register an interpolation rule for C. The corrected component
    /// `C` must have an applicable interpolation rule with an interpolation
    /// function when correction runs. That rule may be a component rule for
    /// `C`, or a bundle rule such as `(A, B)` that contains `C`.
    /// Other members of the selected bundle do not need correction enabled or
    /// their own `PreviousVisual`; their repaired predicted frame histories are
    /// still available as inputs when computing the corrected sample for `C`.
    fn add_linear_correction<D>(self) -> Self
    where
        C: SyncComponent + Diffable<D>,
        D: Ease + Debug + Clone + Default + Send + Sync + 'static;

    /// Add visual correction for this component using `correction_fn` to decay
    /// rollback errors of type `D`.
    ///
    /// This is the custom version of [`Self::add_linear_correction`]. It is
    /// useful when the diff type `D` should not use [`Ease`] interpolation, or
    /// when its decay should follow component-specific logic.
    ///
    /// This does not register an interpolation rule for C. The corrected component
    /// `C` must have an applicable interpolation rule with an interpolation
    /// function when correction runs. That rule may be a component rule for
    /// `C`, or a bundle rule such as `(A, B)` that contains `C`.
    /// Other members of the selected bundle do not need correction enabled or
    /// their own `PreviousVisual`; their repaired predicted frame histories are
    /// still available as inputs when computing the corrected sample for `C`.
    fn add_correction_fn<D>(self, correction_fn: LerpFn<D>) -> Self
    where
        C: SyncComponent + Diffable<D>,
        D: Debug + Clone + Default + Send + Sync + 'static;

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

    /// Marks this component as using custom correction logic.
    ///
    /// This stores the pre-rollback visual value, but does not add Lightyear's
    /// built-in correction systems.
    pub fn custom_correction(mut self) -> Self
    where
        C: SyncComponent,
    {
        self.registration = self.registration.custom_correction();
        self
    }

    /// Backwards-compatible spelling for [`Self::custom_correction`].
    #[deprecated(note = "use `.custom_correction()` instead")]
    pub fn enable_correction(self) -> Self
    where
        C: SyncComponent,
    {
        self.custom_correction()
    }

    /// Add visual correction for this component using `C` as its own rollback
    /// error type.
    ///
    /// The component needs an applicable interpolation rule. The frame
    /// interpolation plugin and entity marker used by correction are added
    /// automatically.
    pub fn add_correction(mut self) -> Self
    where
        C: SyncComponent + Diffable<C> + Ease + Default,
    {
        self.registration = self.registration.add_correction();
        self
    }

    /// Add visual correction for this component using linear interpolation on
    /// rollback errors of type `D`.
    ///
    /// The component needs an applicable interpolation rule. The frame
    /// interpolation plugin and entity marker used by correction are added
    /// automatically.
    pub fn add_linear_correction<D>(mut self) -> Self
    where
        C: SyncComponent + Diffable<D>,
        D: Ease + Debug + Clone + Default + Send + Sync + 'static,
    {
        self.registration = self.registration.add_linear_correction::<D>();
        self
    }

    /// Add visual correction for this component using `correction_fn` to decay
    /// rollback errors of type `D`.
    ///
    /// The component needs an applicable interpolation rule. The frame
    /// interpolation plugin and entity marker used by correction are added
    /// automatically.
    pub fn add_correction_fn<D>(mut self, correction_fn: LerpFn<D>) -> Self
    where
        C: SyncComponent + Diffable<D>,
        D: Debug + Clone + Default + Send + Sync + 'static,
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
    /// diff-based mode.
    fn predict_diff(self) -> PredictedComponentRegistration<'a, C>
    where
        C: SyncComponent + RepliconDiffable;

    /// Enable local rollback for a component or resource that is not handled
    /// by Replicon's prediction marker writes.
    fn local_rollback(self) -> LocalRollbackComponentRegistration<'a, C>
    where
        C: Component<Mutability = Mutable> + Clone;
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

    fn local_rollback(self) -> LocalRollbackComponentRegistration<'a, C>
    where
        C: Component<Mutability = Mutable> + Clone,
    {
        LocalRollbackComponentRegistration::new(add_local_rollback_systems(
            self.into_component_registration(),
        ))
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
        register_prediction_metadata::<C>(self.app);
        // Only `CatchUpGated` routes replicated component state into history.
        // Initial values for non-gated deterministic entities should use the
        // default Replicon write path.
        //
        // Replicon chooses the receive function before applying incoming
        // components. `CatchUpGated` is therefore the marker that can catch the
        // component write while the entity is awaiting catch-up.
        //
        // `DeterministicPredicted` is intentionally not registered here. Outside
        // catch-up there is no forced state rollback that would insert a
        // `ConfirmedHistory<C>` value back into the live entity.
        use crate::rollback::CatchUpGated;
        self.app.set_marker_fns::<CatchUpGated, C>(
            write_initial_live_and_history::<C>,
            remove_history::<C>,
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
        self.app
            .set_marker_fns::<Predicted, C>(write_history::<C>, remove_history::<C>);
        self.app.set_marker_fns::<PredictedSend, C>(
            write_initial_live_and_history::<C>,
            remove_history::<C>,
        );
        // A prespawned entity can receive replicated component data before the
        // server match has inserted `Predicted`. Keep that authoritative data in
        // history so it cannot overwrite the live locally-predicted component.
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
        add_prediction_systems::<C>(self.app);

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
        self.app
            .set_marker_fns::<Predicted, C>(write_history_diff::<C>, remove_history::<C>);
        self.app.set_marker_fns::<PredictedSend, C>(
            write_initial_live_and_history_diff::<C>,
            remove_history::<C>,
        );
        self.app
            .set_marker_fns::<PreSpawned, C>(write_history_diff::<C>, remove_history::<C>);
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
        registry.set_pending_diff_tick::<C>();
        add_prediction_systems::<C>(self.app);
        crate::plugin::add_prediction_diff_systems::<C>(self.app);

        self
    }

    fn custom_correction(self) -> Self
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
            .custom_correction::<C>();
        self
    }

    #[allow(deprecated)]
    fn enable_correction(self) -> Self
    where
        C: SyncComponent,
    {
        self.custom_correction()
    }

    fn add_correction(self) -> Self
    where
        C: SyncComponent + Diffable<C> + Ease + Default,
    {
        self.add_linear_correction::<C>()
    }

    fn add_linear_correction<D>(self) -> Self
    where
        C: SyncComponent + Diffable<D>,
        D: Ease + Debug + Clone + Default + Send + Sync + 'static,
    {
        self.add_correction_fn::<D>(lerp::<D>)
    }

    fn add_correction_fn<D>(self, correction_fn: LerpFn<D>) -> Self
    where
        C: SyncComponent + Diffable<D>,
        D: Debug + Clone + Default + Send + Sync + 'static,
    {
        let has_prediction_registry = self
            .app
            .world()
            .get_resource::<PredictionRegistry>()
            .is_some();
        if !has_prediction_registry {
            return self;
        }
        if !self.app.is_plugin_added::<FrameInterpolationPlugin>() {
            self.app.add_plugins(FrameInterpolationPlugin);
        }
        let correction_fn = correction::ErasedPostRollbackCorrection::new::<C, D>(
            self.app.world_mut(),
            correction_fn,
        );
        self.app
            .world_mut()
            .resource_mut::<PredictionRegistry>()
            .set_correction_fn::<C>(correction_fn);
        correction::add_correction_systems::<C, D>(self.app);
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

    #[deprecated(note = "use `app.resource::<R>().local_rollback()` instead")]
    fn add_resource_rollback<R: Resource<Mutability = Mutable> + Clone>(&mut self);
}

fn register_prediction_metadata<C: SyncComponent>(app: &mut App) {
    let prediction_history_id = app.world_mut().register_component::<PredictionHistory<C>>();
    let confirmed_history_id = app.world_mut().register_component::<ConfirmedHistory<C>>();
    if let Some(mut registry) = app.world_mut().get_resource_mut::<PredictionRegistry>() {
        registry.register::<C>(prediction_history_id, confirmed_history_id);
    }
}

fn add_local_rollback_systems<C: Component<Mutability = Mutable> + Clone>(
    registration: ComponentRegistration<'_, C>,
) -> ComponentRegistration<'_, C> {
    if registration
        .app
        .world()
        .get_resource::<PredictionRegistry>()
        .is_some()
    {
        add_non_networked_rollback_systems::<C>(registration.app);

        // Resources live on dedicated entities in Bevy. If the resource was inserted before
        // rollback was registered, its `Add<C>` observer has already run, so backfill the history
        // that marks the resource entity as a rollback participant.
        let resource_entity =
            registration
                .app
                .world()
                .component_id::<C>()
                .and_then(|component_id| {
                    registration
                        .app
                        .world()
                        .resource_entities()
                        .get(component_id)
                });
        if let Some(resource_entity) = resource_entity {
            registration
                .app
                .world_mut()
                .entity_mut(resource_entity)
                .insert_if_new(PredictionHistory::<C>::default());
        }
    }
    registration
}

fn add_local_rollback<C: SyncComponent>(app: &mut App) -> ComponentRegistration<'_, C> {
    if app.world().get_resource::<PredictionRegistry>().is_none() {
        return ComponentRegistration::<C>::new(app);
    }
    register_prediction_metadata::<C>(app);
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

    fn add_resource_rollback<R: Resource<Mutability = Mutable> + Clone>(&mut self) {
        self.resource::<R>().local_rollback();
    }
}

// TODO: ideally we would update the LastConfirmedTick at this point?
/// Instead of writing into a component directly, it writes data into [`ConfirmedHistory<C>`].
///
/// The authoritative value is retained in confirmed history. State mismatch decisions are made
/// later by the full scan at a completed checkpoint.
fn write_history<C: SyncComponent>(
    ctx: &mut WriteCtx,
    rule_fns: &RuleFns<C>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> Result<()> {
    write_history_inner(ctx, rule_fns, entity, message, false)
}

/// Writes a first authoritative value to both the live component and confirmed
/// history.
///
/// This is shared by `PredictedSend` and `CatchUpGated`. It lets a fresh entity
/// materialize normally while keeping an active catch-up or existing predicted
/// entity history-only. Replicon batches the live component and history
/// insertions, so role-marker observers see the complete initial state.
fn write_initial_live_and_history<C: SyncComponent>(
    ctx: &mut WriteCtx,
    rule_fns: &RuleFns<C>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> Result<()> {
    write_history_inner(ctx, rule_fns, entity, message, true)
}

fn write_history_inner<C: SyncComponent>(
    ctx: &mut WriteCtx,
    rule_fns: &RuleFns<C>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
    materialize_initial: bool,
) -> Result<()> {
    let component: C = rule_fns.deserialize(ctx, message)?;
    let materialized_initial = materialize_initial
        && entity.get::<C>().is_none()
        && entity.get::<PredictionHistory<C>>().is_none()
        && entity.get::<ConfirmedHistory<C>>().is_none();
    if materialized_initial {
        entity.insert(component.clone());
    }
    add_confirmed_to_history_inner(
        ctx.message_tick,
        Some(component),
        entity,
        materialized_initial,
    )?;
    Ok(())
}

fn write_history_diff<C: SyncComponent + RepliconDiffable>(
    ctx: &mut WriteCtx,
    _rule_fns: &RuleFns<C>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> Result<()> {
    write_history_diff_inner::<C>(ctx, entity, message, false)
}

fn write_initial_live_and_history_diff<C: SyncComponent + RepliconDiffable>(
    ctx: &mut WriteCtx,
    _rule_fns: &RuleFns<C>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> Result<()> {
    write_history_diff_inner::<C>(ctx, entity, message, true)
}

fn write_history_diff_inner<C: SyncComponent + RepliconDiffable>(
    ctx: &mut WriteCtx,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
    materialize_initial: bool,
) -> Result<()> {
    let Some((tick, diff)) = client_diff_and_tick::<C>(ctx, entity, message)? else {
        return Ok(());
    };
    match diff {
        ComponentDelta::Snapshot {
            index,
            mut component,
        } => {
            C::map_entities(&mut component, ctx);
            let materialized_initial = materialize_initial
                && entity.get::<C>().is_none()
                && entity.get::<PredictionHistory<C>>().is_none()
                && entity.get::<ConfirmedHistory<C>>().is_none();
            if materialized_initial {
                entity.insert(component.clone());
            }
            let receiver = ctx.get_or_default::<HistoryDiffReceiver<C>>();
            receiver.record_cursor(tick, Some(index));
            add_resolved_confirmed_to_history_inner(
                tick,
                Some(component),
                entity,
                materialized_initial,
            );
        }
        ComponentDelta::Diffs { index, diffs } => {
            let receiver = ctx.get_or_default::<HistoryDiffReceiver<C>>();
            receiver.queue_diff(tick, index, diffs)?;
        }
    }

    while let Some((tick, value)) = {
        let receiver = ctx.get_or_default::<HistoryDiffReceiver<C>>();
        entity
            .get::<ConfirmedHistory<C>>()
            .map(|history| receiver.take_ready_update(history))
            .transpose()?
            .flatten()
    } {
        add_resolved_confirmed_to_history(tick, Some(value), entity);
    }
    Ok(())
}

/// Decode the raw Replicon diff bytes and map the Replicon message tick to the
/// corresponding Lightyear server tick.
fn client_diff_and_tick<C: SyncComponent + RepliconDiffable>(
    ctx: &mut WriteCtx,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> Result<Option<(Tick, ComponentDelta<C>)>> {
    let diff: ComponentDelta<C> = postcard_utils::from_buf(message)?;
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

fn add_confirmed_to_history_inner<C: SyncComponent>(
    message_tick: RepliconTick,
    confirmed_component: Option<C>,
    entity: &mut DeferredEntity,
    materialized_initial: bool,
) -> Result<()> {
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
        return Ok(());
    };
    add_resolved_confirmed_to_history_inner(
        tick,
        confirmed_component,
        entity,
        materialized_initial,
    );
    Ok(())
}

fn add_resolved_confirmed_to_history<C: SyncComponent>(
    tick: Tick,
    confirmed_component: Option<C>,
    entity: &mut DeferredEntity,
) {
    add_resolved_confirmed_to_history_inner(tick, confirmed_component, entity, false)
}

fn add_resolved_confirmed_to_history_inner<C: SyncComponent>(
    tick: Tick,
    confirmed_component: Option<C>,
    entity: &mut DeferredEntity,
    materialized_initial: bool,
) {
    // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access.
    // We extract all needed values and drop the world borrow before using `entity` again.
    let (registry, current_tick) = {
        let world = unsafe { entity.world_mut() };
        let registry = world.resource::<PredictionRegistry>() as *const PredictionRegistry;
        let current_tick = world
            .resource::<lightyear_core::prelude::LocalTimeline>()
            .tick();
        (unsafe { &*registry }, current_tick)
    };
    // Always add confirmed values to history. The completed-checkpoint scan is the only place
    // where authoritative state is compared with prediction history.
    registry.record_confirmed(
        tick,
        confirmed_component,
        entity,
        current_tick,
        materialized_initial,
    );
}

/// Removes component `C` and records the removal in history.
///
/// The removal is retained in confirmed history. State mismatch decisions are made later by the
/// full scan at a completed checkpoint.
fn remove_history<C: SyncComponent>(ctx: &mut RemoveCtx, entity: &mut DeferredEntity) {
    // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access.
    // We extract all needed values and drop the world borrow before using `entity` again.
    let (registry, checkpoints, current_tick) = {
        let world = unsafe { entity.world_mut() };
        let registry = world.resource::<PredictionRegistry>() as *const PredictionRegistry;
        let checkpoints = world
            .resource::<lightyear_replication::checkpoint::ReplicationCheckpointMap>()
            as *const lightyear_replication::checkpoint::ReplicationCheckpointMap;
        let current_tick = world
            .resource::<lightyear_core::prelude::LocalTimeline>()
            .tick();
        // SAFETY: registry lives in the World and won't be moved/dropped during this function
        (
            unsafe { &*registry },
            unsafe { &*checkpoints },
            current_tick,
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

    registry.record_confirmed::<C>(tick, None, entity, current_tick, false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::PredictionManager;
    use crate::plugin::{PredictionMarkerPlugin, PredictionPlugin};
    use alloc::vec::Vec;
    use bevy_ecs::system::RunSystemOnce;
    use bevy_replicon::prelude::{
        AuthMethod, RepliconPlugins, RepliconSharedPlugin, RepliconTick, RuleFns,
    };
    use bevy_replicon::shared::replication::diff::diff_index::DiffIndex;
    use bevy_replicon::shared::replication::registry::ReplicationRegistry;
    use bevy_replicon::shared::replication::registry::test_fns::TestFnsEntityExt;
    use bevy_state::app::StatesPlugin;
    use core::hash::Hasher;
    use lightyear_core::prelude::LocalTimeline;
    use lightyear_interpolation::prelude::{
        InterpolationMarkerPlugin, InterpolationPlugin, InterpolationRegistrationExt,
        InterpolationRegistry,
    };
    use lightyear_replication::checkpoint::ReplicationCheckpointMap;
    use lightyear_replication::prelude::AppComponentExt;
    use lightyear_sync::prelude::{InputTimelineConfig, LocalTimelineSync};
    use serde::{Deserialize, Serialize};

    #[derive(Clone, PartialEq, Debug)]
    struct TestComponent(u32);

    #[derive(Component, Clone, PartialEq, Debug)]
    struct BuilderComponent(u32);

    #[derive(Component, Clone, PartialEq, Debug)]
    struct LocalRollbackComponent(u32);

    #[derive(Component, Clone, Debug)]
    struct LocalRollbackOnlyComponent(u32);

    #[derive(Resource, Clone, Debug, PartialEq)]
    struct LocalRollbackOnlyResource(u32);

    fn prediction_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconSharedPlugin {
                auth_method: AuthMethod::None,
            },
            PredictionMarkerPlugin,
            InterpolationMarkerPlugin,
            InterpolationPlugin,
        ));
        app.init_resource::<PredictionRegistry>();
        app.init_resource::<PredictionManager>();
        app.init_resource::<InputTimelineConfig>();
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
    fn marker_plugin_does_not_enable_prediction() {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconSharedPlugin {
                auth_method: AuthMethod::None,
            },
            PredictionMarkerPlugin,
        ));

        assert!(!app.world().contains_resource::<PredictionRegistry>());
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
        type Diff = u32;

        fn apply_diff(&mut self, diff: &Self::Diff) -> bevy_ecs::error::Result<()> {
            self.0 = *diff;
            Ok(())
        }
    }

    #[derive(Serialize)]
    enum TestComponentDelta<'a> {
        Snapshot {
            index: DiffIndex,
            component: &'a TestDiffComponent,
        },
        Diffs {
            index: DiffIndex,
            diffs: &'a [u32],
        },
    }

    fn diff_snapshot(index: u16, component: TestDiffComponent) -> Bytes {
        let mut message = Vec::new();
        let wire = TestComponentDelta::Snapshot {
            index: DiffIndex::new(index),
            component: &component,
        };
        postcard_utils::to_extend_mut(&wire, &mut message).unwrap();
        message.into()
    }

    fn diff_message(index: u16, diffs: &[u32]) -> Bytes {
        let mut message = Vec::new();
        let wire = TestComponentDelta::Diffs {
            index: DiffIndex::new(index),
            diffs,
        };
        postcard_utils::to_extend_mut(&wire, &mut message).unwrap();
        message.into()
    }

    fn setup_prediction_diff_app() -> (App, bevy_replicon::shared::replication::registry::FnsId) {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconPlugins,
            PredictionMarkerPlugin,
            PredictionPlugin,
        ));
        app.insert_resource(LocalTimeline::default());
        app.insert_resource(ReplicationCheckpointMap::default());
        app.insert_resource(PredictionManager::default());
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
    fn component_builder_local_rollback_supports_non_sync_component() {
        let mut app = prediction_app();

        app.component::<LocalRollbackOnlyComponent>()
            .local_rollback();

        assert!(
            app.world()
                .component_id::<PredictionHistory<LocalRollbackOnlyComponent>>()
                .is_some()
        );
        assert!(
            app.world()
                .component_id::<ConfirmedHistory<LocalRollbackOnlyComponent>>()
                .is_some()
        );
        assert!(
            !app.world()
                .resource::<PredictionRegistry>()
                .predicted::<LocalRollbackOnlyComponent>()
        );
    }

    #[test]
    fn resource_builder_local_rollback_backfills_existing_resource_history() {
        let mut app = prediction_app();
        app.insert_resource(LocalTimeline::default());
        let mut sync = LocalTimelineSync::default();
        sync.set_synced(true);
        app.insert_resource(sync);
        app.insert_resource(LocalRollbackOnlyResource(42));
        app.world_mut()
            .spawn((PredictionManager::default(), InputTimelineConfig::default()));

        app.resource::<LocalRollbackOnlyResource>().local_rollback();
        app.world_mut()
            .run_system_once(
                crate::predicted_history::update_prediction_history::<LocalRollbackOnlyResource>,
            )
            .unwrap();

        assert!(
            app.world()
                .component_id::<PredictionHistory<LocalRollbackOnlyResource>>()
                .is_some()
        );
        assert!(
            app.world()
                .component_id::<ConfirmedHistory<LocalRollbackOnlyResource>>()
                .is_some()
        );
        assert!(
            !app.world()
                .resource::<PredictionRegistry>()
                .predicted::<LocalRollbackOnlyResource>()
        );

        let resource_entity = app
            .world()
            .resource_entities()
            .get(
                app.world()
                    .component_id::<LocalRollbackOnlyResource>()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(
            app.world()
                .get::<PredictionHistory<LocalRollbackOnlyResource>>(resource_entity)
                .unwrap()
                .get_state(Tick(0)),
            Some(&HistoryState::Updated(LocalRollbackOnlyResource(42)))
        );
    }

    #[test]
    fn resource_builder_local_rollback_adds_history_to_late_resource() {
        let mut app = prediction_app();
        app.insert_resource(LocalTimeline::default());
        let mut sync = LocalTimelineSync::default();
        sync.set_synced(true);
        app.insert_resource(sync);
        app.world_mut()
            .spawn((PredictionManager::default(), InputTimelineConfig::default()));

        app.resource::<LocalRollbackOnlyResource>().local_rollback();
        app.insert_resource(LocalRollbackOnlyResource(42));
        app.world_mut().flush();
        app.world_mut()
            .run_system_once(
                crate::predicted_history::update_prediction_history::<LocalRollbackOnlyResource>,
            )
            .unwrap();

        let resource_entity = app
            .world()
            .resource_entities()
            .get(
                app.world()
                    .component_id::<LocalRollbackOnlyResource>()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(
            app.world()
                .get::<PredictionHistory<LocalRollbackOnlyResource>>(resource_entity)
                .unwrap()
                .get_state(Tick(0)),
            Some(&HistoryState::Updated(LocalRollbackOnlyResource(42)))
        );
    }

    #[test]
    fn component_builder_confirmed_write_registers_prediction_metadata() {
        let mut app = prediction_app();

        app.component::<LocalRollbackComponent>()
            .local_rollback()
            .add_confirmed_write();

        assert!(
            app.world()
                .resource::<PredictionRegistry>()
                .predicted::<LocalRollbackComponent>()
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
        let hashed = PredictionRegistry::pop_until_tick_and_hash::<TestComponent>(
            PtrMut::from(&mut history),
            Tick(11),
            &mut hasher,
            hash_fn,
        );

        assert!(hashed);
        assert_ne!(hasher.finish(), 0);
        assert_eq!(history.len(), before_len);
        assert_eq!(history.get(Tick(10)).unwrap().0, 10);
        assert_eq!(history.get(Tick(11)).unwrap().0, 11);
        assert_eq!(history.get(Tick(12)).unwrap().0, 12);
    }

    #[test]
    fn deterministic_hash_reports_missing_prediction_history_value() {
        let mut history = PredictionHistory::<TestComponent>::default();
        let mut hasher = seahash::SeaHasher::default();
        let initial_hash = hasher.finish();
        let hash_fn = unsafe {
            core::mem::transmute::<fn(&TestComponent, &mut seahash::SeaHasher), fn()>(
                hash_test_component,
            )
        };

        let hashed = PredictionRegistry::pop_until_tick_and_hash::<TestComponent>(
            PtrMut::from(&mut history),
            Tick(11),
            &mut hasher,
            hash_fn,
        );

        assert!(!hashed);
        assert_eq!(hasher.finish(), initial_hash);
    }

    #[test]
    fn diff_prediction_buffers_newer_diff_until_older_base_arrives() {
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
            .apply_write(diff_message(5, &[4, 5]), fns_id, tick5);
        {
            let entity_ref = app.world().entity(entity);
            let history = entity_ref
                .get::<ConfirmedHistory<TestDiffComponent>>()
                .unwrap();
            assert!(history.get_state_at(Tick(5)).is_none());
        }

        app.world_mut()
            .entity_mut(entity)
            .apply_write(diff_message(3, &[1, 2, 3]), fns_id, tick3);

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
