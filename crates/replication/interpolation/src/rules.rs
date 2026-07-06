//! Shared interpolation rule types and type-erased callbacks.
//!
//! Bundle-specific rule implementations live in [`crate::rules::bundle`].
//! Frame-interpolation callbacks that reuse these rules live in
//! [`crate::rules::frame_interpolate`].

pub mod bundle;
pub mod frame_interpolate;

pub use bundle::InterpolationBundle;

pub(crate) use bundle::TupleInterpolationBundle;

use self::frame_interpolate::{FrameHistoryComponent, FrameInterpolationFns};
use crate::registry::InterpolationRegistry;
use alloc::vec::Vec;
use bevy_ecs::archetype::Archetype;
use bevy_ecs::component::{ComponentId, Components, StorageType};
use bevy_ecs::query::QueryFilter;
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy_math::{
    Curve,
    curve::{Ease, EaseFunction, EasingCurve},
};
use core::any::TypeId;
use core::marker::PhantomData;
use lightyear_core::prelude::Tick;
use lightyear_replication::deferred_entity::DeferredEntityCommands;
use lightyear_replication::registry::{ComponentKind, LerpFn};

/// Configuration for an interpolation rule.
///
/// Rules are evaluated per interpolated archetype and component type. If
/// multiple rules match the same archetype, the rule with the highest priority
/// is selected. If the selected rule omits history or apply work, Lightyear
/// skips that work instead of falling back to a lower-priority rule.
///
/// # Examples
///
/// Give a marker-filtered rule priority over the default `F = ()` rule:
///
/// ```rust,ignore
/// use bevy_ecs::prelude::*;
/// use lightyear_interpolation::prelude::*;
///
/// #[derive(Component, Clone, PartialEq)]
/// struct Position(f32);
///
/// #[derive(Component)]
/// struct SmoothVisuals;
///
/// fn smooth_lerp(start: Position, end: Position, t: f32) -> Position {
///     Position(start.0 + (end.0 - start.0) * t)
/// }
///
/// app.interpolate_with::<Position>(InterpolationFns::interpolate(smooth_lerp));
/// app.interpolate_with_priority_filtered::<Position, With<SmoothVisuals>>(
///     100,
///     InterpolationFns::interpolate(smooth_lerp),
/// );
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct InterpolationRuleConfig {
    /// Priority used to choose between matching interpolation rules.
    ///
    /// Higher values are selected first. The config's default is `0`; the
    /// public `interpolate_with` registration methods use the number of
    /// components in the interpolation target as their default priority. This
    /// means a default `(Position, Rotation)` rule wins over default
    /// single-component rules on the same archetype. Matching rules with the
    /// same priority use registration order, with earlier registrations
    /// selected first.
    pub priority: usize,
}

/// Functions used by an interpolation rule.
///
/// The constructors describe which work Lightyear owns:
///
/// - [`Self::interpolate`] stores received values in
///   [`ConfirmedHistory`](lightyear_core::prelude::ConfirmedHistory),
///   prepares that history, samples it, and applies the result to the live
///   component. For bundle interpolation, each component is stored in its own
///   history before the tuple interpolation function is called.
/// - [`Self::history_only`] stores and prepares history but does not apply the
///   live component. This is the usual choice when a user system performs
///   custom interpolation.
/// - [`Self::history_only`] combined with
///   [`InterpolationFnsExt::interpolate`] stores and prepares history and keeps
///   an interpolation function. Delayed interpolation still does not apply the
///   live component, but frame interpolation can reuse the function.
/// - [`Self::no_history`] registers an interpolation function without owning
///   delayed interpolation history or delayed live-component presence. Frame
///   interpolation and correction can still reuse the same rule.
/// - [`Self::disabled`] registers no history and no interpolation function. Use
///   it as a high-priority filtered rule to opt matching entities out of a
///   broader interpolation rule.
///
/// # Examples
///
/// Use Lightyear's full interpolation pipeline:
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
///
/// Keep histories but run custom interpolation in your own system:
///
/// ```rust,ignore
/// use bevy_ecs::prelude::*;
/// use lightyear_interpolation::prelude::*;
///
/// #[derive(Component, Clone, PartialEq)]
/// struct Position(f32);
///
/// app.interpolate_with::<Position>(InterpolationFns::history_only());
///
/// app.add_systems(Update, custom_interpolation.in_set(InterpolationSystems::Interpolate));
/// ```
#[derive(Debug, Clone, Copy)]
pub struct InterpolationFns<C> {
    pub(crate) interpolation: Option<LerpFn<C>>,
    pipeline: InterpolationPipeline,
    _marker: PhantomData<fn(C)>,
}

