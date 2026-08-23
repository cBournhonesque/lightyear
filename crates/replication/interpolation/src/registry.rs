use crate::SyncComponent;
use crate::interpolate::{
    apply_interpolation_archetype_erased, history_bracket, update_history_archetype_erased,
    update_history_diff_archetype_erased,
};
use crate::rules::frame_interpolate::{
    ErasedApplyFrameInterpolationFn, apply_frame_interpolation_archetype_erased,
    restore_frame_history_archetype_erased, update_frame_history_archetype_erased,
};
use crate::rules::{
    ConfirmedHistoryFns, ContextInterpolationFn, ErasedApplyInterpolationFn,
    ErasedInsertConfirmedHistoryFn, ErasedUpdateHistoryFn, FrameHistoryFns, InterpolationBundle,
    InterpolationFn, InterpolationFns, InterpolationRule, InterpolationRuleComponent,
    InterpolationRuleConfig, InterpolationRuleId, InterpolationSampleContext,
    TupleInterpolationBundle,
};
use alloc::vec::Vec;
use bevy_app::App;
use bevy_ecs::archetype::Archetype;
use bevy_ecs::component::{ComponentId, Components};
use bevy_ecs::prelude::*;
use bevy_ecs::query::{ArchetypeFilter, ComponentIdSet, QueryState};
use bevy_math::{
    Curve,
    curve::{Ease, EaseFunction, EasingCurve},
};
use bevy_replicon::bytes::Bytes;
use bevy_replicon::client::confirm_history::ConfirmHistory;
use bevy_replicon::postcard_utils;
use bevy_replicon::prelude::{AppMarkerExt, RuleFns};
use bevy_replicon::shared::replication::deferred_entity::DeferredEntity;
use bevy_replicon::shared::replication::diff::{
    ComponentDelta, DiffBuffer, Diffable as RepliconDiffable,
};
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, WriteCtx};
use bevy_replicon::shared::replication::registry::receive_fns::WriteFn;
use bevy_replicon::shared::replication::storage::{EntityStorageCtx, ReplicationStorage};
use bevy_utils::prelude::DebugName;
use core::hash::{BuildHasher, Hash, Hasher};
use core::time::Duration;
use lightyear_core::history_buffer::HistoryState;
use lightyear_core::prelude::{ConfirmedHistory, FrameInterpolationHistory, Interpolated, Tick};
use lightyear_replication::checkpoint::{ReplicationCheckpointMap, resolve_message_tick};
use lightyear_replication::diff_history::HistoryDiffReceiver;
use lightyear_replication::prelude::InterpolatedSend;
use lightyear_replication::registry::replication::{ComponentRegistration, ComponentRegistrator};
use lightyear_replication::registry::{ComponentKind, LerpFn};
use tracing::{error, trace};

fn lerp<C: Ease + Clone>(start: C, other: C, t: f32) -> C {
    let curve = EasingCurve::new(start, other, EaseFunction::Linear);
    curve.sample_unchecked(t)
}

const SINGLE_COMPONENT_RULE_PRIORITY: usize = 1;

/// History representation used when resolving history ownership.
///
/// This affects only the history lane. Apply ownership always requires every
/// live component targeted by the rule.
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub enum RuleTarget {
    /// Normal interpolation uses live components or confirmed histories.
    Default,
    /// Frame interpolation and correction use live components or frame histories.
    Frame,
}

/// The work assigned to one interpolation rule for an archetype.
///
/// The resolver selects rules independently for two jobs:
///
/// - `owns_history` selects which rule's member callbacks create and update
///   histories and insert or remove delayed live components.
/// - `owns_apply` selects which rule's interpolation callback writes the live
///   components.
///
/// Winning one job does not affect resolution of the other. A rule without the
/// corresponding callbacks still reserves its members for that job, so a
/// lower-priority rule cannot run instead.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[doc(hidden)]
pub struct ResolvedRule {
    /// Rule receiving work in at least one lane.
    pub rule_id: InterpolationRuleId,
    /// Whether this rule wins the history-policy lane for all of its members.
    pub owns_history: bool,
    /// Whether this rule wins the interpolation-application lane for all of its members.
    pub owns_apply: bool,
}

/// Reusable temporary storage for [`InterpolationRegistry::resolved_rules_for_archetype`].
///
/// Archetype caches keep one of these and reuse its vector capacities while
/// resolving newly-created archetypes.
#[derive(Debug, Default)]
#[doc(hidden)]
pub struct RuleResolutionScratch {
    candidates: Vec<InterpolationRuleId>,
    history_claimed_members: Vec<ComponentKind>,
    apply_claimed_members: Vec<ComponentKind>,
    resolved_rules: Vec<ResolvedRule>,
}

/// Hash of the component presence that can affect interpolation rule resolution.
///
/// Archetypes with the same key can share one resolved interpolation policy
/// even when their unrelated component sets differ.
#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
#[doc(hidden)]
pub struct InterpolationArchetypeKey(u64);

