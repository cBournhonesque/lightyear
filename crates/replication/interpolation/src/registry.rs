use crate::SyncComponent;
use crate::interpolate::{
    apply_interpolation_archetype_erased, update_history_archetype_erased,
    update_history_diff_archetype_erased,
};
use crate::plugin::refresh_update_interpolation_system_if_finalized;
use crate::rules::frame_interpolate::{
    CachedFrameInterpolationApply, CachedFrameInterpolationComponent,
    ErasedApplyFrameInterpolationFn, ErasedInsertFrameHistoryFn, ErasedRestoreFrameHistoryFn,
    ErasedUpdateFrameHistoryFn, FrameHistoryComponent, FrameInterpolationFns,
    apply_frame_interpolation_archetype_erased, insert_frame_history,
    restore_frame_history_archetype_erased, update_frame_history_archetype_erased,
};
use crate::rules::{
    CachedInterpolationApply, CachedInterpolationComponent, ErasedApplyInterpolationFn,
    ErasedBackfillConfirmedHistoryFn, ErasedInterpolationFns, ErasedLerpFn, ErasedUpdateHistoryFn,
    InterpolationBundle, InterpolationFns, InterpolationRule, InterpolationRuleConfig,
    InterpolationRuleId, RuleKind, TupleInterpolationBundle, matches_filter,
};
use alloc::vec::Vec;
use bevy_app::App;
use bevy_ecs::archetype::Archetype;
use bevy_ecs::component::{ComponentId, Components};
use bevy_ecs::prelude::*;
use bevy_ecs::query::{QueryFilter, QueryState};
use bevy_math::{
    Curve,
    curve::{Ease, EaseFunction, EasingCurve},
};
use bevy_platform::collections::HashSet;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::client::confirm_history::ConfirmHistory;
use bevy_replicon::postcard_utils;
use bevy_replicon::prelude::{AppMarkerExt, RuleFns};
use bevy_replicon::shared::replication::deferred_entity::DeferredEntity;
use bevy_replicon::shared::replication::diff::{
    ComponentDelta, DiffBuffer, Diffable as RepliconDiffable,
};
use bevy_replicon::shared::replication::receive_markers::MarkerConfig;
use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, WriteCtx};
use bevy_replicon::shared::replication::storage::{EntityStorageCtx, ReplicationStorage};
use bevy_utils::prelude::DebugName;
use core::cmp::Ordering;
use indexmap::IndexMap;
use lightyear_core::history_buffer::HistoryState;
use lightyear_core::prelude::{ConfirmedHistory, FrameInterpolationHistory, Interpolated, Tick};
use lightyear_replication::checkpoint::{ReplicationCheckpointMap, resolve_message_tick};
use lightyear_replication::diff_history::HistoryDiffReceiver;
use lightyear_replication::registry::replication::{ComponentRegistration, ComponentRegistrator};
use lightyear_replication::registry::{ComponentKind, ComponentRegistry, LerpFn};
use tracing::{error, trace};

fn lerp<C: Ease + Clone>(start: C, other: C, t: f32) -> C {
    let curve = EasingCurve::new(start, other, EaseFunction::Linear);
    curve.sample_unchecked(t)
}

const SINGLE_COMPONENT_RULE_PRIORITY: usize = 1;

#[derive(Debug, Clone)]
pub(crate) struct InterpolationRuleComponentIds {
    history_component_id: Option<ComponentId>,
    frame_history_component_id: Option<ComponentId>,
    live_component_id: Option<ComponentId>,
    write_component_ids: Vec<ComponentId>,
    frame_write_component_ids: Vec<ComponentId>,
}

impl InterpolationRuleComponentIds {
    pub(crate) fn for_component<C>(world: &mut World, fns: &InterpolationFns<C>) -> Self
    where
        C: SyncComponent,
    {
        let owns_interpolation_history = fns.owns_interpolation_history();
        let applies_interpolation_component = fns.applies_interpolation_component();
        let owns_frame_history = fns.owns_frame_history();
        let applies_frame_component = fns.applies_frame_component();
        let history_component_id =
            owns_interpolation_history.then(|| world.register_component::<ConfirmedHistory<C>>());
        let frame_history_component_id =
            owns_frame_history.then(|| world.register_component::<FrameInterpolationHistory<C>>());
        let uses_live_component = owns_interpolation_history
            || applies_interpolation_component
            || owns_frame_history
            || applies_frame_component;
        let live_component_id = uses_live_component.then(|| world.register_component::<C>());

        let mut write_component_ids = Vec::new();
        if let Some(history_component_id) = history_component_id {
            write_component_ids.push(history_component_id);
        }
        if (owns_interpolation_history || applies_interpolation_component)
            && let Some(live_component_id) = live_component_id
        {
            write_component_ids.push(live_component_id);
        }

        let mut frame_write_component_ids = Vec::new();
        if let Some(frame_history_component_id) = frame_history_component_id {
            frame_write_component_ids.push(frame_history_component_id);
        }
        if (owns_frame_history || applies_frame_component)
            && let Some(live_component_id) = live_component_id
        {
            frame_write_component_ids.push(live_component_id);
        }

        Self {
            history_component_id,
            frame_history_component_id,
            live_component_id,
            write_component_ids,
            frame_write_component_ids,
        }
    }
}

/// Stores interpolation functions and rule selection metadata.
///
/// The registry is managed by [`crate::plugin::InterpolationPlugin`] and the
/// registration APIs. Most users should not mutate it directly; use
/// [`AppInterpolationExt::interpolate_with`] or the component builder methods
/// such as [`InterpolationRegistrationExt::add_linear_interpolation`].
///
/// # Examples
///
/// Inspect whether a component has been registered for interpolation:
///
/// ```rust,ignore
/// use bevy_ecs::prelude::*;
/// use lightyear_interpolation::prelude::*;
///
/// #[derive(Component, Clone, PartialEq)]
/// struct Position(f32);
///
/// app.interpolate_with::<Position>(InterpolationFns::history_only());
/// ```
#[derive(Resource, Debug, Default)]
pub struct InterpolationRegistry {
    /// All registered interpolation rules in insertion order.
    ///
    /// [`InterpolationRuleId`] is an index into this vector. Equal-priority
    /// rules preserve this order, matching Replicon's "first registered wins"
    /// behavior for ties.
    rules: Vec<InterpolationRule>,
    /// Priority-ordered rule index by rule target.
    ///
    /// The key is the rule target type: a component rule is keyed by `C`, while
    /// a bundle rule is keyed by the tuple type `(A, B, ...)`. This is separate
    /// from the rule's member [`ComponentKind`]s, which are actual ECS
    /// components used for overlap resolution. Each value is a list of rule IDs
    /// sorted from highest to lowest priority, with equal-priority rules kept in
    /// registration order. The outer map preserves first-registration order for
    /// deterministic cache rebuilds.
    ///
    /// For example, an archetype containing components `A` and `B` can select
    /// one rule for `A`, one rule for `B`, and one rule for `(A, B)`. If the
    /// selected `(A, B)` rule has higher priority, the later apply-resolution
    /// pass lets it claim both `A` and `B`, so the individual `A` and `B` apply
    /// rules do not run for that archetype.
    rules_by_kind: IndexMap<RuleKind, Vec<InterpolationRuleId>>,
    /// Component kinds whose Replicon receive marker functions have been installed.
    interpolated_marker_fns: HashSet<ComponentKind>,
    /// Whether plugin finalization has run.
    ///
    /// Rule registration after finalization is rejected so the type-erased
    /// interpolation system has stable access requirements.
    finalized: bool,
}

impl InterpolationRegistry {
    const FINALIZED_RULE_REGISTRATION_ERROR: &'static str =
        "cannot register interpolation rules after InterpolationRegistry has been finalized";

    pub(crate) fn finalize(&mut self) {
        self.finalized = true;
    }

    fn assert_not_finalized(&self) {
        assert!(
            !self.finalized,
            "{}",
            Self::FINALIZED_RULE_REGISTRATION_ERROR
        );
    }

