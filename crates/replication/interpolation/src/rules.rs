//! Shared interpolation rule types and type-erased callbacks.
//!
//! Bundle-specific rule implementations live in [`crate::rules::bundle`].
//! Frame-interpolation callbacks that reuse these rules live in
//! [`crate::rules::frame_interpolate`].

pub mod bundle;
pub mod frame_interpolate;

pub use bundle::InterpolationBundle;

pub(crate) use bundle::TupleInterpolationBundle;

use self::frame_interpolate::{
    CachedFrameInterpolationApply, CachedFrameInterpolationHistoryComponent,
    ErasedApplyFrameInterpolationFn, ErasedRestoreFrameHistoryFn, ErasedUpdateFrameHistoryFn,
};
use crate::registry::InterpolationRegistry;
use alloc::{boxed::Box, vec::Vec};
use bevy_ecs::archetype::Archetype;
use bevy_ecs::component::{ComponentId, Components, StorageType};
use bevy_ecs::prelude::{Commands, Entity};
use bevy_ecs::query::{ArchetypeFilter, FilteredAccess};
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy_math::{
    Curve,
    curve::{Ease, EaseFunction, EasingCurve},
};
use core::any::Any;
use core::fmt;
use core::marker::PhantomData;
use core::time::Duration;
use lightyear_core::prelude::Tick;
use lightyear_replication::deferred_entity::DeferredEntityCommands;
use lightyear_replication::registry::{ComponentKind, LerpFn};

/// Context passed to interpolation functions that need more than a normalized fraction.
///
/// Most interpolation functions only need [`Self::t`] and can use the
/// existing [`LerpFn`] shape. More advanced functions, such as Hermite
/// interpolation with endpoint velocities, can use [`Self::sample_delta_secs`]
/// to scale velocities by the actual sample interval.
///
/// Delayed interpolation computes [`Self::sample_delta_secs`] from the
/// bracketing history ticks and the configured tick duration. Frame
/// interpolation and visual correction populate it from the fixed timestep.
///
/// # Examples
///
/// ```rust,ignore
/// use lightyear_interpolation::prelude::*;
/// fn interpolate_position(
///     start: Position,
///     end: Position,
///     ctx: InterpolationSampleContext,
/// ) -> Position {
///     let _sample_delta_secs = ctx.sample_delta_secs;
///     Position(start.0 + (end.0 - start.0) * ctx.t)
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InterpolationSampleContext {
    /// Normalized interpolation fraction in `[0.0, 1.0]`.
    pub t: f32,
    /// Duration between the bracketing samples, in seconds, when available.
    pub sample_delta_secs: Option<f32>,
}

impl InterpolationSampleContext {
    /// Creates a context from a normalized interpolation fraction and an
    /// optional sample interval.
    pub fn new(t: f32, sample_delta_secs: Option<f32>) -> Self {
        Self {
            t,
            sample_delta_secs,
        }
    }

    /// Creates a context when only a normalized interpolation fraction is known.
    pub fn from_t(t: f32) -> Self {
        Self {
            t,
            sample_delta_secs: None,
        }
    }

    pub(crate) fn from_ticks(
        start_tick: Tick,
        end_tick: Tick,
        interpolation_tick: Tick,
        interpolation_overstep: f32,
        tick_duration: Option<Duration>,
    ) -> Self {
        let tick_delta = end_tick - start_tick;
        let t = if tick_delta > 0 {
            (((interpolation_tick - start_tick) as f32 + interpolation_overstep)
                / tick_delta as f32)
                .clamp(0.0, 1.0)
        } else {
            0.0
        };
        let sample_delta_secs = tick_duration
            .and_then(|tick_duration| sample_delta_secs(start_tick, end_tick, tick_duration));
        Self {
            t,
            sample_delta_secs,
        }
    }
}

fn sample_delta_secs(start_tick: Tick, end_tick: Tick, tick_duration: Duration) -> Option<f32> {
    let ticks = end_tick - start_tick;
    (ticks > 0).then_some(tick_duration.as_secs_f32() * ticks as f32)
}

/// Context-aware interpolation callback stored by a rule.
///
/// Unlike [`LerpFn`], this receives the duration between the sampled states
/// when the caller can determine it.
pub type ContextInterpolationFn<C> =
    fn(start: C, other: C, context: InterpolationSampleContext) -> C;