#[derive(Debug, Clone, Copy)]
enum InterpolationPipeline {
    Full,
    HistoryOnly,
    NoHistory,
    FrameHistoryOnly,
    Disabled,
}

impl<C> InterpolationFns<C> {
    /// Enables the full Lightyear interpolation pipeline for `C`.
    ///
    /// Incoming updates are stored in
    /// [`ConfirmedHistory<C>`](lightyear_core::prelude::ConfirmedHistory),
    /// prepared every frame, and the provided interpolation function is used to
    /// apply the live component at the interpolation timeline.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// # use bevy_ecs::prelude::*;
    /// # use lightyear_interpolation::prelude::*;
    /// # #[derive(Component, Clone, PartialEq)]
    /// # struct Position(f32);
    /// fn lerp_position(start: Position, end: Position, t: f32) -> Position {
    ///     Position(start.0 + (end.0 - start.0) * t)
    /// }
    ///
    /// app.interpolate_with::<Position>(InterpolationFns::interpolate(lerp_position));
    /// ```
    pub fn interpolate(interpolation: LerpFn<C>) -> Self {
        Self {
            interpolation: Some(interpolation),
            pipeline: InterpolationPipeline::Full,
            _marker: PhantomData,
        }
    }

    /// Stores and prepares interpolation history, but does not apply `C`.
    ///
    /// Use this when Lightyear should receive component updates into
    /// [`ConfirmedHistory<C>`](lightyear_core::prelude::ConfirmedHistory), but visible interpolation is handled by a user
    /// system. For example, a system may sample several histories and write a
    /// render-only component.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// # use bevy_ecs::prelude::*;
    /// # use lightyear_interpolation::prelude::*;
    /// # #[derive(Component, Clone, PartialEq)]
    /// # struct Position(f32);
    /// app.interpolate_with::<Position>(InterpolationFns::history_only());
    /// ```
    pub fn history_only() -> Self {
        Self {
            interpolation: None,
            pipeline: InterpolationPipeline::HistoryOnly,
            _marker: PhantomData,
        }
    }

    /// Registers an interpolation function without delayed interpolation history.
    ///
    /// This is useful when a component is not delayed-interpolated through
    /// [`Interpolated`](lightyear_core::prelude::Interpolated), but entities
    /// with [`FrameInterpolate`](lightyear_core::prelude::FrameInterpolate)
    /// should still reuse the same interpolation function for visual smoothing
    /// and correction. Unlike [`Self::disabled`], this still participates in
    /// frame interpolation and correction because it stores an interpolation
    /// function.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// # use bevy_ecs::prelude::*;
    /// # use lightyear_interpolation::prelude::*;
    /// # #[derive(Component, Clone, PartialEq)]
    /// # struct Position(f32);
    /// # fn lerp_position(start: Position, end: Position, t: f32) -> Position {
    /// #     Position(start.0 + (end.0 - start.0) * t)
    /// # }
    /// app.interpolate_with::<Position>(InterpolationFns::no_history(lerp_position));
    /// ```
    pub fn no_history(interpolation: LerpFn<C>) -> Self {
        Self {
            interpolation: Some(interpolation),
            pipeline: InterpolationPipeline::NoHistory,
            _marker: PhantomData,
        }
    }