    /// Iterates over component or bundle rule targets that have interpolation rules.
    #[doc(hidden)]
    pub fn rule_kinds(&self) -> impl Iterator<Item = RuleKind> + '_ {
        self.rules_by_kind.keys().copied()
    }

    /// Returns a rule by ID.
    #[doc(hidden)]
    pub fn rule(&self, rule_id: InterpolationRuleId) -> Option<&InterpolationRule> {
        self.rules.get(rule_id.0)
    }

    /// Returns the number of registered rules.
    ///
    /// [`crate::archetypes::InterpolatedArchetypes`] uses this to invalidate
    /// its local cache when rules are registered after the interpolation system
    /// has already run.
    #[doc(hidden)]
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Returns components that need `FrameInterpolationHistory<C>` backfilled
    /// when `FrameInterpolate` is added.
    #[doc(hidden)]
    pub fn frame_history_components(&self) -> impl Iterator<Item = FrameHistoryComponent> + '_ {
        self.rules
            .iter()
            .filter_map(InterpolationRule::frame_history_component)
    }

    /// Returns per-component callbacks for backfilling confirmed history when
    /// `Interpolated` is added to an entity that already has a replicated live
    /// component.
    #[doc(hidden)]
    pub fn confirmed_history_backfill_fns(
        &self,
    ) -> impl Iterator<Item = (ComponentId, ComponentId, ErasedBackfillConfirmedHistoryFn)> + '_
    {
        self.rules.iter().filter_map(|rule| {
            Some((
                rule.fns.live_component_id?,
                rule.fns.history_component_id?,
                rule.fns.backfill_confirmed_history?,
            ))
        })
    }

    /// Returns component IDs that the type-erased interpolation system may write.
    ///
    /// The custom interpolation system param uses this to declare access before
    /// it reads or writes component columns through [`UnsafeWorldCell`].
    pub(crate) fn component_write_ids(&self) -> Vec<ComponentId> {
        let mut ids = Vec::new();
        for rule in &self.rules {
            for component_id in rule.fns.write_component_ids.iter().copied() {
                if !ids.contains(&component_id) {
                    ids.push(component_id);
                }
            }
        }
        ids
    }

    /// Returns component IDs that the type-erased frame interpolation systems may write.
    #[doc(hidden)]
    pub fn frame_component_write_ids(&self) -> Vec<ComponentId> {
        let mut ids = Vec::new();
        for rule in &self.rules {
            if let Some(frame) = &rule.fns.frame {
                for component_id in frame.write_component_ids.iter().copied() {
                    if !ids.contains(&component_id) {
                        ids.push(component_id);
                    }
                }
            }
        }
        ids
    }

    /// Compares two rules using interpolation precedence.
    ///
    /// Higher priority sorts first. Rules with equal priority keep
    /// registration order, with earlier registrations sorting first.
    #[doc(hidden)]
    pub fn cmp_rule_precedence(
        &self,
        lhs: InterpolationRuleId,
        rhs: InterpolationRuleId,
    ) -> Ordering {
        let lhs_priority = self.rule(lhs).map(InterpolationRule::priority);
        let rhs_priority = self.rule(rhs).map(InterpolationRule::priority);
        rhs_priority
            .cmp(&lhs_priority)
            .then_with(|| lhs.index().cmp(&rhs.index()))
    }

    /// Selects the highest-priority matching rule for `kind` on `archetype`.
    ///
    /// Rules are pre-sorted by descending priority and ascending registration
    /// order, so this returns the first matching rule. It only answers
    /// "which rule owns this target on this archetype"; overlap suppression for
    /// live component writes is handled later by
    /// `CachedInterpolatedArchetype::resolve_apply_rules`.
    #[doc(hidden)]
    pub fn select_rule_for_archetype(
        &self,
        components: &Components,
        archetype: &Archetype,
        kind: RuleKind,
    ) -> Option<InterpolationRuleId> {
        self.rules_by_kind
            .get(&kind)?
            .iter()
            .copied()
            .find(|rule_id| {
                self.rules
                    .get(rule_id.0)
                    .is_some_and(|rule| (rule.matches_archetype)(components, archetype))
            })
    }

    /// Builds cached metadata for updating a selected rule's history component.
    ///
    /// The returned [`CachedInterpolationComponent`] is consumed by the
    /// type-erased history update phase. Rules that do not own history do not
    /// need this cache entry; they may still be selected and may still apply
    /// live components through [`Self::cached_apply_component`].
    ///
    /// Returns `None` when:
    ///
    /// - the rule does not own the history phase,
    /// - the `ConfirmedHistory<C>` component is not registered,
    /// - or this archetype does not currently contain the `ConfirmedHistory<C>`
    ///   column.
    ///
    /// The last case is expected for newly-interpolated entities before the
    /// receive marker or backfill observer has inserted history, and for rules
    /// that only apply live components without maintaining history.
    pub(crate) fn cached_history_update_component(
        &self,
        _components: &Components,
        archetype: &Archetype,
        rule_id: InterpolationRuleId,
    ) -> Option<CachedInterpolationComponent> {
        let rule = self.rules.get(rule_id.0)?;
        if !rule.owns_history() {
            return None;
        }
        let kind = rule.members.first().copied()?;
        let history_component_id = rule.fns.history_component_id?;
        if !archetype.contains(history_component_id) {
            return None;
        }
        let history_storage = archetype.get_storage_type(history_component_id)?;
        let live_component_present = rule
            .fns
            .live_component_id
            .is_some_and(|component_id| archetype.contains(component_id));
        Some(CachedInterpolationComponent {
            kind,
            history_component_id,
            history_storage,
            live_component_present,
            update_history: rule.fns.update_history?,
            interpolation: rule.fns.interpolation,
        })
    }

    /// Builds cached apply metadata for `rule_id`.
    ///
    /// The caller has already selected this rule for the archetype and resolved
    /// overlaps with higher-priority bundle/component rules.
    pub(crate) fn cached_apply_component(
        &self,
        rule_id: InterpolationRuleId,
    ) -> Option<CachedInterpolationApply> {
        let rule = self.rules.get(rule_id.0)?;
        if !rule.applies_component() {
            return None;
        }
        Some(CachedInterpolationApply {
            rule_id,
            apply_interpolation: rule.fns.apply_interpolation?,
        })
    }

    /// Builds cached metadata for updating/restoring a selected frame history component.
    #[doc(hidden)]
    pub fn cached_frame_history_component(
        &self,
        components: &Components,
        archetype: &Archetype,
        rule_id: InterpolationRuleId,
    ) -> Option<CachedFrameInterpolationComponent> {
        let rule = self.rules.get(rule_id.0)?;
        let frame = rule.fns.frame.as_ref()?;
        if !frame.owns_history() {
            return None;
        }
        let kind = rule.members.first().copied()?;
        let history_component_id = frame.history_component_id?;
        let live_component_id = rule.fns.live_component_id?;
        let history_component_present = archetype.contains(history_component_id);
        let live_component_present = archetype.contains(live_component_id);
        if !history_component_present && !live_component_present {
            return None;
        }
        let history_storage = history_component_present
            .then(|| archetype.get_storage_type(history_component_id))
            .flatten();
        Some(CachedFrameInterpolationComponent {
            kind,
            history_component_id,
            history_storage,
            history_component_present,
            live_component_id,
            live_component_present,
            update_frame_history: frame.update_history?,
            restore_frame_history: frame.restore_history?,
        })
    }

    /// Builds cached frame apply metadata for `rule_id`.
    #[doc(hidden)]
    pub fn cached_frame_apply_component(
        &self,
        rule_id: InterpolationRuleId,
    ) -> Option<CachedFrameInterpolationApply> {
        let rule = self.rules.get(rule_id.0)?;
        let frame = rule.fns.frame.as_ref()?;
        if !frame.applies_component() {
            return None;
        }
        Some(CachedFrameInterpolationApply {
            rule_id,
            apply_frame_interpolation: frame.apply_interpolation?,
        })
    }

    pub(crate) fn insert_rule<C, F>(
        &mut self,
        fns: InterpolationFns<C>,
        config: InterpolationRuleConfig,
        component_ids: InterpolationRuleComponentIds,
    ) -> InterpolationRuleId
    where
        C: SyncComponent,
        F: QueryFilter + 'static,
    {
        self.assert_not_finalized();
        self.insert_rule_with_update_history::<C, F>(
            fns,
            config,
            Some(update_history_archetype_erased::<C>),
            Some(backfill_confirmed_history::<C>),
            component_ids,
        )
    }

    pub(crate) fn insert_diff_rule<C, F>(
        &mut self,
        fns: InterpolationFns<C>,
        config: InterpolationRuleConfig,
        component_ids: InterpolationRuleComponentIds,
    ) -> InterpolationRuleId
    where
        C: SyncComponent + RepliconDiffable,
        F: QueryFilter + 'static,
    {
        self.assert_not_finalized();
        self.insert_rule_with_update_history::<C, F>(
            fns,
            config,
            Some(update_history_diff_archetype_erased::<C>),
            Some(backfill_confirmed_history_diff::<C>),
            component_ids,
        )
    }

    fn insert_rule_with_update_history<C, F>(
        &mut self,
        fns: InterpolationFns<C>,
        config: InterpolationRuleConfig,
        update_history: Option<ErasedUpdateHistoryFn>,
        backfill_confirmed_history: Option<ErasedBackfillConfirmedHistoryFn>,
        component_ids: InterpolationRuleComponentIds,
    ) -> InterpolationRuleId
    where
        C: SyncComponent,
        F: QueryFilter + 'static,
    {
        let kind = RuleKind::of::<C>();
        let member = ComponentKind::of::<C>();
        let owns_interpolation_history = fns.owns_interpolation_history();
        let applies_interpolation_component = fns.applies_interpolation_component();
        let owns_frame_history = fns.owns_frame_history();
        let applies_frame_component = fns.applies_frame_component();
        let update_history = owns_interpolation_history
            .then_some(update_history)
            .flatten();
        let backfill_confirmed_history = owns_interpolation_history
            .then_some(backfill_confirmed_history)
            .flatten();
        let apply_interpolation = applies_interpolation_component
            .then_some(apply_interpolation_archetype_erased::<C> as ErasedApplyInterpolationFn);
        let update_frame_history = owns_frame_history
            .then_some(update_frame_history_archetype_erased::<C> as ErasedUpdateFrameHistoryFn);
        let restore_frame_history = owns_frame_history
            .then_some(restore_frame_history_archetype_erased::<C> as ErasedRestoreFrameHistoryFn);
        let apply_frame_interpolation = applies_frame_component.then_some(
            apply_frame_interpolation_archetype_erased::<C> as ErasedApplyFrameInterpolationFn,
        );
        let insert_frame_history =
            owns_frame_history.then_some(insert_frame_history::<C> as ErasedInsertFrameHistoryFn);
        let frame = FrameInterpolationFns::new(
            component_ids.frame_history_component_id,
            component_ids.live_component_id,
            component_ids.frame_write_component_ids,
            insert_frame_history,
            update_frame_history,
            restore_frame_history,
            apply_frame_interpolation,
        );
        let fns = ErasedInterpolationFns::from_typed(
            fns,
            update_history,
            backfill_confirmed_history,
            apply_interpolation,
            component_ids.history_component_id,
            component_ids.live_component_id,
            component_ids.write_component_ids,
            frame,
        );
        let rule_id = InterpolationRuleId(self.rules.len());
        self.rules.push(InterpolationRule {
            kind,
            members: alloc::vec![member],
            priority: config.priority,
            fns,
            matches_archetype: matches_filter::<F>,
        });
        self.insert_rule_id_for_kind(kind, rule_id);
        rule_id
    }

    pub(crate) fn insert_bundle_rule<S, F>(
        &mut self,
        fns: InterpolationFns<S>,
        config: InterpolationRuleConfig,
        members: Vec<ComponentKind>,
        write_component_ids: Vec<ComponentId>,
        apply_interpolation: Option<ErasedApplyInterpolationFn>,
        frame_write_component_ids: Vec<ComponentId>,
        apply_frame_interpolation: Option<ErasedApplyFrameInterpolationFn>,
    ) -> InterpolationRuleId
    where
        S: 'static,
        F: QueryFilter + 'static,
    {
        self.assert_not_finalized();
        let kind = RuleKind::of::<S>();
        let frame = FrameInterpolationFns::new(
            None,
            None,
            frame_write_component_ids,
            None,
            None,
            None,
            apply_frame_interpolation,
        );
        let fns = ErasedInterpolationFns::from_typed(
            fns,
            None,
            None,
            apply_interpolation,
            None,
            None,
            write_component_ids,
            frame,
        );
        let rule_id = InterpolationRuleId(self.rules.len());
        self.rules.push(InterpolationRule {
            kind,
            members,
            priority: config.priority,
            fns,
            matches_archetype: matches_filter::<F>,
        });
        self.insert_rule_id_for_kind(kind, rule_id);
        rule_id
    }

    /// Inserts `rule_id` into the per-kind index in precedence order.
    ///
    /// This follows Replicon's rule insertion model: higher-priority rules are
    /// placed before lower-priority rules, while equal-priority rules are
    /// appended after existing equal-priority registrations.
    fn insert_rule_id_for_kind(&mut self, kind: RuleKind, rule_id: InterpolationRuleId) {
        let priority = self.rules[rule_id.0].priority;
        let rules = self.rules_by_kind.entry(kind).or_default();
        let higher_priority_count =
            rules.partition_point(|existing| self.rules[existing.0].priority > priority);
        let equal_priority_count = rules[higher_priority_count..]
            .partition_point(|existing| self.rules[existing.0].priority == priority);
        rules.insert(higher_priority_count + equal_priority_count, rule_id);
    }

    /// Returns `true` if any interpolation rule covers component `C`.
    pub fn interpolated<C: Component>(&self) -> bool {
        let kind = ComponentKind::of::<C>();
        self.rules.iter().any(|rule| rule.members.contains(&kind))
    }

    pub(crate) fn has_interpolation_fn<C: Component>(&self) -> bool {
        let kind = RuleKind::of::<C>();
        self.rules
            .iter()
            .any(|rule| rule.kind == kind && rule.fns.interpolation.is_some())
    }

    /// Returns the highest-priority interpolation function registered for `C`.
    ///
    /// This helper is for custom systems that already know they need a
    /// single-component interpolation function and cannot run through the
    /// per-archetype rule cache. It does not support tuple rules; systems that
    /// need bundle priority should select rules for the current archetype.
    #[doc(hidden)]
    pub fn interpolation_for<C: Component + Clone>(&self) -> Option<LerpFn<C>> {
        self.rules_by_kind
            .get(&RuleKind::of::<C>())?
            .iter()
            .filter_map(|rule_id| self.rules.get(rule_id.0))
            .find_map(|rule| {
                rule.fns
                    .interpolation
                    .map(|interpolation| unsafe { core::mem::transmute(interpolation) })
            })
    }

    pub(crate) fn sample_for_rule<C: Component + Clone>(
        &self,
        rule_id: InterpolationRuleId,
        history: &ConfirmedHistory<C>,
        interpolation_tick: Tick,
        interpolation_overstep: f32,
    ) -> Option<HistoryState<C>> {
        let rule = &self.rules[rule_id.0];
        debug_assert_eq!(rule.kind, RuleKind::of::<C>());
        sample_history_with_interpolation(
            rule.fns.interpolation,
            history,
            interpolation_tick,
            interpolation_overstep,
        )
    }

    pub(crate) fn interpolation_for_rule<S: 'static>(
        &self,
        rule_id: InterpolationRuleId,
    ) -> Option<LerpFn<S>> {
        let rule = &self.rules[rule_id.0];
        debug_assert_eq!(rule.kind, RuleKind::of::<S>());
        rule.fns
            .interpolation
            .map(|interpolation| unsafe { core::mem::transmute(interpolation) })
    }
}