/// Interpolation function stored by a rule.
///
/// `Lerp` preserves the `fn(start, end, t)` API. `Contextual` is used
/// for interpolation that needs sample timing, such as Hermite interpolation
/// with velocities.
#[derive(Clone, Copy)]
pub enum InterpolationFn<C> {
    /// Interpolation function that receives a normalized interpolation fraction.
    Lerp(LerpFn<C>),
    /// Context-aware interpolation function.
    Contextual(ContextInterpolationFn<C>),
}

impl<C> fmt::Debug for InterpolationFn<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lerp(_) => f.write_str("InterpolationFn::Lerp(..)"),
            Self::Contextual(_) => f.write_str("InterpolationFn::Contextual(..)"),
        }
    }
}

impl<C> InterpolationFn<C> {
    /// Applies the interpolation function using the provided context.
    pub fn interpolate(&self, start: C, other: C, context: InterpolationSampleContext) -> C {
        match self {
            Self::Lerp(interpolation) => interpolation(start, other, context.t),
            Self::Contextual(interpolation) => interpolation(start, other, context),
        }
    }
}

/// Configuration for an interpolation rule.
///
/// Rules are evaluated per interpolated archetype and component type. If
/// multiple rules match the same archetype, priority is resolved independently
/// for history and apply behavior. Histories can represent temporarily absent
/// members, while apply requires every live member. A bundle may therefore
/// maintain `(A, B)` histories while a standalone rule applies `A` until `B`
/// becomes live again.
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
///   custom interpolation: the history will be populated by Lightyear, but the
///   user should write their own system to perform interpolation. You can still
///   specify a default interpolation fn that can be used for other purposes, such
///   as frame interpolation.
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
    pub(crate) interpolation: Option<InterpolationFn<C>>,
    pipeline: InterpolationPipeline,
    _marker: PhantomData<fn(C)>,
}

#[derive(Debug, Clone, Copy)]
enum InterpolationPipeline {
    Full,
    HistoryOnly,
    NoHistory,
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
            interpolation: Some(InterpolationFn::Lerp(interpolation)),
            pipeline: InterpolationPipeline::Full,
            _marker: PhantomData,
        }
    }

    /// Enables the full Lightyear interpolation pipeline with a callback that
    /// receives the sample interval.
    ///
    /// Use this for interpolation such as Hermite curves whose endpoint
    /// derivatives must be scaled by the duration between the sampled states.
    pub fn interpolate_with_context(interpolation: ContextInterpolationFn<C>) -> Self {
        Self {
            interpolation: Some(InterpolationFn::Contextual(interpolation)),
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
            interpolation: Some(InterpolationFn::Lerp(interpolation)),
            pipeline: InterpolationPipeline::NoHistory,
            _marker: PhantomData,
        }
    }

    /// Registers a context-aware interpolation function without delayed
    /// interpolation history.
    pub fn no_history_with_context(interpolation: ContextInterpolationFn<C>) -> Self {
        Self {
            interpolation: Some(InterpolationFn::Contextual(interpolation)),
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

/// Helpers for adding interpolation functions to [`InterpolationFns`].
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

    /// Stores a context-aware interpolation callback on this rule without
    /// changing which pipeline stages the rule owns.
    fn interpolate_with_context(self, interpolation: ContextInterpolationFn<C>) -> Self;

    /// Stores a linear [`Ease`] interpolation function on this rule.
    fn linear_interpolate(self) -> Self
    where
        C: Ease + Clone;
}

impl<C> InterpolationFnsExt<C> for InterpolationFns<C> {
    fn interpolate(mut self, interpolation: LerpFn<C>) -> Self {
        self.interpolation = Some(InterpolationFn::Lerp(interpolation));
        self
    }

    fn interpolate_with_context(mut self, interpolation: ContextInterpolationFn<C>) -> Self {
        self.interpolation = Some(InterpolationFn::Contextual(interpolation));
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

#[derive(Debug, Clone, Copy)]
pub(crate) struct UpdateHistoryContext {
    pub(crate) server_complete_tick: Option<Tick>,
    pub(crate) current_interpolate_tick: Tick,
}

/// The actual typed interpolation callback stored without its component or
/// bundle type. It only computes an output from sampled values; it does not
/// access the ECS world or histories.
pub(crate) struct ErasedInterpolation {
    inner: Box<dyn Any + Send + Sync + 'static>,
}

impl fmt::Debug for ErasedInterpolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ErasedInterpolation(..)")
    }
}

impl ErasedInterpolation {
    fn from_typed<S: 'static>(interpolation: InterpolationFn<S>) -> Self {
        Self {
            inner: Box::new(interpolation),
        }
    }

    pub(crate) fn typed<S: 'static>(&self) -> &InterpolationFn<S> {
        self.inner
            .downcast_ref::<InterpolationFn<S>>()
            .expect("interpolation rule kind and interpolation function type should match")
    }
}

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
    &UpdateHistoryContext,
    Option<&mut bevy_replicon::shared::replication::storage::ReplicationStorage>,
    &mut DeferredEntityCommands,
);