impl InterpolationArchetypeKey {
    fn new(archetype: &Archetype, component_ids: impl IntoIterator<Item = ComponentId>) -> Self {
        let mut hasher = bevy_platform::hash::FixedHasher.build_hasher();
        for component_id in component_ids {
            if archetype.contains(component_id) {
                component_id.hash(&mut hasher);
            }
        }
        Self(hasher.finish())
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
    /// Component kinds whose Replicon receive marker functions have been installed.
    interpolated_marker_fns: Vec<ComponentKind>,
    /// Component presence that can affect normal interpolation rule resolution.
    default_archetype_key_component_ids: Vec<ComponentId>,
    /// Component presence that can affect frame interpolation rule resolution.
    frame_archetype_key_component_ids: Vec<ComponentId>,
    /// Whether plugin finalization has run.
    ///
    /// Rule registration after finalization is rejected so the type-erased
    /// interpolation system has stable access requirements.
    finalized: bool,
}

impl InterpolationRegistry {
    const FINALIZED_RULE_REGISTRATION_ERROR: &'static str =
        "cannot register interpolation rules after InterpolationRegistry has been finalized";

    #[doc(hidden)]
    pub fn finalize(&mut self) {
        if self.finalized {
            return;
        }
        self.compute_archetype_key_component_ids();
        self.finalized = true;
    }

    fn compute_archetype_key_component_ids(&mut self) {
        let mut default_component_ids = ComponentIdSet::new();
        let mut frame_component_ids = ComponentIdSet::new();

        for rule in &self.rules {
            default_component_ids.extend(rule.filter_component_ids.iter().copied());
            frame_component_ids.extend(rule.filter_component_ids.iter().copied());

            for component in &rule.members {
                default_component_ids.insert(component.live_component_id);
                default_component_ids.insert(component.confirmed_history_component_id);

                frame_component_ids.insert(component.live_component_id);
                frame_component_ids.insert(component.frame_history_component_id);
            }
        }

        self.default_archetype_key_component_ids = default_component_ids.into_iter().collect();
        self.frame_archetype_key_component_ids = frame_component_ids.into_iter().collect();
    }

    fn assert_not_finalized(&self) {
        assert!(
            !self.finalized,
            "{}",
            Self::FINALIZED_RULE_REGISTRATION_ERROR
        );
    }

    /// Returns a rule by ID.
    ///
    /// # Panics
    ///
    /// Panics if `rule_id` was not produced by this registry.
    #[doc(hidden)]
    pub fn rule(&self, rule_id: InterpolationRuleId) -> &InterpolationRule {
        self.rules
            .get(rule_id.0)
            .expect("interpolation rule ID should belong to this registry")
    }

    /// Hashes the rule-resolution-relevant component presence for `archetype`
    /// and `target`.
    #[doc(hidden)]
    pub fn archetype_key(
        &self,
        archetype: &Archetype,
        target: RuleTarget,
    ) -> InterpolationArchetypeKey {
        self.archetype_key_with(archetype, target, core::iter::empty())
    }

    /// Hashes the rule-resolution-relevant component presence plus additional
    /// cache-specific components for `archetype` and `target`.
    #[doc(hidden)]
    pub fn archetype_key_with(
        &self,
        archetype: &Archetype,
        target: RuleTarget,
        additional_component_ids: impl IntoIterator<Item = ComponentId>,
    ) -> InterpolationArchetypeKey {
        let component_ids = match target {
            RuleTarget::Default => &self.default_archetype_key_component_ids,
            RuleTarget::Frame => &self.frame_archetype_key_component_ids,
        };
        InterpolationArchetypeKey::new(
            archetype,
            component_ids.iter().copied().chain(
                additional_component_ids
                    .into_iter()
                    .filter(|component_id| !component_ids.contains(component_id)),
            ),
        )
    }

    #[cfg(test)]
    pub(crate) fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Returns component IDs that the type-erased interpolation system may write.
    ///
    /// The custom interpolation system param uses this to declare access before
    /// it reads or writes component columns through [`UnsafeWorldCell`].
    pub(crate) fn component_write_ids(&self) -> Vec<ComponentId> {
        let mut ids = Vec::new();
        for member in self.rules.iter().flat_map(|rule| &rule.members) {
            if member.confirmed.is_some() {
                for component_id in [
                    member.confirmed_history_component_id,
                    member.live_component_id,
                ] {
                    if !ids.contains(&component_id) {
                        ids.push(component_id);
                    }
                }
            }
        }
        ids
    }

    /// Returns component IDs that the type-erased frame interpolation systems may write.
    #[doc(hidden)]
    pub fn frame_component_write_ids(&self) -> Vec<ComponentId> {
        let mut ids = Vec::new();
        for member in self.rules.iter().flat_map(|rule| &rule.members) {
            if member.frame.is_some() {
                for component_id in [member.frame_history_component_id, member.live_component_id] {
                    if !ids.contains(&component_id) {
                        ids.push(component_id);
                    }
                }
            }
        }
        ids
    }