pub(crate) fn sample_history_with_interpolation<C: Component + Clone>(
    interpolation: Option<ErasedLerpFn>,
    history: &ConfirmedHistory<C>,
    interpolation_tick: Tick,
    interpolation_overstep: f32,
) -> Option<HistoryState<C>> {
    let previous_index = (0..history.len())
        .take_while(|i| {
            history
                .get_nth_tick(*i)
                .is_some_and(|tick| tick <= interpolation_tick)
        })
        .last()?;

    let (start_tick, start_state) = history.get_nth_state(previous_index)?;
    let HistoryState::Updated(start) = start_state else {
        return Some(HistoryState::Removed);
    };

    let Some((end_tick, HistoryState::Updated(end))) = history.get_nth_state(previous_index + 1)
    else {
        return Some(HistoryState::Updated(start.clone()));
    };

    let Some(interpolation) = interpolation else {
        return Some(HistoryState::Updated(start.clone()));
    };

    // Clamp rather than extrapolate beyond the newest confirmed value. This
    // makes late packets converge to the freshest server state instead of
    // overshooting when motion changes direction.
    let fraction = (((interpolation_tick - start_tick) as f32 + interpolation_overstep)
        / (end_tick - start_tick) as f32)
        .clamp(0.0, 1.0);
    trace!(
        target: "lightyear_debug::interpolation",
        kind = "confirmed_history_sample",
        component = ?DebugName::type_name::<C>(),
        interpolation_tick = interpolation_tick.0,
        start_tick = start_tick.0,
        end_tick = end_tick.0,
        interpolation_overstep,
        fraction,
        history_len = history.len(),
        "sampled confirmed history for interpolation"
    );
    let interpolation_fn: LerpFn<C> = unsafe { core::mem::transmute(interpolation) };
    Some(HistoryState::Updated(interpolation_fn(
        start.clone(),
        end.clone(),
        fraction,
    )))
}