/// Type-erased function that initializes `ConfirmedHistory<C>` from an
/// entity's current replicated value.
pub(crate) type ErasedInsertConfirmedHistoryFn = fn(Entity, &mut Commands);

/// Context passed to type-erased interpolation apply functions.
#[derive(Debug, Clone, Copy)]
#[doc(hidden)]
pub struct ApplyInterpolationContext {
    pub(crate) interpolation_tick: Tick,
    pub(crate) interpolation_overstep: f32,
    pub(crate) tick_duration: Option<Duration>,
}

/// Type-erased archetype executor for one selected interpolation rule.
///
/// It fetches history values for every entity, invokes the rule's
/// [`ErasedInterpolation`], and writes the interpolated live component values.
/// Component and bundle rules use the same executor shape.
pub(crate) type ErasedApplyInterpolationFn = fn(
    UnsafeWorldCell,
    &Archetype,
    &InterpolationRegistry,
    InterpolationRuleId,
    ApplyInterpolationContext,
);

/// Cached typed component metadata needed by the type-erased history updater.
///
/// One value is stored per history operation owned by a resolved rule. It lets
/// the update system initialize a missing history from the current replicated
/// value or update an existing history. The component IDs and type-erased
/// callbacks are copied out of the rule when this cache is built, so history
/// updates do not need the rule or its ID afterward.
#[derive(Debug, Clone)]
pub(crate) struct CachedInterpolationComponent {
    /// Component ID for `ConfirmedHistory<C>`.
    pub(crate) history_component_id: ComponentId,
    /// Storage backing `ConfirmedHistory<C>` when it is present.
    pub(crate) history_storage: Option<StorageType>,
    /// Whether the history column is present on the cached archetype.
    pub(crate) history_component_present: bool,
    /// Whether the live component `C` is present on the cached archetype.
    pub(crate) live_component_present: bool,
    /// Type-erased history update function for `C`.
    pub(crate) update_history: ErasedUpdateHistoryFn,
    /// Type-erased history initializer for `C`.
    pub(crate) insert_history: ErasedInsertConfirmedHistoryFn,
}

/// Cached type-erased apply metadata for one selected interpolation rule.
///
/// Values are stored on [`crate::archetypes::CachedInterpolationPolicy`]
/// after rule priority and bundle/component overlap have been resolved. Apply
/// keeps the rule ID because the rule's typed interpolation function remains
/// stored in [`InterpolationRegistry`] and is retrieved when the callback runs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CachedInterpolationApply {
    /// ID of the selected rule whose interpolation function should run.
    pub(crate) rule_id: InterpolationRuleId,
    /// Type-erased function that writes this rule's live component(s).
    pub(crate) apply_interpolation: ErasedApplyInterpolationFn,
}

/// One interpolation rule registered by [`crate::registry::AppInterpolationExt`].
///
/// A rule has a list of real component `members` it owns or writes, erased
/// functions describing which work Lightyear owns, and an archetype filter.
/// Matching rules are sorted by priority before the registry resolves overlap
/// between component and bundle candidates.
#[derive(Debug)]
pub struct InterpolationRule {
    /// Components owned by this rule, including their history behavior.
    /// Bundle rules have more than one member.
    pub(crate) members: Vec<InterpolationRuleComponent>,
    /// Higher-priority rules are selected before lower-priority rules.
    pub(crate) priority: usize,
    /// Type-erased interpolation function for this rule's component or bundle.
    pub(crate) interpolation: Option<ErasedInterpolation>,
    /// Type-erased delayed-interpolation apply callback.
    pub(crate) apply_interpolation: Option<ErasedApplyInterpolationFn>,
    /// Type-erased frame-interpolation apply callback.
    pub(crate) apply_frame_interpolation: Option<ErasedApplyFrameInterpolationFn>,
    /// Archetype-level filter predicate compiled from the rule filter type.
    pub(crate) matches_archetype: MatchesArchetypeFn,
    /// Components whose presence is inspected by the archetype filter.
    pub(crate) filter_component_ids: Vec<ComponentId>,
}