    /// Resolves history and apply ownership for one archetype.
    ///
    /// Candidates are ordered by descending priority and then by registration
    /// order. History and apply are then resolved independently, with separate
    /// claimed-member sets:
    ///
    /// - For history, a member is available when either its live component or
    ///   its history selected by `target` is present.
    /// - For apply, a member is available only when its live component is
    ///   present. History presence does not affect apply selection.
    /// - A winning rule claims all of its members for that job. A later rule
    ///   cannot reuse any of them for the same job.
    ///
    /// The jobs must be independent because history maintenance controls
    /// delayed component presence. If replication has received `B` into
    /// `History<B>` but the interpolation timeline has not reached B's
    /// insertion tick, the entity has `(A, History<A>, History<B>)`. The bundle
    /// cannot apply without live `B`, but it must still win history so its
    /// history callbacks continue running and eventually insert live `B`.
    /// Meanwhile, the standalone `A` rule can keep interpolating live `A`.
    ///
    /// For example, consider a higher-priority `(A, B)` rule and a
    /// lower-priority `A` rule, both registered with
    /// `InterpolationFns::interpolate`:
    ///
    /// - With `(A, B, History<A>)`, both live members exist. The bundle wins
    ///   both jobs even though `History<B>` is missing. Missing history does not
    ///   make the resolver fall back to `A`; the bundle's history callback will
    ///   normally create `History<B>`.
    /// - With `(A, History<A>, History<B>)`, live `B` is absent during delayed
    ///   insertion or after delayed removal. The bundle wins history because
    ///   both members have a history representation, but it cannot win apply.
    ///   The `A` rule therefore applies live `A` while the bundle maintains both
    ///   histories.
    /// - With `(A, History<A>)`, neither live `B` nor `History<B>` exists. The
    ///   bundle is unavailable for both jobs, so the `A` rule wins both.
    ///
    /// A normal delayed insertion therefore transitions like this:
    ///
    /// ```text
    /// replication receives B:
    ///     (A, History<A>)
    ///         -> (A, History<A>, History<B>)
    ///
    /// before B's insertion tick:
    ///     history owner = (A, B)  // maintains both histories
    ///     apply owner   = A       // interpolates the only live member
    ///
    /// when history maintenance reaches B's insertion tick:
    ///     (A, History<A>, History<B>)
    ///         -> (A, B, History<A>, History<B>)
    ///
    /// after B is live:
    ///     history owner = (A, B)
    ///     apply owner   = (A, B)  // interpolates the bundle together
    /// ```
    ///
    /// History preparation runs before interpolation application. If creating
    /// a missing history inserts or removes a live component, the apply phase
    /// resolves the resulting archetype before choosing its apply rule.
    #[doc(hidden)]
    pub fn resolved_rules_for_archetype<'a>(
        &self,
        components: &Components,
        archetype: &Archetype,
        target: RuleTarget,
        scratch: &'a mut RuleResolutionScratch,
    ) -> &'a [ResolvedRule] {
        let RuleResolutionScratch {
            candidates,
            history_claimed_members,
            apply_claimed_members,
            resolved_rules,
        } = scratch;
        candidates.clear();
        history_claimed_members.clear();
        apply_claimed_members.clear();
        resolved_rules.clear();

        candidates.extend(
            self.rules
                .iter()
                .enumerate()
                .filter(|(_, rule)| (rule.matches_archetype)(components, archetype))
                .map(|(index, _)| InterpolationRuleId(index)),
        );
        candidates.sort_by(|lhs, rhs| {
            self.rules[rhs.0]
                .priority
                .cmp(&self.rules[lhs.0].priority)
                .then_with(|| lhs.0.cmp(&rhs.0))
        });
        for rule_id in candidates.iter().copied() {
            let rule = &self.rules[rule_id.0];
            let history = rule.members.iter().all(|member| {
                archetype.contains(member.live_component_id)
                    || match target {
                        RuleTarget::Default => {
                            archetype.contains(member.confirmed_history_component_id)
                        }
                        RuleTarget::Frame => archetype.contains(member.frame_history_component_id),
                    }
            }) && !rule
                .members()
                .any(|member| history_claimed_members.contains(&member));
            let apply = rule
                .members
                .iter()
                .all(|member| archetype.contains(member.live_component_id))
                && !rule
                    .members()
                    .any(|member| apply_claimed_members.contains(&member));
            if history {
                history_claimed_members.extend(rule.members());
            }
            if apply {
                apply_claimed_members.extend(rule.members());
            }
            if history || apply {
                resolved_rules.push(ResolvedRule {
                    rule_id,
                    owns_history: history,
                    owns_apply: apply,
                });
            }
        }
        resolved_rules
    }

    pub(crate) fn insert_rule(&mut self, rule: InterpolationRule) -> InterpolationRuleId {
        self.assert_not_finalized();
        for (index, member) in rule.members.iter().enumerate() {
            assert!(
                !rule.members[..index]
                    .iter()
                    .any(|previous| previous.kind == member.kind),
                "interpolation bundle rules cannot contain duplicate component types"
            );
        }

        let rule_id = InterpolationRuleId(self.rules.len());
        self.rules.push(rule);
        rule_id
    }

    /// Returns `true` if any interpolation rule covers component `C`.
    pub fn interpolated<C: Component>(&self) -> bool {
        let kind = ComponentKind::of::<C>();
        self.rules
            .iter()
            .any(|rule| rule.members().any(|member| member == kind))
    }

    pub(crate) fn interpolation_fn_for_rule<S: 'static>(
        &self,
        rule_id: InterpolationRuleId,
    ) -> &InterpolationFn<S> {
        self.rule(rule_id)
            .interpolation
            .as_ref()
            .expect("interpolation apply callback requires an interpolation function")
            .typed::<S>()
    }
}