/// Extension trait for registering interpolation rules on [`App`].
///
/// The API mirrors Replicon's filtered rule registration style: the component
/// type selects the history being managed, `F` selects matching archetypes, and
/// `*_with_priority` variants decide which rule wins when several filters
/// match.
///
/// Marker components are written as filters such as `With<MyMarker>`. They do
/// not require a separate interpolation marker registration step.
///
/// # Examples
///
/// Register a default rule and a marker-filtered override:
///
/// ```rust,ignore
/// use bevy_ecs::prelude::*;
/// use lightyear_interpolation::prelude::*;
///
/// #[derive(Component, Clone, PartialEq)]
/// struct Position(f32);
///
/// #[derive(Component)]
/// struct ProjectileVisuals;
///
/// fn lerp_position(start: Position, end: Position, t: f32) -> Position {
///     Position(start.0 + (end.0 - start.0) * t)
/// }
///
/// app.interpolate_with::<Position>(InterpolationFns::interpolate(lerp_position));
/// app.interpolate_with_priority_filtered::<Position, With<ProjectileVisuals>>(
///     100,
///     InterpolationFns::disabled(),
/// );
/// ```
pub trait AppInterpolationExt {
    /// Registers a full interpolation rule for component `C` using its linear [`Ease`] curve.
    fn linear_interpolate<C>(&mut self) -> &mut Self
    where
        C: SyncComponent + Ease,
    {
        self.linear_interpolate_filtered::<C, ()>()
    }

    /// Registers a full linear interpolation rule for component `C` with explicit priority.
    fn linear_interpolate_with_priority<C>(&mut self, priority: usize) -> &mut Self
    where
        C: SyncComponent + Ease,
    {
        self.linear_interpolate_with_priority_filtered::<C, ()>(priority)
    }

    /// Registers a default-priority full linear interpolation rule for component `C`
    /// and archetype filter `F`.
    fn linear_interpolate_filtered<C, F>(&mut self) -> &mut Self
    where
        C: SyncComponent + Ease,
        F: QueryFilter + 'static,
    {
        self.linear_interpolate_with_priority_filtered::<C, F>(SINGLE_COMPONENT_RULE_PRIORITY)
    }

    /// Registers a full linear interpolation rule for component `C`, archetype
    /// filter `F`, and explicit priority.
    fn linear_interpolate_with_priority_filtered<C, F>(&mut self, priority: usize) -> &mut Self
    where
        C: SyncComponent + Ease,
        F: QueryFilter + 'static,
    {
        self.interpolate_with_priority_filtered::<C, F>(
            priority,
            InterpolationFns::interpolate(lerp::<C>),
        )
    }

    /// Registers a default-priority interpolation rule for component `C`.
    ///
    /// If the selected [`InterpolationFns`] owns history, Lightyear receives
    /// authoritative updates into [`ConfirmedHistory<C>`]. If it owns apply,
    /// Lightyear samples that history and writes the live component during
    /// [`crate::plugin::InterpolationSystems::Prepare`].
    ///
    /// # Examples
    ///
    /// Register the default rule for `Position`:
    ///
    /// ```rust,ignore
    /// use bevy_ecs::prelude::*;
    /// use lightyear_interpolation::prelude::*;
    ///
    /// #[derive(Component, Clone, PartialEq)]
    /// struct Position(f32);
    ///
    /// fn lerp_position(start: Position, end: Position, t: f32) -> Position {
    ///     Position(start.0 + (end.0 - start.0) * t)
    /// }
    ///
    /// app.interpolate_with::<Position>(InterpolationFns::interpolate(lerp_position));
    /// ```
    fn interpolate_with<C>(&mut self, fns: InterpolationFns<C>) -> &mut Self
    where
        C: SyncComponent,
    {
        self.interpolate_filtered_with::<C, ()>(fns)
    }

    /// Registers an interpolation rule for component `C` with explicit priority.
    fn interpolate_with_priority<C>(
        &mut self,
        priority: usize,
        fns: InterpolationFns<C>,
    ) -> &mut Self
    where
        C: SyncComponent,
    {
        self.interpolate_with_priority_filtered::<C, ()>(priority, fns)
    }

    /// Registers a default-priority interpolation rule for component `C` and archetype filter `F`.
    ///
    /// Use [`Self::interpolate_with`] for the default unfiltered rule. Filters
    /// do not receive an automatic priority bonus, so use
    /// [`Self::interpolate_with_priority_filtered`] when a filtered rule should
    /// override a broader rule registered at the same priority.
    ///
    /// # Examples
    ///
    /// Register a rule that applies only to entities with `VisualInterpolation`:
    ///
    /// ```rust,ignore
    /// use bevy_ecs::prelude::*;
    /// use lightyear_interpolation::prelude::*;
    ///
    /// #[derive(Component, Clone, PartialEq)]
    /// struct Position(f32);
    ///
    /// #[derive(Component)]
    /// struct VisualInterpolation;
    ///
    /// fn lerp_position(start: Position, end: Position, t: f32) -> Position {
    ///     Position(start.0 + (end.0 - start.0) * t)
    /// }
    ///
    /// app.interpolate_filtered_with::<Position, With<VisualInterpolation>>(
    ///     InterpolationFns::interpolate(lerp_position),
    /// );
    /// ```
    fn interpolate_filtered_with<C, F>(&mut self, fns: InterpolationFns<C>) -> &mut Self
    where
        C: SyncComponent,
        F: QueryFilter + 'static,
    {
        self.interpolate_with_priority_filtered::<C, F>(SINGLE_COMPONENT_RULE_PRIORITY, fns)
    }

    /// Registers an interpolation rule for component `C`, archetype filter `F`,
    /// and explicit priority.
    fn interpolate_with_priority_filtered<C, F>(
        &mut self,
        priority: usize,
        fns: InterpolationFns<C>,
    ) -> &mut Self
    where
        C: SyncComponent,
        F: QueryFilter + 'static;

    /// Registers a bundle interpolation rule with default bundle priority.
    ///
    /// Lightyear stores each component in its own [`ConfirmedHistory`], then
    /// samples their histories together and calls the tuple interpolation
    /// function when all histories have the same bracketing ticks.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use bevy_ecs::prelude::*;
    /// use lightyear_interpolation::prelude::*;
    ///
    /// #[derive(Component, Clone, PartialEq)]
    /// struct Position(f32);
    /// #[derive(Component, Clone, PartialEq)]
    /// struct Rotation(f32);
    ///
    /// fn interpolate_transform(
    ///     start: (Position, Rotation),
    ///     end: (Position, Rotation),
    ///     t: f32,
    /// ) -> (Position, Rotation) {
    ///     (
    ///         Position(start.0.0 + (end.0.0 - start.0.0) * t),
    ///         Rotation(start.1.0 + (end.1.0 - start.1.0) * t),
    ///     )
    /// }
    ///
    /// app.interpolate_bundle_with::<(Position, Rotation)>(
    ///     InterpolationFns::interpolate(interpolate_transform),
    /// );
    /// ```
    fn interpolate_bundle_with<B>(&mut self, fns: InterpolationFns<B>) -> &mut Self
    where
        B: InterpolationBundle,
    {
        self.interpolate_bundle_filtered_with::<B, ()>(fns)
    }