    /// Disables Lightyear interpolation work for matching entities.
    ///
    /// A disabled high-priority rule can be used to exclude a filtered set of
    /// entities from a broader default interpolation rule. If the disabled rule
    /// is selected for an archetype, Lightyear does not fall back to lower
    /// priority matching rules for that component. Unlike [`Self::no_history`],
    /// this does not register an interpolation function for frame interpolation
    /// or correction.
    ///
    /// This is useful when most entities should interpolate but a marked subset
    /// should snap, be driven by a custom visual system, or temporarily opt out
    /// during a mode change.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// # use bevy_ecs::prelude::*;
    /// # use lightyear_interpolation::prelude::*;
    /// # #[derive(Component, Clone, PartialEq)]
    /// # struct Position(f32);
    /// #[derive(Component)]
    /// struct SnapOnly;
    ///
    /// app.linear_interpolate::<Position>();
    ///
    /// // Entities with `SnapOnly` match both rules. This rule has higher
    /// // priority, so it blocks the broader default `Position` interpolation.
    /// app.interpolate_with_priority_filtered::<Position, With<SnapOnly>>(
    ///     100,
    ///     InterpolationFns::disabled(),
    /// );
    /// ```
    pub fn disabled() -> Self {
        Self {
            interpolation: None,
            pipeline: InterpolationPipeline::Disabled,
            _marker: PhantomData,
        }
    }

    // Internal synthetic rule used for bundle `no_history` members. A tuple
    // rule owns the interpolation function, but frame interpolation still needs
    // per-component `FrameInterpolationHistory<C>` entries for each member.
    pub(crate) fn frame_history_only() -> Self {
        Self {
            interpolation: None,
            pipeline: InterpolationPipeline::FrameHistoryOnly,
            _marker: PhantomData,
        }
    }

    pub(crate) fn owns_interpolation_history(&self) -> bool {
        matches!(
            self.pipeline,
            InterpolationPipeline::Full | InterpolationPipeline::HistoryOnly
        )
    }

    pub(crate) fn applies_interpolation_component(&self) -> bool {
        matches!(self.pipeline, InterpolationPipeline::Full)
    }

    pub(crate) fn owns_frame_history(&self) -> bool {
        matches!(
            self.pipeline,
            InterpolationPipeline::Full
                | InterpolationPipeline::HistoryOnly
                | InterpolationPipeline::NoHistory
                | InterpolationPipeline::FrameHistoryOnly
        )
    }

    pub(crate) fn applies_frame_component(&self) -> bool {
        self.interpolation.is_some()
            && matches!(
                self.pipeline,
                InterpolationPipeline::Full
                    | InterpolationPipeline::HistoryOnly
                    | InterpolationPipeline::NoHistory
            )
    }
}

/// Fluent helpers for adding interpolation functions to [`InterpolationFns`].
///
/// These are most useful with [`InterpolationFns::history_only`], where
/// Lightyear should own history/presence but a custom system owns delayed
/// interpolation while frame interpolation and correction can still reuse the
/// interpolation function.
///
/// # Examples
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
/// app.interpolate_with::<Position>(
///     InterpolationFns::history_only().interpolate(lerp_position),
/// );
/// ```
pub trait InterpolationFnsExt<C> {
    /// Stores `interpolation` on this rule without changing which pipeline
    /// stages the rule owns.
    fn interpolate(self, interpolation: LerpFn<C>) -> Self;

    /// Stores a linear [`Ease`] interpolation function on this rule.
    fn linear_interpolate(self) -> Self
    where
        C: Ease + Clone;
}

impl<C> InterpolationFnsExt<C> for InterpolationFns<C> {
    fn interpolate(mut self, interpolation: LerpFn<C>) -> Self {
        self.interpolation = Some(interpolation);
        self
    }

    fn linear_interpolate(self) -> Self
    where
        C: Ease + Clone,
    {
        self.interpolate(linear_lerp::<C>)
    }
}