impl InterpolationRule {
    pub(crate) fn new<S, F>(
        components: &Components,
        fns: InterpolationFns<S>,
        members: Vec<InterpolationRuleComponent>,
        priority: usize,
        apply_interpolation: Option<ErasedApplyInterpolationFn>,
        apply_frame_interpolation: Option<ErasedApplyFrameInterpolationFn>,
    ) -> Self
    where
        S: 'static,
        F: ArchetypeFilter + 'static,
    {
        let filter_state = F::get_state(components)
            .expect("interpolation rule filters should be initialized before registration");
        let mut filter_access = FilteredAccess::default();
        F::update_component_access(&filter_state, &mut filter_access);
        let mut filter_component_ids = Vec::new();
        for component_id in filter_access
            .with_filters()
            .chain(filter_access.without_filters())
        {
            if !filter_component_ids.contains(&component_id) {
                filter_component_ids.push(component_id);
            }
        }

        Self {
            members,
            priority,
            interpolation: fns.interpolation.map(ErasedInterpolation::from_typed::<S>),
            apply_interpolation,
            apply_frame_interpolation,
            matches_archetype: matches_filter::<F>,
            filter_component_ids,
        }
    }

    #[doc(hidden)]
    pub fn members(&self) -> impl Iterator<Item = ComponentKind> + '_ {
        self.members.iter().map(|member| member.kind)
    }

    /// Builds cached frame-apply metadata for this rule.
    #[doc(hidden)]
    pub fn cached_frame_apply(
        &self,
        rule_id: InterpolationRuleId,
    ) -> Option<CachedFrameInterpolationApply> {
        Some(CachedFrameInterpolationApply {
            rule_id,
            apply_frame_interpolation: self.apply_frame_interpolation?,
        })
    }

    /// Returns the frame-history operations owned by this rule that are
    /// relevant to `archetype`.
    #[doc(hidden)]
    pub fn cached_frame_history_components<'a>(
        &'a self,
        archetype: &'a Archetype,
    ) -> impl Iterator<Item = CachedFrameInterpolationHistoryComponent> + 'a {
        self.members.iter().filter_map(move |member| {
            let frame = member.frame?;
            let history_component_id = member.frame_history_component_id;
            let live_component_id = member.live_component_id;
            let history_component_present = archetype.contains(history_component_id);
            let live_component_present = archetype.contains(live_component_id);
            if !history_component_present && !live_component_present {
                return None;
            }
            Some(CachedFrameInterpolationHistoryComponent {
                kind: member.kind,
                history_component_id,
                history_storage: history_component_present
                    .then(|| archetype.get_storage_type(history_component_id))
                    .flatten(),
                history_component_present,
                live_component_id,
                live_component_present,
                update_frame_history: frame.update_history,
                restore_frame_history: frame.restore_history,
            })
        })
    }
}

/// One component owned by an interpolation rule.
///
/// The history callbacks live on the member instead of in a separate global
/// provider catalog. Each selected rule therefore carries the history behavior
/// needed to initialize and maintain its members as well as its apply behavior.
#[derive(Debug, Clone, Copy)]
pub(crate) struct InterpolationRuleComponent {
    /// Type identity used to prevent rules from claiming the same member.
    pub(crate) kind: ComponentKind,
    /// Bevy component IDs whose presence can make this member available.
    pub(crate) live_component_id: ComponentId,
    pub(crate) confirmed_history_component_id: ComponentId,
    pub(crate) frame_history_component_id: ComponentId,
    /// History behavior owned by this rule, independently of the IDs above.
    pub(crate) confirmed: Option<ConfirmedHistoryFns>,
    pub(crate) frame: Option<FrameHistoryFns>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ConfirmedHistoryFns {
    pub(crate) update_history: ErasedUpdateHistoryFn,
    pub(crate) insert_history: ErasedInsertConfirmedHistoryFn,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameHistoryFns {
    pub(crate) update_history: ErasedUpdateFrameHistoryFn,
    pub(crate) restore_history: ErasedRestoreFrameHistoryFn,
}

pub(crate) fn matches_filter<F>(components: &Components, archetype: &Archetype) -> bool
where
    F: ArchetypeFilter + 'static,
{
    F::get_state(components)
        .is_some_and(|state| F::matches_component_set(&state, &|id| archetype.contains(id)))
}