    /// Registers a bundle interpolation rule with explicit priority.
    fn interpolate_bundle_with_priority<B>(
        &mut self,
        priority: usize,
        fns: InterpolationFns<B>,
    ) -> &mut Self
    where
        B: InterpolationBundle,
    {
        self.interpolate_bundle_with_priority_filtered::<B, ()>(priority, fns)
    }

    /// Registers a bundle interpolation rule for archetype filter `F` with
    /// default bundle priority.
    ///
    /// Use [`Self::interpolate_bundle_with`] for the default unfiltered rule.
    /// Filters do not receive an automatic priority bonus, so use
    /// [`Self::interpolate_bundle_with_priority_filtered`] when a filtered rule
    /// should override a broader rule registered at the same priority.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use bevy_ecs::prelude::*;
    /// use lightyear_interpolation::prelude::*;
    ///
    /// #[derive(Component, Clone, PartialEq)]
    /// struct Position(f32);
    /// #[derive(Component, Clone, PartialEq)]
    /// struct Rotation(f32);
    /// #[derive(Component)]
    /// struct VisualInterpolation;
    ///
    /// fn interpolate_transform(
    ///     start: (Position, Rotation),
    ///     end: (Position, Rotation),
    ///     t: f32,
    /// ) -> (Position, Rotation) {
    ///     (
    ///         Position(start.0.0 + (end.0.0 - start.0.0) * t),
    ///         Rotation(start.1.0 + (end.1.0 - start.1.0) * t),
    ///     )
    /// }
    ///
    /// app.interpolate_bundle_with_priority_filtered::<(Position, Rotation), With<VisualInterpolation>>(
    ///     100,
    ///     InterpolationFns::interpolate(interpolate_transform),
    /// );
    /// ```
    fn interpolate_bundle_filtered_with<B, F>(&mut self, fns: InterpolationFns<B>) -> &mut Self
    where
        B: InterpolationBundle,
        F: QueryFilter + 'static,
    {
        self.interpolate_bundle_with_priority_filtered::<B, F>(B::COMPONENT_COUNT, fns)
    }

    /// Registers a bundle interpolation rule for archetype filter `F` and
    /// explicit priority.
    fn interpolate_bundle_with_priority_filtered<B, F>(
        &mut self,
        priority: usize,
        fns: InterpolationFns<B>,
    ) -> &mut Self
    where
        B: InterpolationBundle,
        F: QueryFilter + 'static;

    /// Registers a default-priority interpolation rule for a diff-replicated component `C`.
    ///
    /// This is equivalent to [`Self::interpolate_with`], but installs the diff
    /// receive path so interpolation history can reconstruct authoritative
    /// values from Replicon diffs.
    ///
    /// # Examples
    ///
    /// Store diff-replicated updates in history and run custom interpolation:
    ///
    /// ```rust,ignore
    /// use bevy_ecs::prelude::*;
    /// use lightyear_interpolation::prelude::*;
    ///
    /// #[derive(Component, Clone, PartialEq)]
    /// struct Position(f32);
    ///
    /// app.interpolate_diff_with::<Position>(InterpolationFns::history_only());
    /// ```
    fn interpolate_diff_with<C>(&mut self, fns: InterpolationFns<C>) -> &mut Self
    where
        C: SyncComponent + RepliconDiffable,
    {
        self.interpolate_diff_filtered_with::<C, ()>(fns)
    }

    /// Registers an interpolation rule for a diff-replicated component `C`
    /// with explicit priority.
    fn interpolate_diff_with_priority<C>(
        &mut self,
        priority: usize,
        fns: InterpolationFns<C>,
    ) -> &mut Self
    where
        C: SyncComponent + RepliconDiffable,
    {
        self.interpolate_diff_with_priority_filtered::<C, ()>(priority, fns)
    }

    /// Registers a default-priority interpolation rule for a diff-replicated
    /// component `C` and filter `F`.
    ///
    /// Use [`Self::interpolate_diff_with`] for the default unfiltered rule.
    /// Filters do not receive an automatic priority bonus, so use
    /// [`Self::interpolate_diff_with_priority_filtered`] when a filtered rule
    /// should override a broader rule registered at the same priority.
    fn interpolate_diff_filtered_with<C, F>(&mut self, fns: InterpolationFns<C>) -> &mut Self
    where
        C: SyncComponent + RepliconDiffable,
        F: QueryFilter + 'static,
    {
        self.interpolate_diff_with_priority_filtered::<C, F>(SINGLE_COMPONENT_RULE_PRIORITY, fns)
    }

    /// Registers an interpolation rule for a diff-replicated component `C`,
    /// filter `F`, and explicit priority.
    fn interpolate_diff_with_priority_filtered<C, F>(
        &mut self,
        priority: usize,
        fns: InterpolationFns<C>,
    ) -> &mut Self
    where
        C: SyncComponent + RepliconDiffable,
        F: QueryFilter + 'static;
}

impl AppInterpolationExt for App {
    fn interpolate_with_priority_filtered<C, F>(
        &mut self,
        priority: usize,
        fns: InterpolationFns<C>,
    ) -> &mut Self
    where
        C: SyncComponent,
        F: QueryFilter + 'static,
    {
        add_interpolation_rule::<C, F>(self, fns, InterpolationRuleConfig { priority });
        self
    }

    fn interpolate_bundle_with_priority_filtered<B, F>(
        &mut self,
        priority: usize,
        fns: InterpolationFns<B>,
    ) -> &mut Self
    where
        B: InterpolationBundle,
        F: QueryFilter + 'static,
    {
        B::add_rule::<F>(self, fns, InterpolationRuleConfig { priority });
        self
    }

    fn interpolate_diff_with_priority_filtered<C, F>(
        &mut self,
        priority: usize,
        fns: InterpolationFns<C>,
    ) -> &mut Self
    where
        C: SyncComponent + RepliconDiffable,
        F: QueryFilter + 'static,
    {
        add_interpolation_diff_rule::<C, F>(self, fns, InterpolationRuleConfig { priority });
        self
    }
}

fn register_interpolated_marker_fns<C: SyncComponent>(app: &mut bevy_app::App) {
    ensure_interpolation_registry(app);
    let kind = ComponentKind::of::<C>();
    let already_registered = {
        let registry = app.world().resource::<InterpolationRegistry>();
        registry.interpolated_marker_fns.contains(&kind)
    };
    if already_registered {
        return;
    }
    app.register_marker_with::<Interpolated>(MarkerConfig {
        priority: 100,
        need_history: true,
    });
    app.set_marker_fns::<Interpolated, C>(write_history::<C>, remove_history::<C>);
    app.world_mut()
        .resource_mut::<InterpolationRegistry>()
        .interpolated_marker_fns
        .insert(kind);
}

fn register_interpolated_diff_marker_fns<C: SyncComponent + RepliconDiffable>(
    app: &mut bevy_app::App,
) {
    ensure_interpolation_registry(app);
    let kind = ComponentKind::of::<C>();
    app.register_marker_with::<Interpolated>(MarkerConfig {
        priority: 100,
        need_history: true,
    });
    app.set_marker_fns::<Interpolated, C>(write_history_diff::<C>, remove_history::<C>);
    app.world_mut()
        .resource_mut::<InterpolationRegistry>()
        .interpolated_marker_fns
        .insert(kind);
}

/// Backfills `ConfirmedHistory<C>` when `Interpolated` is added after the live
/// replicated component was already inserted.
pub(crate) fn backfill_confirmed_history<C: SyncComponent>(
    entity: Entity,
    commands: &mut Commands,
) {
    commands.queue(move |world: &mut World| {
        let Some((component, message_tick)) = ({
            let Ok(entity_ref) = world.get_entity(entity) else {
                return;
            };
            if entity_ref.contains::<ConfirmedHistory<C>>() {
                return;
            }
            let Some(component) = entity_ref.get::<C>() else {
                return;
            };
            let Some(confirm_history) = entity_ref.get::<ConfirmHistory>() else {
                return;
            };
            Some((component.clone(), confirm_history.last_tick()))
        }) else {
            return;
        };

        let Some(checkpoints) = world.get_resource::<ReplicationCheckpointMap>() else {
            debug_assert!(
                false,
                "missing checkpoint map while backfilling ConfirmedHistory"
            );
            return;
        };
        let Some(tick) = checkpoints.get(message_tick) else {
            debug_assert!(
                false,
                "missing authoritative checkpoint mapping while backfilling ConfirmedHistory"
            );
            return;
        };

        let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
            return;
        };
        if entity_mut.contains::<ConfirmedHistory<C>>() {
            return;
        }
        let mut history = ConfirmedHistory::<C>::default();
        history.insert_present(tick, component);
        entity_mut.insert(history);
        entity_mut.remove::<C>();
    });
}