fn linear_lerp<C>(start: C, other: C, t: f32) -> C
where
    C: Ease + Clone,
{
    let curve = EasingCurve::new(start, other, EaseFunction::Linear);
    curve.sample_unchecked(t)
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[doc(hidden)]
pub struct InterpolationRuleId(pub(crate) usize);

impl InterpolationRuleId {
    pub(crate) fn index(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct UpdateHistoryContext {
    pub(crate) server_complete_tick: Option<Tick>,
    pub(crate) current_interpolate_tick: Tick,
    pub(crate) interpolation_overstep: f32,
    pub(crate) interpolation: Option<ErasedLerpFn>,
}

/// Type-erased interpolation function stored by the interpolation registry.
///
/// Typed functions are registered as [`LerpFn<C>`] and erased internally so
/// rules for different components and bundles can share the same cache.
pub type ErasedInterpolationFn = unsafe fn();

pub(crate) type ErasedLerpFn = ErasedInterpolationFn;

/// Returns whether a cached interpolation rule matches an archetype.
pub(crate) type MatchesArchetypeFn = fn(&Components, &Archetype) -> bool;

/// Type-erased function that updates history for one component on one archetype.
///
/// Structural changes to the live component set are recorded into
/// [`DeferredEntityCommands`] and flushed after the query scan finishes.
pub(crate) type ErasedUpdateHistoryFn = fn(
    UnsafeWorldCell,
    &Archetype,
    &CachedInterpolationComponent,
    UpdateHistoryContext,
    Option<&mut bevy_replicon::shared::replication::storage::ReplicationStorage>,
    &mut DeferredEntityCommands,
);

/// Context passed to type-erased interpolation apply functions.
#[derive(Debug, Clone, Copy)]
#[doc(hidden)]
pub struct ApplyInterpolationContext {
    pub(crate) interpolation_tick: Tick,
    pub(crate) interpolation_overstep: f32,
}

/// Type-erased function that applies one selected interpolation rule to one archetype.
///
/// Component and bundle rules use the same function shape. The cached
/// archetype stores these after priority and overlap resolution, so the apply
/// phase only needs to call each function in order.
pub(crate) type ErasedApplyInterpolationFn = fn(
    UnsafeWorldCell,
    &Archetype,
    &InterpolationRegistry,
    InterpolationRuleId,
    ApplyInterpolationContext,
);

/// Cached typed component metadata needed by the type-erased history updater.
///
/// One value is stored per selected history-owning rule on each cached
/// interpolated archetype. It lets the update system decide whether the
/// corresponding live component is currently present on that archetype.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CachedInterpolationComponent {
    /// Component kind whose history is updated.
    pub(crate) kind: ComponentKind,
    /// Component ID for `ConfirmedHistory<C>`.
    pub(crate) history_component_id: ComponentId,
    /// Storage backing `ConfirmedHistory<C>` on the cached archetype.
    pub(crate) history_storage: StorageType,
    /// Whether the live component `C` is present on the cached archetype.
    pub(crate) live_component_present: bool,
    /// Type-erased history update function for `C`.
    pub(crate) update_history: ErasedUpdateHistoryFn,
    /// Optional interpolation function used when sampling the history.
    pub(crate) interpolation: Option<ErasedLerpFn>,
}

impl CachedInterpolationComponent {
    pub(crate) fn kind(&self) -> ComponentKind {
        self.kind
    }

    pub(crate) fn history_component_id(&self) -> ComponentId {
        self.history_component_id
    }

    pub(crate) fn history_storage(&self) -> StorageType {
        self.history_storage
    }

    pub(crate) fn live_component_present(&self) -> bool {
        self.live_component_present
    }

    pub(crate) fn update_history(&self) -> ErasedUpdateHistoryFn {
        self.update_history
    }

    pub(crate) fn interpolation(&self) -> Option<ErasedLerpFn> {
        self.interpolation
    }
}

/// Cached type-erased apply metadata for one selected interpolation rule.
///
/// Values are stored on [`crate::archetypes::CachedInterpolatedArchetype`]
/// after rule priority and bundle/component overlap have been resolved.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CachedInterpolationApply {
    /// ID of the selected rule whose interpolation function should run.
    pub(crate) rule_id: InterpolationRuleId,
    /// Type-erased function that writes this rule's live component(s).
    pub(crate) apply_interpolation: ErasedApplyInterpolationFn,
}

impl CachedInterpolationApply {
    pub(crate) fn rule_id(&self) -> InterpolationRuleId {
        self.rule_id
    }

    pub(crate) fn apply_interpolation(&self) -> ErasedApplyInterpolationFn {
        self.apply_interpolation
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ErasedInterpolationFns {
    pub(crate) interpolation: Option<ErasedLerpFn>,
    pub(crate) update_history: Option<ErasedUpdateHistoryFn>,
    pub(crate) apply_interpolation: Option<ErasedApplyInterpolationFn>,
    pub(crate) history_component_id: Option<ComponentId>,
    pub(crate) live_component_id: Option<ComponentId>,
    pub(crate) write_component_ids: Vec<ComponentId>,
    pub(crate) frame: Option<FrameInterpolationFns>,
}

impl ErasedInterpolationFns {
    pub(crate) fn from_typed<S: 'static>(
        fns: InterpolationFns<S>,
        update_history: Option<ErasedUpdateHistoryFn>,
        apply_interpolation: Option<ErasedApplyInterpolationFn>,
        history_component_id: Option<ComponentId>,
        live_component_id: Option<ComponentId>,
        write_component_ids: Vec<ComponentId>,
        frame: Option<FrameInterpolationFns>,
    ) -> Self {
        Self {
            interpolation: fns
                .interpolation
                .map(|f| unsafe { core::mem::transmute::<LerpFn<S>, unsafe fn()>(f) }),
            update_history,
            apply_interpolation,
            history_component_id,
            live_component_id,
            write_component_ids,
            frame,
        }
    }
}

/// Key used to select between interpolation rules.
///
/// A rule kind is the type registered by the user: for a single component this
/// is `C`, and for bundle interpolation this is the tuple type `(A, B, ...)`.
/// It is intentionally separate from [`ComponentKind`], which is reserved for
/// actual ECS component members that a rule reads, writes, or claims during
/// overlap resolution.
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq)]
pub struct RuleKind(TypeId);