fn build_rule_member<C>(
    world: &mut World,
    owns_interpolation_history: bool,
    owns_frame_history: bool,
    update_history: ErasedUpdateHistoryFn,
    insert_history: ErasedInsertConfirmedHistoryFn,
) -> InterpolationRuleComponent
where
    C: SyncComponent,
{
    InterpolationRuleComponent {
        kind: ComponentKind::of::<C>(),
        // Every member needs all representation IDs for archetype matching,
        // including disabled rules that own no history or apply callbacks.
        live_component_id: world.register_component::<C>(),
        confirmed_history_component_id: world.register_component::<ConfirmedHistory<C>>(),
        frame_history_component_id: world.register_component::<FrameInterpolationHistory<C>>(),
        confirmed: owns_interpolation_history.then_some(ConfirmedHistoryFns {
            update_history,
            insert_history,
        }),
        frame: owns_frame_history.then_some(FrameHistoryFns {
            update_history: update_frame_history_archetype_erased::<C>,
            restore_history: restore_frame_history_archetype_erased::<C>,
        }),
    }
}

pub(crate) fn component_rule<C, F>(
    world: &mut World,
    fns: InterpolationFns<C>,
    config: InterpolationRuleConfig,
    update_history: ErasedUpdateHistoryFn,
    insert_history: ErasedInsertConfirmedHistoryFn,
) -> InterpolationRule
where
    C: SyncComponent,
    F: ArchetypeFilter + 'static,
{
    let member = build_rule_member::<C>(
        world,
        fns.owns_interpolation_history(),
        fns.owns_frame_history(),
        update_history,
        insert_history,
    );
    let apply_interpolation = fns
        .applies_interpolation_component()
        .then_some(apply_interpolation_archetype_erased::<C> as ErasedApplyInterpolationFn);
    let apply_frame_interpolation = fns.applies_frame_component().then_some(
        apply_frame_interpolation_archetype_erased::<C> as ErasedApplyFrameInterpolationFn,
    );
    InterpolationRule::new::<C, F>(
        world.components(),
        fns,
        alloc::vec![member],
        config.priority,
        apply_interpolation,
        apply_frame_interpolation,
    )
}