/// Diff-aware variant of [`backfill_confirmed_history`].
pub(crate) fn backfill_confirmed_history_diff<C: SyncComponent + RepliconDiffable>(
    entity: Entity,
    commands: &mut Commands,
) {
    commands.queue(move |world: &mut World| {
        let Some((component, message_tick, insert_history)) = ({
            let Ok(entity_ref) = world.get_entity(entity) else {
                return;
            };
            let Some(component) = entity_ref.get::<C>() else {
                return;
            };
            let Some(confirm_history) = entity_ref.get::<ConfirmHistory>() else {
                return;
            };
            Some((
                component.clone(),
                confirm_history.last_tick(),
                !entity_ref.contains::<ConfirmedHistory<C>>(),
            ))
        }) else {
            return;
        };

        let Some(checkpoints) = world.get_resource::<ReplicationCheckpointMap>() else {
            debug_assert!(
                false,
                "missing checkpoint map while backfilling diff ConfirmedHistory"
            );
            return;
        };
        let Some(tick) = checkpoints.get(message_tick) else {
            debug_assert!(
                false,
                "missing authoritative checkpoint mapping while backfilling diff ConfirmedHistory"
            );
            return;
        };

        let (cursor, has_receiver) = world
            .get_resource::<ReplicationStorage>()
            .map(|storage| {
                (
                    storage
                        .get::<DiffBuffer<C>>(entity)
                        .and_then(DiffBuffer::<C>::last_applied),
                    storage.get::<HistoryDiffReceiver<C>>(entity).is_some(),
                )
            })
            .unwrap_or_default();

        if !insert_history && has_receiver {
            return;
        }

        {
            let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
                return;
            };
            if insert_history && !entity_mut.contains::<ConfirmedHistory<C>>() {
                let mut history = ConfirmedHistory::<C>::default();
                history.insert_present(tick, component);
                entity_mut.insert(history);
            }
            entity_mut.remove::<C>();
        }

        if !has_receiver
            && let Some(cursor) = cursor
            && let Some(mut storage) = world.get_resource_mut::<ReplicationStorage>()
            && storage.get::<HistoryDiffReceiver<C>>(entity).is_none()
        {
            let mut receiver = HistoryDiffReceiver::<C>::default();
            receiver.record_cursor(tick, Some(cursor));
            storage.insert(entity, receiver);
        }
    });
}