impl RuleKind {
    #[doc(hidden)]
    pub fn of<T: 'static>() -> Self {
        Self(TypeId::of::<T>())
    }
}

/// One interpolation rule registered by [`crate::registry::AppInterpolationExt`].
///
/// A rule has a [`RuleKind`] used for cache lookup, a list of real component
/// `members` it owns or writes, erased functions describing which work
/// Lightyear owns, and an archetype filter. Rules are sorted by priority per
/// kind, so
/// [`InterpolationRegistry::select_rule_for_archetype`] can return the first
/// matching rule.
#[derive(Debug, Clone)]
pub struct InterpolationRule {
    /// Rule key used when selecting a rule for a component or bundle target.
    pub(crate) kind: RuleKind,
    /// Components owned by this rule. Bundle rules have more than one member.
    pub(crate) members: Vec<ComponentKind>,
    /// Higher-priority rules are selected before lower-priority rules.
    pub(crate) priority: usize,
    /// Type-erased interpolation/history/apply functions for this rule.
    pub(crate) fns: ErasedInterpolationFns,
    /// Archetype-level filter predicate compiled from the rule filter type.
    pub(crate) matches_archetype: MatchesArchetypeFn,
}

impl InterpolationRule {
    pub(crate) fn owns_history(&self) -> bool {
        self.fns.update_history.is_some() && self.fns.history_component_id.is_some()
    }

    pub(crate) fn applies_component(&self) -> bool {
        self.fns.apply_interpolation.is_some()
    }

    pub(crate) fn owns_frame_history(&self) -> bool {
        self.fns
            .frame
            .as_ref()
            .is_some_and(FrameInterpolationFns::owns_history)
    }

    pub(crate) fn applies_frame_component(&self) -> bool {
        self.fns
            .frame
            .as_ref()
            .is_some_and(FrameInterpolationFns::applies_component)
    }

    pub(crate) fn frame_history_component(&self) -> Option<FrameHistoryComponent> {
        self.fns
            .frame
            .as_ref()
            .and_then(|frame| frame.history_component(*self.members.first()?))
    }

    #[doc(hidden)]
    pub fn members(&self) -> &[ComponentKind] {
        &self.members
    }

    #[doc(hidden)]
    pub fn priority(&self) -> usize {
        self.priority
    }
}

pub(crate) fn matches_filter<F>(components: &Components, archetype: &Archetype) -> bool
where
    F: QueryFilter + 'static,
{
    F::get_state(components)
        .is_some_and(|state| F::matches_component_set(&state, &|id| archetype.contains(id)))
}