pub(crate) fn sample_history_with_interpolation<C: Component + Clone>(
    interpolation: &InterpolationFn<C>,
    history: &ConfirmedHistory<C>,
    interpolation_tick: Tick,
    interpolation_overstep: f32,
    tick_duration: Option<Duration>,
) -> Option<HistoryState<C>> {
    let bracket = history_bracket(history, interpolation_tick)?;
    let HistoryState::Updated(start) = bracket.start_state else {
        return Some(HistoryState::Removed);
    };

    let Some((end_tick, HistoryState::Updated(end))) = bracket.end else {
        return Some(HistoryState::Updated(start.clone()));
    };

    // Clamp rather than extrapolate beyond the newest confirmed value. This
    // makes late packets converge to the freshest server state instead of
    // overshooting when motion changes direction.
    let context = InterpolationSampleContext::from_ticks(
        bracket.start_tick,
        end_tick,
        interpolation_tick,
        interpolation_overstep,
        tick_duration,
    );
    trace!(
        target: "lightyear_debug::interpolation",
        kind = "confirmed_history_sample",
        component = ?DebugName::type_name::<C>(),
        interpolation_tick = interpolation_tick.0,
        start_tick = bracket.start_tick.0,
        end_tick = end_tick.0,
        interpolation_overstep,
        fraction = context.t,
        history_len = history.len(),
        "sampled confirmed history for interpolation"
    );
    Some(HistoryState::Updated(interpolation.interpolate(
        start.clone(),
        end.clone(),
        context,
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
        F: ArchetypeFilter + 'static,
    {
        self.linear_interpolate_with_priority_filtered::<C, F>(SINGLE_COMPONENT_RULE_PRIORITY)
    }

    /// Registers a full linear interpolation rule for component `C`, archetype
    /// filter `F`, and explicit priority.
    fn linear_interpolate_with_priority_filtered<C, F>(&mut self, priority: usize) -> &mut Self
    where
        C: SyncComponent + Ease,
        F: ArchetypeFilter + 'static,
    {
        self.interpolate_with_priority_filtered::<C, F>(
            priority,
            InterpolationFns::interpolate(lerp::<C>),
        )
    }

    /// Registers a default-priority interpolation rule for component `C`.
    ///
    /// If the registered [`InterpolationFns`] owns history, Lightyear receives
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
        F: ArchetypeFilter + 'static,
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
        F: ArchetypeFilter + 'static;

    /// Registers a bundle interpolation rule with default bundle priority.
    ///
    /// Lightyear stores each component in its own [`ConfirmedHistory`], then
    /// samples their histories together at shared ticks around the
    /// interpolation time. Unchanged members carry their latest present value
    /// forward to those shared ticks.
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
        F: ArchetypeFilter + 'static,
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
        F: ArchetypeFilter + 'static;

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
        F: ArchetypeFilter + 'static,
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
        F: ArchetypeFilter + 'static;
}

impl AppInterpolationExt for App {
    fn interpolate_with_priority_filtered<C, F>(
        &mut self,
        priority: usize,
        fns: InterpolationFns<C>,
    ) -> &mut Self
    where
        C: SyncComponent,
        F: ArchetypeFilter + 'static,
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
        F: ArchetypeFilter + 'static,
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
        F: ArchetypeFilter + 'static,
    {
        add_interpolation_diff_rule::<C, F>(self, fns, InterpolationRuleConfig { priority });
        self
    }
}

fn register_interpolated_marker_fns<C: SyncComponent>(app: &mut bevy_app::App, write: WriteFn<C>) {
    // Frame interpolation can use the same rule registry without Replicon.
    // Such apps have no receive pipeline whose marker functions need replacing.
    if !app.world().contains_resource::<ReplicationRegistry>() {
        return;
    }
    let kind = ComponentKind::of::<C>();
    if app
        .world()
        .resource::<InterpolationRegistry>()
        .interpolated_marker_fns
        .contains(&kind)
    {
        return;
    }
    app.set_marker_fns::<Interpolated, C>(write, remove_history::<C>);
    app.set_marker_fns::<InterpolatedSend, C>(write, remove_history::<C>);
    app.world_mut()
        .resource_mut::<InterpolationRegistry>()
        .interpolated_marker_fns
        .push(kind);
}

/// Initializes `ConfirmedHistory<C>` from the current replicated component.
pub(crate) fn insert_confirmed_history<C: SyncComponent>(entity: Entity, commands: &mut Commands) {
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
                "missing checkpoint map while initializing ConfirmedHistory"
            );
            return;
        };
        let Some(tick) = checkpoints.get(message_tick) else {
            debug_assert!(
                false,
                "missing authoritative checkpoint mapping while initializing ConfirmedHistory"
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

/// Diff-aware variant of [`insert_confirmed_history`].
pub(crate) fn insert_confirmed_history_diff<C: SyncComponent + RepliconDiffable>(
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
                "missing checkpoint map while initializing diff ConfirmedHistory"
            );
            return;
        };
        let Some(tick) = checkpoints.get(message_tick) else {
            debug_assert!(
                false,
                "missing authoritative checkpoint mapping while initializing diff ConfirmedHistory"
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
    /// Add interpolation for this component using the provided [`LerpFn`].
    ///
    /// This will register interpolation systems to interpolate between two confirmed states.
    fn add_interpolation_with(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent;

    /// Add interpolation that receives sample timing in addition to the
    /// normalized interpolation fraction.
    fn add_interpolation_with_context(self, interpolation_fn: ContextInterpolationFn<C>) -> Self
    where
        C: SyncComponent;

    /// Like [`Self::add_interpolation_with`], but for components replicated with
    /// Replicon's diff-based mode.
    fn add_interpolation_diff_with(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent + RepliconDiffable;

    /// Diff-based counterpart to [`Self::add_interpolation_with_context`].
    fn add_interpolation_diff_with_context(
        self,
        interpolation_fn: ContextInterpolationFn<C>,
    ) -> Self
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

    fn add_interpolation_with_context(self, interpolation_fn: ContextInterpolationFn<C>) -> Self
    where
        C: SyncComponent,
    {
        Self::from_component_registration(add_interpolation_with_context_impl(
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

    fn add_interpolation_diff_with_context(
        self,
        interpolation_fn: ContextInterpolationFn<C>,
    ) -> Self
    where
        C: SyncComponent + RepliconDiffable,
    {
        Self::from_component_registration(add_interpolation_diff_with_context_impl(
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

pub(crate) fn add_interpolation_rule<C, F>(
    app: &mut App,
    fns: InterpolationFns<C>,
    config: InterpolationRuleConfig,
) where
    C: SyncComponent,
    F: ArchetypeFilter + 'static,
{
    add_component_interpolation_rule::<C, F>(
        app,
        fns,
        config,
        write_history::<C>,
        update_history_archetype_erased::<C>,
        insert_confirmed_history::<C>,
    );
}

fn add_interpolation_diff_rule<C, F>(
    app: &mut App,
    fns: InterpolationFns<C>,
    config: InterpolationRuleConfig,
) where
    C: SyncComponent + RepliconDiffable,
    F: ArchetypeFilter + 'static,
{
    add_component_interpolation_rule::<C, F>(
        app,
        fns,
        config,
        write_history_diff::<C>,
        update_history_diff_archetype_erased::<C>,
        insert_confirmed_history_diff::<C>,
    );
}

fn add_component_interpolation_rule<C, F>(
    app: &mut App,
    fns: InterpolationFns<C>,
    config: InterpolationRuleConfig,
    write_history: WriteFn<C>,
    update_history: ErasedUpdateHistoryFn,
    insert_history: ErasedInsertConfirmedHistoryFn,
) where
    C: SyncComponent,
    F: ArchetypeFilter + 'static,
{
    app.world_mut().init_resource::<InterpolationRegistry>();
    app.world()
        .resource::<InterpolationRegistry>()
        .assert_not_finalized();
    QueryState::<&Archetype, F>::new(app.world_mut());
    if fns.owns_interpolation_history() {
        register_interpolated_marker_fns::<C>(app, write_history);
    }
    let rule = component_rule::<C, F>(app.world_mut(), fns, config, update_history, insert_history);
    app.world_mut()
        .resource_mut::<InterpolationRegistry>()
        .insert_rule(rule);
}

/// Builds one self-contained member for a bundle interpolation rule.
pub(crate) fn interpolation_rule_member<C>(
    app: &mut App,
    include_interpolation_history: bool,
    include_frame_history: bool,
) -> InterpolationRuleComponent
where
    C: SyncComponent,
{
    let member = build_rule_member::<C>(
        app.world_mut(),
        include_interpolation_history,
        include_frame_history,
        update_history_archetype_erased::<C>,
        insert_confirmed_history::<C>,
    );
    if include_interpolation_history {
        register_interpolated_marker_fns::<C>(app, write_history::<C>);
    }
    member
}

pub(crate) fn add_interpolation_bundle_rule<B, F>(
    app: &mut App,
    fns: InterpolationFns<B>,
    config: InterpolationRuleConfig,
) where
    B: TupleInterpolationBundle,
    F: ArchetypeFilter + 'static,
{
    app.world_mut().init_resource::<InterpolationRegistry>();
    app.world()
        .resource::<InterpolationRegistry>()
        .assert_not_finalized();
    QueryState::<&Archetype, F>::new(app.world_mut());
    let owns_interpolation_history = fns.owns_interpolation_history();
    let owns_frame_history = fns.owns_frame_history();
    let applies_interpolation_component = fns.applies_interpolation_component();
    let applies_frame_component = fns.applies_frame_component();
    let apply_interpolation =
        applies_interpolation_component.then_some(B::apply_archetype as ErasedApplyInterpolationFn);
    let apply_frame_interpolation = applies_frame_component
        .then_some(B::apply_frame_archetype as ErasedApplyFrameInterpolationFn);
    let members = B::rule_members(app, owns_interpolation_history, owns_frame_history);
    let rule = InterpolationRule::new::<B, F>(
        app.world().components(),
        fns,
        members,
        config.priority,
        apply_interpolation,
        apply_frame_interpolation,
    );
    app.world_mut()
        .resource_mut::<InterpolationRegistry>()
        .insert_rule(rule);
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

fn add_interpolation_with_context_impl<'a, C>(
    registration: ComponentRegistration<'a, C>,
    interpolation_fn: ContextInterpolationFn<C>,
) -> ComponentRegistration<'a, C>
where
    C: SyncComponent,
{
    add_interpolation_rule::<C, ()>(
        registration.app,
        InterpolationFns::interpolate_with_context(interpolation_fn),
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

fn add_interpolation_diff_with_context_impl<'a, C>(
    registration: ComponentRegistration<'a, C>,
    interpolation_fn: ContextInterpolationFn<C>,
) -> ComponentRegistration<'a, C>
where
    C: SyncComponent + RepliconDiffable,
{
    add_interpolation_diff_rule::<C, ()>(
        registration.app,
        InterpolationFns::interpolate_with_context(interpolation_fn),
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
    use crate::interpolate::apply_interpolation;
    use crate::timeline::InterpolationTimeline;
    use alloc::vec::Vec;
    use bevy_app::App;
    use bevy_ecs::component::Component;
    use bevy_ecs::system::RunSystemOnce;
    use bevy_replicon::postcard_utils;
    use bevy_replicon::prelude::{RepliconPlugins, RepliconTick, RuleFns};
    use bevy_replicon::shared::replication::diff::diff_index::DiffIndex;
    use bevy_replicon::shared::replication::registry::ReplicationRegistry;
    use bevy_replicon::shared::replication::registry::test_fns::TestFnsEntityExt;
    use bevy_state::app::StatesPlugin;
    use lightyear_core::time::TickInstant;
    use lightyear_core::timeline::NetworkTimeline;
    use lightyear_replication::registry::replication::AppComponentExt;
    use serde::{Deserialize, Serialize};

    #[derive(Component, Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct TestComp(f32);

    #[derive(Component, Clone, Debug, PartialEq)]
    struct TestComp2(f32);

    #[derive(Component)]
    struct NoHistory;

    fn lerp(start: TestComp, end: TestComp, t: f32) -> TestComp {
        TestComp(start.0 + (end.0 - start.0) * t)
    }

    fn lerp2(start: TestComp2, end: TestComp2, t: f32) -> TestComp2 {
        TestComp2(start.0 + (end.0 - start.0) * t)
    }

    fn bundle_lerp(
        start: (TestComp, TestComp2),
        end: (TestComp, TestComp2),
        t: f32,
    ) -> (TestComp, TestComp2) {
        (lerp(start.0, end.0, t), lerp2(start.1, end.1, t))
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
        let rule = component_rule::<TestComp, ()>(
            &mut world,
            fns,
            InterpolationRuleConfig::default(),
            update_history_archetype_erased::<TestComp>,
            insert_confirmed_history::<TestComp>,
        );
        let rule_id = registry.insert_rule(rule);
        (registry, rule_id)
    }

    fn sample_for_rule(
        registry: &InterpolationRegistry,
        rule_id: InterpolationRuleId,
        history: &ConfirmedHistory<TestComp>,
        interpolation_tick: Tick,
        interpolation_overstep: f32,
    ) -> Option<HistoryState<TestComp>> {
        sample_history_with_interpolation(
            registry.interpolation_fn_for_rule::<TestComp>(rule_id),
            history,
            interpolation_tick,
            interpolation_overstep,
            None,
        )
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
            crate::plugin::InterpolationMarkerPlugin,
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
    fn add_interpolation_diff_with_applies_registered_sampler() {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconPlugins,
            crate::plugin::InterpolationMarkerPlugin,
            crate::plugin::InterpolationPlugin,
        ));
        app.insert_resource(ReplicationCheckpointMap::default());
        app.component::<TestDiffComponent>()
            .replicate_diff()
            .add_interpolation_diff_with(diff_lerp);
        let mut timeline = InterpolationTimeline::default();
        timeline.set_now(TickInstant::from(Tick(15)));
        app.insert_resource(timeline);
        app.finish();

        let mut history = ConfirmedHistory::<TestDiffComponent>::default();
        history.insert_present(Tick(10), TestDiffComponent(0));
        history.insert_present(Tick(20), TestDiffComponent(10));
        let entity = app
            .world_mut()
            .spawn((Interpolated, TestDiffComponent(99), history))
            .id();

        app.world_mut()
            .run_system_once(apply_interpolation)
            .unwrap();

        assert_eq!(
            app.world().get::<TestDiffComponent>(entity),
            Some(&TestDiffComponent(10))
        );
    }

    /// Checks that interpolation marker setup can be registered before replication metadata.
    #[test]
    fn marker_functions_can_precede_replication_component_registration() {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconPlugins,
            crate::plugin::InterpolationMarkerPlugin,
            crate::plugin::InterpolationPlugin,
        ));

        app.interpolate_with::<TestComp>(InterpolationFns::interpolate(lerp));
        app.component::<TestComp>().replicate();
        app.finish();

        let registry = app.world().resource::<InterpolationRegistry>();
        assert!(registry.finalized);
        assert!(
            registry
                .interpolated_marker_fns
                .contains(&ComponentKind::of::<TestComp>())
        );
    }

    #[test]
    #[should_panic(
        expected = "cannot register interpolation rules after InterpolationRegistry has been finalized"
    )]
    fn finalized_registry_rejects_rule_registration() {
        let mut registry = InterpolationRegistry::default();
        let mut world = World::new();
        let fns = InterpolationFns::history_only();
        let rule = component_rule::<TestComp, ()>(
            &mut world,
            fns,
            InterpolationRuleConfig::default(),
            update_history_archetype_erased::<TestComp>,
            insert_confirmed_history::<TestComp>,
        );
        registry.finalize();
        registry.insert_rule(rule);
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
            sample_for_rule(&registry, rule_id, &history, Tick(30), 0.0),
            Some(HistoryState::Updated(TestComp(10.0)))
        );
        assert_eq!(
            sample_for_rule(&registry, rule_id, &history, Tick(20), 0.5),
            Some(HistoryState::Updated(TestComp(10.0)))
        );
    }

    #[test]
    fn sample_returns_start_value_with_single_keyframe() {
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(42.0));

        let (registry, rule_id) = registry();
        assert_eq!(
            sample_for_rule(&registry, rule_id, &history, Tick(5), 0.0),
            None
        );
        assert_eq!(
            sample_for_rule(&registry, rule_id, &history, Tick(10), 0.0),
            Some(HistoryState::Updated(TestComp(42.0)))
        );
        assert_eq!(
            sample_for_rule(&registry, rule_id, &history, Tick(50), 0.5),
            Some(HistoryState::Updated(TestComp(42.0)))
        );
    }

    #[test]
    fn inserts_history_when_interpolated_added_after_component_is_already_replicated() {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconPlugins,
            crate::plugin::InterpolationMarkerPlugin,
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
        app.world_mut().run_schedule(bevy_app::Update);
        assert!(
            app.world()
                .get::<ConfirmedHistory<TestComp>>(entity)
                .is_none()
        );

        app.world_mut().entity_mut(entity).insert(Interpolated);
        app.world_mut().run_schedule(bevy_app::Update);

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

    /// Checks that a winning no-history rule prevents lower-rule history initialization.
    #[test]
    fn no_history_winner_does_not_initialize_lower_rule_history() {
        let mut app = App::new();
        app.add_plugins((
            StatesPlugin,
            RepliconPlugins,
            crate::plugin::InterpolationMarkerPlugin,
            crate::plugin::InterpolationPlugin,
        ));
        app.insert_resource(ReplicationCheckpointMap::default());
        app.component::<TestComp>().replicate();
        app.interpolate_with::<TestComp>(InterpolationFns::interpolate(lerp));
        app.interpolate_with_priority_filtered::<TestComp, With<NoHistory>>(
            100,
            InterpolationFns::no_history(lerp),
        );
        app.finish();

        let replicon_tick = record_checkpoint(&mut app, 42);
        let entity = app
            .world_mut()
            .spawn((TestComp(2.0), NoHistory, ConfirmHistory::new(replicon_tick)))
            .id();
        app.world_mut().entity_mut(entity).insert(Interpolated);
        app.world_mut().run_schedule(bevy_app::Update);

        assert_eq!(app.world().get::<TestComp>(entity), Some(&TestComp(2.0)));
        assert!(
            !app.world()
                .entity(entity)
                .contains::<ConfirmedHistory<TestComp>>()
        );
    }

    /// Checks history and apply ownership for every live/history presence combination.
    #[test]
    fn resolved_rule_ownership_covers_live_and_history_presence_matrix() {
        let mut world = World::new();
        let mut registry = InterpolationRegistry::default();

        let first_rule = registry.insert_rule(component_rule::<TestComp, ()>(
            &mut world,
            InterpolationFns::interpolate(lerp),
            InterpolationRuleConfig { priority: 1 },
            update_history_archetype_erased::<TestComp>,
            insert_confirmed_history::<TestComp>,
        ));
        let second_rule = registry.insert_rule(component_rule::<TestComp2, ()>(
            &mut world,
            InterpolationFns::interpolate(lerp2),
            InterpolationRuleConfig { priority: 1 },
            update_history_archetype_erased::<TestComp2>,
            insert_confirmed_history::<TestComp2>,
        ));

        let bundle_members = alloc::vec![
            build_rule_member::<TestComp>(
                &mut world,
                true,
                true,
                update_history_archetype_erased::<TestComp>,
                insert_confirmed_history::<TestComp>,
            ),
            build_rule_member::<TestComp2>(
                &mut world,
                true,
                true,
                update_history_archetype_erased::<TestComp2>,
                insert_confirmed_history::<TestComp2>,
            ),
        ];
        let bundle_rule =
            registry.insert_rule(InterpolationRule::new::<(TestComp, TestComp2), ()>(
                world.components(),
                InterpolationFns::interpolate(bundle_lerp),
                bundle_members,
                2,
                Some(
                    <(TestComp, TestComp2) as TupleInterpolationBundle>::apply_archetype
                        as ErasedApplyInterpolationFn,
                ),
                Some(
                    <(TestComp, TestComp2) as TupleInterpolationBundle>::apply_frame_archetype
                        as ErasedApplyFrameInterpolationFn,
                ),
            ));

        let both_live_no_history = world.spawn((TestComp(0.0), TestComp2(0.0))).id();
        let both_live_first_history = world
            .spawn((
                TestComp(0.0),
                TestComp2(0.0),
                ConfirmedHistory::<TestComp>::default(),
                FrameInterpolationHistory::<TestComp>::default(),
            ))
            .id();
        // This archetype represents both delayed insertion of TestComp2 before
        // its insertion tick and delayed removal after its removal tick.
        let only_first_both_histories = world
            .spawn((
                TestComp(0.0),
                ConfirmedHistory::<TestComp>::default(),
                ConfirmedHistory::<TestComp2>::default(),
                FrameInterpolationHistory::<TestComp>::default(),
                FrameInterpolationHistory::<TestComp2>::default(),
            ))
            .id();
        let only_second_both_histories = world
            .spawn((
                TestComp2(0.0),
                ConfirmedHistory::<TestComp>::default(),
                ConfirmedHistory::<TestComp2>::default(),
                FrameInterpolationHistory::<TestComp>::default(),
                FrameInterpolationHistory::<TestComp2>::default(),
            ))
            .id();
        let only_first_own_history = world
            .spawn((
                TestComp(0.0),
                ConfirmedHistory::<TestComp>::default(),
                FrameInterpolationHistory::<TestComp>::default(),
            ))
            .id();
        let only_second_own_history = world
            .spawn((
                TestComp2(0.0),
                ConfirmedHistory::<TestComp2>::default(),
                FrameInterpolationHistory::<TestComp2>::default(),
            ))
            .id();
        let histories_only = world
            .spawn((
                ConfirmedHistory::<TestComp>::default(),
                ConfirmedHistory::<TestComp2>::default(),
                FrameInterpolationHistory::<TestComp>::default(),
                FrameInterpolationHistory::<TestComp2>::default(),
            ))
            .id();
        let no_live_or_history = world.spawn_empty().id();

        let ownership = |rule_id, owns_history, owns_apply| ResolvedRule {
            rule_id,
            owns_history,
            owns_apply,
        };
        let cases = alloc::vec![
            (
                "both live, no history yet",
                both_live_no_history,
                alloc::vec![ownership(bundle_rule, true, true)],
            ),
            (
                "both live, one history missing",
                both_live_first_history,
                alloc::vec![ownership(bundle_rule, true, true)],
            ),
            (
                "delayed insertion/removal of second member",
                only_first_both_histories,
                alloc::vec![
                    ownership(bundle_rule, true, false),
                    ownership(first_rule, false, true),
                ],
            ),
            (
                "delayed insertion/removal of first member",
                only_second_both_histories,
                alloc::vec![
                    ownership(bundle_rule, true, false),
                    ownership(second_rule, false, true),
                ],
            ),
            (
                "second member and its history absent",
                only_first_own_history,
                alloc::vec![ownership(first_rule, true, true)],
            ),
            (
                "first member and its history absent",
                only_second_own_history,
                alloc::vec![ownership(second_rule, true, true)],
            ),
            (
                "both histories retained, both live members absent",
                histories_only,
                alloc::vec![ownership(bundle_rule, true, false)],
            ),
            (
                "no live members or histories",
                no_live_or_history,
                alloc::vec![],
            ),
        ];

        // History contents and change status deliberately do not appear in
        // this matrix. Ownership depends only on archetype presence; sampling
        // and history maintenance interpret Updated/Removed entries later.
        let mut scratch = RuleResolutionScratch::default();
        for target in [RuleTarget::Default, RuleTarget::Frame] {
            for (name, entity, expected) in &cases {
                let entity_ref = world.entity(*entity);
                let archetype = entity_ref.archetype();
                let actual = registry.resolved_rules_for_archetype(
                    world.components(),
                    archetype,
                    target,
                    &mut scratch,
                );
                assert_eq!(
                    actual,
                    expected.as_slice(),
                    "unexpected {target:?} ownership for {name}"
                );
            }
        }
    }

    /// Checks that a bundle cannot list the same component type more than once.
    #[test]
    #[should_panic(
        expected = "interpolation bundle rules cannot contain duplicate component types"
    )]
    fn bundle_rule_rejects_duplicate_component_types() {
        let mut app = App::new();
        app.interpolate_bundle_with::<(TestComp, TestComp)>(InterpolationFns::disabled());
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