pub trait InterpolationRegistrationExt<'a, C>: ComponentRegistrator<'a, C> {
    /// Add interpolation for this component using the provided [`LerpFn`]
    ///
    /// This will register interpolation systems to interpolate between two confirmed states.
    fn add_interpolation_with(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent;

    /// Like [`Self::add_interpolation_with`], but for components replicated with
    /// Replicon's diff-based mode.
    fn add_interpolation_diff_with(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent + RepliconDiffable;

    /// Enable interpolation systems for this component using the [`Ease`] implementation
    ///
    /// This will register interpolation systems to interpolate between two confirmed states.
    fn add_linear_interpolation(self) -> Self
    where
        C: SyncComponent + Ease;

    /// Like [`Self::add_linear_interpolation`], but for components replicated
    /// with Replicon's diff-based mode.
    fn add_linear_interpolation_diff(self) -> Self
    where
        C: SyncComponent + RepliconDiffable + Ease;

    /// The remote updates will be stored in a [`ConfirmedHistory<C>`] component
    /// but the user has to define the interpolation logic themselves
    /// (`lightyear` won't perform any kind of interpolation)
    fn add_custom_interpolation(self) -> Self
    where
        C: SyncComponent;

    /// Like [`Self::add_custom_interpolation`], but for components replicated
    /// with Replicon's diff-based mode.
    fn add_custom_interpolation_diff(self) -> Self
    where
        C: SyncComponent + RepliconDiffable;
}

impl<'a, C, R> InterpolationRegistrationExt<'a, C> for R
where
    R: ComponentRegistrator<'a, C>,
{
    fn add_interpolation_with(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent,
    {
        Self::from_component_registration(add_interpolation_with_impl(
            self.into_component_registration(),
            interpolation_fn,
        ))
    }

    fn add_interpolation_diff_with(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent + RepliconDiffable,
    {
        Self::from_component_registration(add_interpolation_diff_with_impl(
            self.into_component_registration(),
            interpolation_fn,
        ))
    }

    fn add_linear_interpolation(self) -> Self
    where
        C: SyncComponent + Ease,
    {
        self.add_interpolation_with(lerp::<C>)
    }

    fn add_linear_interpolation_diff(self) -> Self
    where
        C: SyncComponent + RepliconDiffable + Ease,
    {
        self.add_interpolation_diff_with(lerp::<C>)
    }

    fn add_custom_interpolation(self) -> Self
    where
        C: SyncComponent,
    {
        Self::from_component_registration(add_custom_interpolation_impl(
            self.into_component_registration(),
        ))
    }

    fn add_custom_interpolation_diff(self) -> Self
    where
        C: SyncComponent + RepliconDiffable,
    {
        Self::from_component_registration(add_custom_interpolation_diff_impl(
            self.into_component_registration(),
        ))
    }
}

fn ensure_interpolation_registry(app: &mut App) {
    if !app.world().contains_resource::<InterpolationRegistry>() {
        app.world_mut()
            .insert_resource(InterpolationRegistry::default());
    }
}

pub(crate) fn mark_interpolated<C: SyncComponent>(app: &mut App) {
    let mut registry = app.world_mut().resource_mut::<ComponentRegistry>();
    registry
        .component_metadata_map
        .get_mut(&ComponentKind::of::<C>())
        .unwrap()
        .replication
        .as_mut()
        .unwrap()
        .set_interpolated(true);
}

pub(crate) fn add_interpolation_rule<C, F>(
    app: &mut App,
    fns: InterpolationFns<C>,
    config: InterpolationRuleConfig,
) where
    C: SyncComponent,
    F: QueryFilter + 'static,
{
    QueryState::<&Archetype, F>::new(app.world_mut());
    ensure_interpolation_registry(app);
    let component_ids = InterpolationRuleComponentIds::for_component::<C>(app.world_mut(), &fns);
    let uses_interpolation_component =
        fns.owns_interpolation_history() || fns.applies_interpolation_component();
    let uses_frame_component = fns.owns_frame_history() || fns.applies_frame_component();
    if uses_interpolation_component || uses_frame_component {
        app.world_mut().register_component::<C>();
    }
    if fns.owns_interpolation_history() {
        register_interpolated_marker_fns::<C>(app);
        mark_interpolated::<C>(app);
    }
    if fns.applies_interpolation_component() {
        mark_interpolated::<C>(app);
    }
    app.world_mut()
        .resource_mut::<InterpolationRegistry>()
        .insert_rule::<C, F>(fns, config, component_ids);
    refresh_update_interpolation_system_if_finalized(app);
}

fn add_interpolation_diff_rule<C, F>(
    app: &mut App,
    fns: InterpolationFns<C>,
    config: InterpolationRuleConfig,
) where
    C: SyncComponent + RepliconDiffable,
    F: QueryFilter + 'static,
{
    QueryState::<&Archetype, F>::new(app.world_mut());
    ensure_interpolation_registry(app);
    let component_ids = InterpolationRuleComponentIds::for_component::<C>(app.world_mut(), &fns);
    let uses_interpolation_component =
        fns.owns_interpolation_history() || fns.applies_interpolation_component();
    let uses_frame_component = fns.owns_frame_history() || fns.applies_frame_component();
    if uses_interpolation_component || uses_frame_component {
        app.world_mut().register_component::<C>();
    }
    if fns.owns_interpolation_history() {
        register_interpolated_diff_marker_fns::<C>(app);
        mark_interpolated::<C>(app);
    }
    if fns.applies_interpolation_component() {
        mark_interpolated::<C>(app);
    }
    app.world_mut()
        .resource_mut::<InterpolationRegistry>()
        .insert_diff_rule::<C, F>(fns, config, component_ids);
    refresh_update_interpolation_system_if_finalized(app);
}

pub(crate) fn add_interpolation_bundle_rule<B, F>(
    app: &mut App,
    fns: InterpolationFns<B>,
    config: InterpolationRuleConfig,
) where
    B: TupleInterpolationBundle,
    F: QueryFilter + 'static,
{
    QueryState::<&Archetype, F>::new(app.world_mut());
    ensure_interpolation_registry(app);
    let owns_interpolation_history = fns.owns_interpolation_history();
    let owns_frame_history = fns.owns_frame_history();
    let applies_interpolation_component = fns.applies_interpolation_component();
    let applies_frame_component = fns.applies_frame_component();
    let apply_interpolation =
        applies_interpolation_component.then_some(B::apply_archetype as ErasedApplyInterpolationFn);
    let apply_frame_interpolation = applies_frame_component
        .then_some(B::apply_frame_archetype as ErasedApplyFrameInterpolationFn);
    let component_ids = if applies_interpolation_component || applies_frame_component {
        B::component_ids(app)
    } else {
        Vec::new()
    };
    let write_component_ids = if applies_interpolation_component {
        component_ids.clone()
    } else {
        Vec::new()
    };
    let frame_write_component_ids = if applies_frame_component {
        component_ids.clone()
    } else {
        Vec::new()
    };
    if applies_interpolation_component {
        B::mark_interpolated(app);
    }
    app.world_mut()
        .resource_mut::<InterpolationRegistry>()
        .insert_bundle_rule::<B, F>(
            fns,
            config,
            B::component_kinds(),
            write_component_ids,
            apply_interpolation,
            frame_write_component_ids,
            apply_frame_interpolation,
        );
    if owns_interpolation_history || owns_frame_history {
        B::add_history_rules::<F>(app, config, owns_interpolation_history);
    }
    refresh_update_interpolation_system_if_finalized(app);
}

fn add_interpolation_with_impl<'a, C>(
    registration: ComponentRegistration<'a, C>,
    interpolation_fn: LerpFn<C>,
) -> ComponentRegistration<'a, C>
where
    C: SyncComponent,
{
    add_interpolation_rule::<C, ()>(
        registration.app,
        InterpolationFns::interpolate(interpolation_fn),
        InterpolationRuleConfig {
            priority: SINGLE_COMPONENT_RULE_PRIORITY,
        },
    );
    registration
}

fn add_interpolation_diff_with_impl<'a, C>(
    registration: ComponentRegistration<'a, C>,
    interpolation_fn: LerpFn<C>,
) -> ComponentRegistration<'a, C>
where
    C: SyncComponent + RepliconDiffable,
{
    add_interpolation_diff_rule::<C, ()>(
        registration.app,
        InterpolationFns::interpolate(interpolation_fn),
        InterpolationRuleConfig {
            priority: SINGLE_COMPONENT_RULE_PRIORITY,
        },
    );
    registration
}

fn add_custom_interpolation_impl<C>(
    registration: ComponentRegistration<'_, C>,
) -> ComponentRegistration<'_, C>
where
    C: SyncComponent,
{
    add_interpolation_rule::<C, ()>(
        registration.app,
        InterpolationFns::history_only(),
        InterpolationRuleConfig {
            priority: SINGLE_COMPONENT_RULE_PRIORITY,
        },
    );
    registration
}

fn add_custom_interpolation_diff_impl<C>(
    registration: ComponentRegistration<'_, C>,
) -> ComponentRegistration<'_, C>
where
    C: SyncComponent + RepliconDiffable,
{
    add_interpolation_diff_rule::<C, ()>(
        registration.app,
        InterpolationFns::history_only(),
        InterpolationRuleConfig {
            priority: SINGLE_COMPONENT_RULE_PRIORITY,
        },
    );
    registration
}

/// Instead of writing into a component directly, it writes data into [`ConfirmedHistory<C>`].
fn write_history<C: SyncComponent>(
    ctx: &mut WriteCtx,
    rule_fns: &RuleFns<C>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<()> {
    let component: C = rule_fns.deserialize(ctx, message)?;
    // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access.
    let checkpoints = {
        let world = unsafe { entity.world_mut() };
        let checkpoints =
            world.resource::<ReplicationCheckpointMap>() as *const ReplicationCheckpointMap;
        unsafe { &*checkpoints }
    };
    let Some(tick) = resolve_message_tick(checkpoints, ctx.message_tick) else {
        error!(
            message_tick = ?ctx.message_tick,
            "missing authoritative checkpoint mapping while writing interpolation history"
        );
        debug_assert!(
            false,
            "missing authoritative checkpoint mapping while writing interpolation history"
        );
        return Ok(());
    };
    let mut new_history = None;
    insert_interpolation_history_value(entity, &mut new_history, tick, component);
    if let Some(history) = new_history {
        entity.insert(history);
    }
    Ok(())
}

fn write_history_diff<C: SyncComponent + RepliconDiffable>(
    ctx: &mut WriteCtx,
    _rule_fns: &RuleFns<C>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<()> {
    let mut new_history = None;
    let Some((tick, diff)) = client_diff_and_tick::<C>(ctx, entity, message)? else {
        return Ok(());
    };
    match diff {
        ComponentDelta::Snapshot {
            index,
            mut component,
        } => {
            C::map_entities(&mut component, ctx);
            let receiver = ctx.get_or_default::<HistoryDiffReceiver<C>>();
            receiver.record_cursor(tick, Some(index));
            insert_interpolation_history_value(entity, &mut new_history, tick, component);
        }
        ComponentDelta::Diffs { index, diffs } => {
            let receiver = ctx.get_or_default::<HistoryDiffReceiver<C>>();
            receiver.queue_diff(tick, index, diffs)?;
        }
    }

    while let Some((tick, value)) = {
        let receiver = ctx.get_or_default::<HistoryDiffReceiver<C>>();
        if let Some(history) = new_history.as_ref() {
            receiver.take_ready_update(history)?
        } else {
            entity
                .get::<ConfirmedHistory<C>>()
                .map(|history| receiver.take_ready_update(history))
                .transpose()?
                .flatten()
        }
    } {
        insert_interpolation_history_value(entity, &mut new_history, tick, value);
    }

    if let Some(history) = new_history {
        entity.insert(history);
    }
    Ok(())
}

fn insert_interpolation_history_value<C: SyncComponent>(
    entity: &mut DeferredEntity,
    new_history: &mut Option<ConfirmedHistory<C>>,
    tick: Tick,
    value: C,
) {
    if let Some(mut history) = entity.get_mut::<ConfirmedHistory<C>>() {
        history.insert_present(tick, value);
    } else {
        let history = new_history.get_or_insert_with(ConfirmedHistory::<C>::default);
        history.insert_present(tick, value);
    }
}

/// Decode the raw Replicon diff bytes and map the Replicon message tick to the
/// corresponding Lightyear server tick.
fn client_diff_and_tick<C: SyncComponent + RepliconDiffable>(
    ctx: &mut WriteCtx,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<Option<(Tick, ComponentDelta<C>)>> {
    let diff: ComponentDelta<C> = postcard_utils::from_buf(message)?;
    let checkpoints = {
        // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access.
        let world = unsafe { entity.world_mut() };
        let checkpoints =
            world.resource::<ReplicationCheckpointMap>() as *const ReplicationCheckpointMap;
        unsafe { &*checkpoints }
    };
    let Some(tick) = resolve_message_tick(checkpoints, ctx.message_tick) else {
        error!(
            message_tick = ?ctx.message_tick,
            "missing authoritative checkpoint mapping while writing diff interpolation history"
        );
        debug_assert!(
            false,
            "missing authoritative checkpoint mapping while writing diff interpolation history"
        );
        return Ok(None);
    };
    Ok(Some((tick, diff)))
}

/// Records a component removal in `ConfirmedHistory<C>`.
///
/// The live component is removed later by interpolation systems once the interpolation timeline
/// reaches the server tick that produced this removal.
fn remove_history<C: SyncComponent>(ctx: &mut RemoveCtx, entity: &mut DeferredEntity) {
    // SAFETY: we only access resources, which don't alias with the DeferredEntity's component access.
    let checkpoints = {
        let world = unsafe { entity.world_mut() };
        let checkpoints =
            world.resource::<ReplicationCheckpointMap>() as *const ReplicationCheckpointMap;
        unsafe { &*checkpoints }
    };
    let Some(tick) = resolve_message_tick(checkpoints, ctx.message_tick) else {
        error!(
            message_tick = ?ctx.message_tick,
            "missing authoritative checkpoint mapping while recording interpolation removal"
        );
        debug_assert!(
            false,
            "missing authoritative checkpoint mapping while recording interpolation removal"
        );
        return;
    };
    if let Some(mut history) = entity.get_mut::<ConfirmedHistory<C>>() {
        history.insert_removed(tick);
    } else {
        let mut history = ConfirmedHistory::<C>::default();
        history.insert_removed(tick);
        entity.insert(history);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use bevy_app::App;
    use bevy_ecs::component::Component;
    use bevy_replicon::postcard_utils;
    use bevy_replicon::prelude::{RepliconPlugins, RepliconTick, RuleFns};
    use bevy_replicon::shared::replication::diff::diff_index::DiffIndex;
    use bevy_replicon::shared::replication::registry::ReplicationRegistry;
    use bevy_replicon::shared::replication::registry::test_fns::TestFnsEntityExt;
    use bevy_state::app::StatesPlugin;
    use lightyear_replication::registry::replication::AppComponentExt;
    use serde::{Deserialize, Serialize};

    #[derive(Component, Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct TestComp(f32);

    fn lerp(start: TestComp, end: TestComp, t: f32) -> TestComp {
        TestComp(start.0 + (end.0 - start.0) * t)
    }

    fn diff_lerp(start: TestDiffComponent, end: TestDiffComponent, t: f32) -> TestDiffComponent {
        if t < 0.5 { start } else { end }
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

    fn registry() -> (InterpolationRegistry, InterpolationRuleId) {
        let mut registry = InterpolationRegistry::default();
        let mut world = World::new();
        let fns = InterpolationFns::interpolate(lerp);
        let component_ids =
            InterpolationRuleComponentIds::for_component::<TestComp>(&mut world, &fns);
        let rule_id = registry.insert_rule::<TestComp, ()>(
            fns,
            InterpolationRuleConfig::default(),
            component_ids,
        );
        (registry, rule_id)
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

    fn setup_interpolation_diff_app() -> (App, bevy_replicon::shared::replication::registry::FnsId)
    {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconPlugins,
            crate::plugin::InterpolationPlugin,
        ));
        app.insert_resource(ReplicationCheckpointMap::default());
        app.component::<TestDiffComponent>()
            .replicate_diff()
            .add_custom_interpolation_diff();

        let fns_id =
            app.world_mut()
                .resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                    let (_, fns_id) =
                        registry.register_rule_fns(world, RuleFns::<TestDiffComponent>::new_diff());
                    fns_id
                });
        (app, fns_id)
    }

    #[test]
    fn add_interpolation_diff_with_registers_diff_history_and_sampler() {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconPlugins,
            crate::plugin::InterpolationPlugin,
        ));
        app.component::<TestDiffComponent>()
            .replicate_diff()
            .add_interpolation_diff_with(diff_lerp);

        let registry = app.world().resource::<InterpolationRegistry>();
        assert!(registry.interpolated::<TestDiffComponent>());
        assert!(registry.has_interpolation_fn::<TestDiffComponent>());
    }

    #[test]
    #[should_panic(
        expected = "cannot register interpolation rules after InterpolationRegistry has been finalized"
    )]
    fn finalized_registry_rejects_rule_registration() {
        let mut registry = InterpolationRegistry::default();
        let mut world = World::new();
        let fns = InterpolationFns::history_only();
        let component_ids =
            InterpolationRuleComponentIds::for_component::<TestComp>(&mut world, &fns);
        registry.finalize();
        registry.insert_rule::<TestComp, ()>(
            fns,
            InterpolationRuleConfig::default(),
            component_ids,
        );
    }

    fn record_checkpoint(app: &mut App, tick: u32) -> RepliconTick {
        let replicon_tick = RepliconTick::new(tick);
        app.world_mut()
            .resource_mut::<ReplicationCheckpointMap>()
            .record(replicon_tick, Tick(tick));
        replicon_tick
    }

    #[test]
    fn sample_clamps_to_newest_value_when_tick_is_past_end() {
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(0.0));
        history.insert_present(Tick(20), TestComp(10.0));

        let (registry, rule_id) = registry();
        assert_eq!(
            registry.sample_for_rule(rule_id, &history, Tick(30), 0.0),
            Some(HistoryState::Updated(TestComp(10.0)))
        );
        assert_eq!(
            registry.sample_for_rule(rule_id, &history, Tick(20), 0.5),
            Some(HistoryState::Updated(TestComp(10.0)))
        );
    }

    #[test]
    fn sample_returns_start_value_with_single_keyframe() {
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(42.0));

        let (registry, rule_id) = registry();
        assert_eq!(
            registry.sample_for_rule(rule_id, &history, Tick(5), 0.0),
            None
        );
        assert_eq!(
            registry.sample_for_rule(rule_id, &history, Tick(10), 0.0),
            Some(HistoryState::Updated(TestComp(42.0)))
        );
        assert_eq!(
            registry.sample_for_rule(rule_id, &history, Tick(50), 0.5),
            Some(HistoryState::Updated(TestComp(42.0)))
        );
    }

    #[test]
    fn inserts_history_when_interpolated_added_after_component_is_already_replicated() {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconPlugins,
            crate::plugin::InterpolationPlugin,
        ));
        app.insert_resource(ReplicationCheckpointMap::default());
        app.component::<TestComp>()
            .replicate()
            .add_custom_interpolation();
        app.finish();

        let replicon_tick = RepliconTick::new(11);
        app.world_mut()
            .resource_mut::<ReplicationCheckpointMap>()
            .record(replicon_tick, Tick(42));

        let entity = app
            .world_mut()
            .spawn((TestComp(2.0), ConfirmHistory::new(replicon_tick)))
            .id();
        app.world_mut().entity_mut(entity).insert(Interpolated);

        let history = app
            .world()
            .entity(entity)
            .get::<ConfirmedHistory<TestComp>>()
            .unwrap();
        assert_eq!(
            history
                .start_present()
                .map(|(tick, value)| (tick, value.clone())),
            Some((Tick(42), TestComp(2.0)))
        );
        assert!(
            !app.world().entity(entity).contains::<TestComp>(),
            "live interpolated component should be removed until the interpolation timeline reaches the history start tick"
        );
    }

    #[test]
    fn inserts_history_when_interpolated_and_component_are_spawned_together() {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconPlugins,
            crate::plugin::InterpolationPlugin,
        ));
        app.insert_resource(ReplicationCheckpointMap::default());
        app.component::<TestComp>()
            .replicate()
            .add_custom_interpolation();
        app.finish();

        let replicon_tick = RepliconTick::new(12);
        app.world_mut()
            .resource_mut::<ReplicationCheckpointMap>()
            .record(replicon_tick, Tick(43));

        let entity = app
            .world_mut()
            .spawn((
                TestComp(3.0),
                ConfirmHistory::new(replicon_tick),
                Interpolated,
            ))
            .id();

        let history = app
            .world()
            .entity(entity)
            .get::<ConfirmedHistory<TestComp>>()
            .unwrap();
        assert_eq!(
            history
                .start_present()
                .map(|(tick, value)| (tick, value.clone())),
            Some((Tick(43), TestComp(3.0)))
        );
        assert!(
            !app.world().entity(entity).contains::<TestComp>(),
            "live interpolated component should be removed until the interpolation timeline reaches the history start tick"
        );
    }

    #[test]
    fn diff_interpolation_buffers_newer_diff_until_older_base_arrives() {
        let (mut app, fns_id) = setup_interpolation_diff_app();
        let tick0 = record_checkpoint(&mut app, 0);
        let tick3 = record_checkpoint(&mut app, 3);
        let tick5 = record_checkpoint(&mut app, 5);

        let entity = app.world_mut().spawn(Interpolated).id();

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
