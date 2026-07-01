use crate::SyncComponent;
use crate::archetypes::InterpolatedArchetypes;
use crate::interpolate::{
    interpolate_bundle, present_history_bracket, update_history_archetype_erased,
    update_history_diff_archetype_erased,
};
use crate::plugin::{
    InterpolationSystems, add_interpolation_systems, add_prepare_interpolation_diff_systems,
    add_prepare_interpolation_systems,
};
use alloc::boxed::Box;
use alloc::vec::Vec;
use bevy_app::{App, Update};
use bevy_ecs::archetype::Archetype;
use bevy_ecs::component::{ComponentId, StorageType};
use bevy_ecs::entity::EntityHashMap;
use bevy_ecs::prelude::*;
use bevy_ecs::query::{ArchetypeFilter, QueryData, QueryFilter, QueryState};
use bevy_ecs::schedule::IntoScheduleConfigs;
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy_math::{
    Curve,
    curve::{Ease, EaseFunction, EasingCurve},
};
use bevy_platform::collections::HashMap;
use bevy_platform::collections::HashSet;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::client::confirm_history::ConfirmHistory;
use bevy_replicon::postcard_utils;
use bevy_replicon::prelude::{AppMarkerExt, RuleFns};
use bevy_replicon::shared::replication::deferred_entity::{DeferredEntity, EntityScratch};
use bevy_replicon::shared::replication::diff::{
    ComponentDelta, DiffBuffer, Diffable as RepliconDiffable,
};
use bevy_replicon::shared::replication::receive_markers::MarkerConfig;
use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, WriteCtx};
use bevy_replicon::shared::replication::storage::{EntityStorageCtx, ReplicationStorage};
use bevy_utils::prelude::DebugName;
use core::marker::PhantomData;
use lightyear_core::history_buffer::HistoryState;
use lightyear_core::prelude::{ConfirmedHistory, Interpolated, Tick};
use lightyear_replication::checkpoint::{ReplicationCheckpointMap, resolve_message_tick};
use lightyear_replication::diff_history::HistoryDiffReceiver;
use lightyear_replication::registry::replication::{ComponentRegistration, ComponentRegistrator};
use lightyear_replication::registry::{ComponentKind, ComponentRegistry, LerpFn};
use tracing::{error, trace};

fn lerp<C: Ease + Clone>(start: C, other: C, t: f32) -> C {
    let curve = EasingCurve::new(start, other, EaseFunction::Linear);
    curve.sample_unchecked(t)
}

/// Filter accepted by interpolation rules.
///
/// This intentionally accepts `()` plus Bevy archetype filters such as
/// `With<M>`, `Without<M>`, tuples, and `Or`. It does not accept per-entity
/// filters like `Changed<T>`, because interpolation policy selection is cached
/// per archetype.
///
/// Marker components are represented through filters, not through a separate
/// interpolation marker registry. For example, `With<VisualInterpolation>`
/// selects entities that have the `VisualInterpolation` component.
///
/// # Examples
///
/// Register a rule that only applies to entities with a local marker:
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
/// app.interpolate_with_priority_filtered::<Position, With<VisualInterpolation>>(
///     100,
///     InterpolationFns::interpolate(lerp_position),
/// );
/// ```
pub trait InterpolationRuleFilter: QueryFilter {}

impl<F: ArchetypeFilter> InterpolationRuleFilter for F {}

/// Configuration for an interpolation rule.
///
/// Rules are evaluated per interpolated archetype and component type. If
/// multiple rules match the same archetype, the rule with the highest priority
/// is selected. If the selected rule leaves a phase disabled, Lightyear skips
/// that phase instead of falling back to a lower-priority rule.
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
    /// Higher values are selected first. The default priority is `0` for
    /// component and diff rules. Default bundle registrations use the number
    /// of components in the bundle, so a default `(Position, Rotation)` rule
    /// wins over default single-component rules on the same archetype.
    /// Matching rules with the same priority use registration order, with
    /// earlier registrations selected first.
    pub priority: usize,
}

/// Functions used by an interpolation rule.
///
/// The constructors describe which phases Lightyear owns:
///
/// - [`Self::interpolate`] stores received values in [`ConfirmedHistory`],
///   prepares that history, samples it, and applies the result to the live
///   component. For bundle interpolation, each component is stored in its own
///   history before the tuple interpolation function is called.
/// - [`Self::history_only`] stores and prepares history but does not apply the
///   live component. This is the usual choice when a user system performs
///   custom interpolation.
/// - [`Self::disabled`] intentionally opts matching entities out of
///   Lightyear interpolation for this component.
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
    interpolation: Option<LerpFn<C>>,
    history: bool,
    apply: bool,
    _marker: PhantomData<fn(C)>,
}

impl<C> InterpolationFns<C> {
    /// Enables the full Lightyear interpolation pipeline for `C`.
    ///
    /// Incoming updates are stored in [`ConfirmedHistory<C>`], history is
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
            history: true,
            apply: true,
            _marker: PhantomData,
        }
    }

    /// Stores and prepares interpolation history, but does not apply `C`.
    ///
    /// Use this when Lightyear should receive component updates into
    /// [`ConfirmedHistory<C>`], but visible interpolation is handled by a user
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
            history: true,
            apply: false,
            _marker: PhantomData,
        }
    }

    /// Stores and prepares history and keeps an interpolation function, but
    /// does not apply `C`.
    ///
    /// This is useful for helper APIs that need a pure interpolation function
    /// while the final write still happens in a custom user system.
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
    /// app.interpolate_with::<Position>(
    ///     InterpolationFns::history_only_with_interpolator(lerp_position),
    /// );
    /// ```
    pub fn history_only_with_interpolator(interpolation: LerpFn<C>) -> Self {
        Self {
            interpolation: Some(interpolation),
            history: true,
            apply: false,
            _marker: PhantomData,
        }
    }

    /// Disables Lightyear interpolation phases for matching entities.
    ///
    /// A disabled high-priority rule can be used to exclude a filtered set of
    /// entities from a broader default interpolation rule.
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
    /// app.interpolate_with_priority_filtered::<Position, With<SnapOnly>>(
    ///     100,
    ///     InterpolationFns::disabled(),
    /// );
    /// ```
    pub fn disabled() -> Self {
        Self {
            interpolation: None,
            history: false,
            apply: false,
            _marker: PhantomData,
        }
    }
}

/// Tuple of components that can be interpolated together as one rule.
///
/// Bundle interpolation stores each component in its own
/// [`ConfirmedHistory`], then samples every history at the same interpolation
/// tick. The tuple interpolation function only runs when all member histories
/// have the same bracketing start and end ticks.
///
/// Lightyear currently implements this trait for tuples of 2 to 8 distinct
/// [`SyncComponent`] types.
///
/// # Examples
///
/// Register interpolation for `Position` and `Rotation` together:
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
pub trait InterpolationBundle: 'static {
    /// Number of components in the bundle.
    ///
    /// This is used as the default priority for bundle rules, so a default
    /// bundle rule takes priority over matching rules for smaller overlapping
    /// bundles or individual components.
    #[doc(hidden)]
    const COMPONENT_COUNT: usize;

    /// Query used by the bundle interpolation apply system.
    #[doc(hidden)]
    type Query: QueryData;

    /// Component kinds written by the bundle interpolation apply system.
    #[doc(hidden)]
    fn component_kinds() -> Vec<ComponentKind>;

    /// Applies interpolation to one query item.
    #[doc(hidden)]
    fn apply_item(
        item: <Self::Query as QueryData>::Item<'_, '_>,
        interpolation_registry: &InterpolationRegistry,
        interpolated_archetypes: &InterpolatedArchetypes,
        interpolation_tick: Tick,
        interpolation_overstep: f32,
    );

    /// Adds the typed interpolation apply system for this bundle.
    #[doc(hidden)]
    fn add_apply_system(app: &mut App);

    /// Adds per-component history rules for every component in the bundle.
    #[doc(hidden)]
    fn add_history_rules<F>(app: &mut App, config: InterpolationRuleConfig)
    where
        F: InterpolationRuleFilter + 'static;

    /// Marks every member component as interpolated in Lightyear's component registry.
    #[doc(hidden)]
    fn mark_interpolated(app: &mut App);
}

macro_rules! impl_interpolation_bundle {
    (
        $N:tt,
        (
            $C0:ident,
            $component0:ident,
            $history0:ident,
            $start_tick0:ident,
            $start0:ident,
            $end0:ident,
            $end_tick0:ident,
            $end_value0:ident,
            $output0:ident
        ),
        $(
            (
                $C:ident,
                $component:ident,
                $history:ident,
                $start_tick:ident,
                $start:ident,
                $end:ident,
                $end_tick:ident,
                $end_value:ident,
                $output:ident
            )
        ),+
        $(,)?
    ) => {
        impl<$C0, $($C),+> InterpolationBundle for ($C0, $($C,)+)
        where
            $C0: SyncComponent,
            $($C: SyncComponent),+
        {
            type Query = (
                &'static Archetype,
                (Option<&'static mut $C0>, &'static ConfirmedHistory<$C0>),
                $((Option<&'static mut $C>, &'static ConfirmedHistory<$C>)),+
            );

            const COMPONENT_COUNT: usize = $N;

            fn component_kinds() -> Vec<ComponentKind> {
                alloc::vec![ComponentKind::of::<$C0>(), $(ComponentKind::of::<$C>()),+]
            }

            fn apply_item(
                item: <Self::Query as QueryData>::Item<'_, '_>,
                interpolation_registry: &InterpolationRegistry,
                interpolated_archetypes: &InterpolatedArchetypes,
                interpolation_tick: Tick,
                interpolation_overstep: f32,
            ) {
                let (archetype, ($component0, $history0), $(($component, $history)),+) = item;
                let kind = ComponentKind::of::<($C0, $($C,)+)>();
                let Some(rule_id) =
                    interpolated_archetypes.apply_rule_for(archetype.id(), kind)
                else {
                    return;
                };
                let Some(rule) = interpolation_registry.rule(rule_id) else {
                    return;
                };
                if !rule.applies_component() {
                    return;
                }

                let Some(($start_tick0, $start0, $end0)) =
                    present_history_bracket($history0, interpolation_tick)
                else {
                    return;
                };
                $(
                    let Some(($start_tick, $start, $end)) =
                        present_history_bracket($history, interpolation_tick)
                    else {
                        return;
                    };
                )+
                if false $(|| $start_tick0 != $start_tick)+ {
                    return;
                }

                let interpolated = match ($end0, $($end,)+) {
                    (
                        Some(($end_tick0, $end_value0)),
                        $(Some(($end_tick, $end_value)),)+
                    ) if true $(&& $end_tick0 == $end_tick)+ => {
                        let fraction = (((interpolation_tick - $start_tick0) as f32
                            + interpolation_overstep)
                            / ($end_tick0 - $start_tick0) as f32)
                            .clamp(0.0, 1.0);
                        if let Some(interpolation) =
                            interpolation_registry
                                .interpolation_for_rule::<($C0, $($C,)+)>(rule_id)
                        {
                            interpolation(
                                ($start0, $($start,)+),
                                ($end_value0, $($end_value,)+),
                                fraction,
                            )
                        } else {
                            ($start0, $($start,)+)
                        }
                    }
                    ($end0, $($end,)+) if $end0.is_none() $(&& $end.is_none())+ => {
                        ($start0, $($start,)+)
                    }
                    _ => return,
                };

                let Some(mut $component0) = $component0 else {
                    return;
                };
                $(
                    let Some(mut $component) = $component else {
                        return;
                    };
                )+
                let ($output0, $($output,)+) = interpolated;
                *$component0 = $output0;
                $(
                    *$component = $output;
                )+
            }

            fn add_apply_system(app: &mut App) {
                app.add_systems(
                    Update,
                    interpolate_bundle::<($C0, $($C,)+)>.in_set(InterpolationSystems::Interpolate),
                );
            }

            fn add_history_rules<F>(app: &mut App, config: InterpolationRuleConfig)
            where
                F: InterpolationRuleFilter + 'static,
            {
                add_interpolation_rule::<$C0, F>(
                    app,
                    InterpolationFns::history_only(),
                    config,
                );
                $(
                    add_interpolation_rule::<$C, F>(
                        app,
                        InterpolationFns::history_only(),
                        config,
                    );
                )+
            }

            fn mark_interpolated(app: &mut App) {
                mark_interpolated::<$C0>(app);
                $(mark_interpolated::<$C>(app);)+
            }
        }
    };
}

variadics_please::all_tuples_with_size!(
    impl_interpolation_bundle,
    2,
    8,
    C,
    component,
    history,
    start_tick,
    start,
    end,
    end_tick,
    end_value,
    output
);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub(crate) struct InterpolationRuleId(usize);

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

/// Returns the component ID for a typed component if that component is registered.
pub(crate) type ComponentIdFn = fn(&World) -> Option<ComponentId>;

/// Returns whether a cached interpolation rule matches an archetype.
pub(crate) type MatchesArchetypeFn = fn(&World, &Archetype) -> bool;

/// Type-erased function that updates histories for one component on one archetype.
///
/// The [`UnsafeWorldCell`] is used for direct table access to
/// [`ConfirmedHistory`] and, for diff-replicated components, access to
/// [`ReplicationStorage`]. Structural changes to the live component set are
/// recorded into [`DeferredHistoryCommands`] and flushed after the archetype
/// scan finishes.
pub(crate) type ErasedUpdateHistoryFn = fn(
    UnsafeWorldCell,
    &Archetype,
    &CachedInterpolationComponent,
    UpdateHistoryContext,
    &mut DeferredHistoryCommands,
);

trait DeferredHistoryMutation: Send + Sync + 'static {
    fn apply(self: Box<Self>, entity: &mut DeferredEntity<'_>);
}

struct InsertHistoryComponent<C>(C);

impl<C: Component> DeferredHistoryMutation for InsertHistoryComponent<C> {
    fn apply(self: Box<Self>, entity: &mut DeferredEntity<'_>) {
        entity.insert(self.0);
    }
}

struct RemoveHistoryComponent<C>(PhantomData<fn(C)>);

impl<C: Component> DeferredHistoryMutation for RemoveHistoryComponent<C> {
    fn apply(self: Box<Self>, entity: &mut DeferredEntity<'_>) {
        entity.remove::<C>();
    }
}

/// Batched structural changes produced while preparing interpolation history.
///
/// History updates run over cached archetype table storage. Moving entities to
/// new archetypes during that scan would invalidate the storage access, so live
/// component insertions/removals are collected here and applied afterwards.
/// Mutations are grouped by entity and flushed through Replicon's
/// [`DeferredEntity`], so several component changes for the same entity become
/// one removal bundle and one insertion bundle.
#[derive(Default)]
pub(crate) struct DeferredHistoryCommands {
    entities: EntityHashMap<Vec<Box<dyn DeferredHistoryMutation>>>,
}

impl DeferredHistoryCommands {
    pub(crate) fn insert<C: Component>(&mut self, entity: Entity, component: C) {
        self.entities
            .entry(entity)
            .or_default()
            .push(Box::new(InsertHistoryComponent(component)));
    }

    pub(crate) fn remove<C: Component>(&mut self, entity: Entity) {
        self.entities
            .entry(entity)
            .or_default()
            .push(Box::new(RemoveHistoryComponent::<C>(PhantomData)));
    }

    pub(crate) fn apply(self, world: &mut World) {
        let mut scratch = EntityScratch::default();
        for (entity, mutations) in self.entities {
            let Ok(entity_mut) = world.get_entity_mut(entity) else {
                continue;
            };
            let mut deferred = DeferredEntity::new(entity_mut, &mut scratch);
            for mutation in mutations {
                mutation.apply(&mut deferred);
            }
            deferred.flush();
        }
    }
}

/// Cached typed component metadata needed by the type-erased history updater.
///
/// One value is stored per selected history-owning rule on each cached
/// interpolated archetype. It lets the update system jump directly to the
/// `ConfirmedHistory<C>` column and decide whether the corresponding live
/// component is currently present on that archetype.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CachedInterpolationComponent {
    /// Component ID for `ConfirmedHistory<C>`.
    history_component_id: ComponentId,
    /// Storage backing `ConfirmedHistory<C>` on the cached archetype.
    history_storage: StorageType,
    /// Whether the live component `C` is present on the cached archetype.
    live_component_present: bool,
    /// Type-erased history update function for `C`.
    update_history: ErasedUpdateHistoryFn,
    /// Optional interpolation function used when sampling the history.
    interpolation: Option<ErasedLerpFn>,
}

impl CachedInterpolationComponent {
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

#[derive(Debug, Clone, Copy)]
pub(crate) struct ErasedInterpolationFns {
    interpolation: Option<ErasedLerpFn>,
    history: bool,
    apply: bool,
    update_history: Option<ErasedUpdateHistoryFn>,
    history_component_id: Option<ComponentIdFn>,
    live_component_id: ComponentIdFn,
}

impl ErasedInterpolationFns {
    fn from_typed<S: 'static>(
        fns: InterpolationFns<S>,
        update_history: Option<ErasedUpdateHistoryFn>,
        history_component_id: Option<ComponentIdFn>,
        live_component_id: ComponentIdFn,
    ) -> Self {
        Self {
            interpolation: fns
                .interpolation
                .map(|f| unsafe { core::mem::transmute::<LerpFn<S>, unsafe fn()>(f) }),
            history: fns.history,
            apply: fns.apply,
            update_history,
            history_component_id,
            live_component_id,
        }
    }
}

fn confirmed_history_component_id<C: Component + Clone>(world: &World) -> Option<ComponentId> {
    world.component_id::<ConfirmedHistory<C>>()
}

fn live_component_id<C: Component>(world: &World) -> Option<ComponentId> {
    world.component_id::<C>()
}

fn no_component_id(_: &World) -> Option<ComponentId> {
    None
}

/// One interpolation rule registered by [`AppInterpolationExt`].
///
/// A rule has a `kind` used for cache lookup, a list of component `members`
/// it owns or writes, erased functions describing which phases Lightyear runs,
/// and an archetype filter. Rules are sorted by priority per `kind`, so
/// [`InterpolationRegistry::select_rule_for_archetype`] can return the first
/// matching rule.
#[derive(Debug, Clone)]
pub(crate) struct InterpolationRule {
    /// Rule key used when selecting a rule for a component or bundle.
    kind: ComponentKind,
    /// Components owned by this rule. Bundle rules have more than one member.
    members: Vec<ComponentKind>,
    /// Higher-priority rules are selected before lower-priority rules.
    priority: usize,
    /// Type-erased interpolation/history/apply functions for this rule.
    fns: ErasedInterpolationFns,
    /// Archetype-level filter predicate compiled from the rule filter type.
    matches_archetype: MatchesArchetypeFn,
}

impl InterpolationRule {
    pub(crate) fn owns_history(&self) -> bool {
        self.fns.history
    }

    pub(crate) fn applies_component(&self) -> bool {
        self.fns.apply
    }

    pub(crate) fn members(&self) -> &[ComponentKind] {
        &self.members
    }

    pub(crate) fn priority(&self) -> usize {
        self.priority
    }
}

/// Compatibility metadata stored for a component registered for interpolation.
///
/// This type backs existing registry APIs and frame interpolation integration.
/// New rule registrations should prefer [`InterpolationFns`] and
/// [`AppInterpolationExt`].
///
/// # Examples
///
/// Construct metadata for a component that only stores history:
///
/// ```rust,ignore
/// use lightyear_interpolation::prelude::*;
///
/// let metadata = InterpolationMetadata {
///     interpolation: None,
///     custom_interpolation: true,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct InterpolationMetadata {
    /// Erased interpolation function registered for this component.
    ///
    /// This is retained for compatibility with the existing registry APIs and
    /// frame interpolation integration. New code should prefer
    /// [`InterpolationFns`] and [`AppInterpolationExt`].
    pub interpolation: Option<ErasedInterpolationFn>,
    /// Whether Lightyear only maintains history and user code performs the
    /// actual interpolation.
    pub custom_interpolation: bool,
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
/// let registry = app.world().resource::<InterpolationRegistry>();
/// assert!(registry.interpolated::<Position>());
/// ```
#[derive(Resource, Debug, Default)]
pub struct InterpolationRegistry {
    pub(crate) interpolation_map: HashMap<ComponentKind, InterpolationMetadata>,
    rules: Vec<InterpolationRule>,
    rules_by_component: HashMap<ComponentKind, Vec<InterpolationRuleId>>,
}

#[derive(Resource, Debug, Default)]
struct InterpolatedMarkerFnRegistry {
    kinds: HashSet<ComponentKind>,
}

#[derive(Resource, Debug, Default)]
struct RegisteredInterpolationSystems {
    prepare: HashSet<ComponentKind>,
    prepare_diff: HashSet<ComponentKind>,
    apply: HashSet<ComponentKind>,
}

impl InterpolationRegistry {
    pub fn set_linear_interpolation<C: Component + Clone + Ease>(&mut self) {
        self.set_interpolation(lerp::<C>);
    }

    pub fn set_interpolation<C: Component + Clone>(&mut self, interpolation_fn: LerpFn<C>) {
        let kind = ComponentKind::of::<C>();
        self.interpolation_map
            .entry(kind)
            .or_insert_with(|| InterpolationMetadata {
                interpolation: None,
                custom_interpolation: false,
            })
            .interpolation = Some(unsafe { core::mem::transmute(interpolation_fn) });
    }

    /// Iterates over component or bundle kinds that have interpolation rules.
    pub(crate) fn rule_component_kinds(&self) -> impl Iterator<Item = ComponentKind> + '_ {
        self.rules_by_component.keys().copied()
    }

    /// Returns a rule by ID.
    pub(crate) fn rule(&self, rule_id: InterpolationRuleId) -> Option<&InterpolationRule> {
        self.rules.get(rule_id.0)
    }

    /// Selects the highest-priority matching rule for `kind` on `archetype`.
    ///
    /// Rules are pre-sorted by descending priority and ascending registration
    /// order, so this returns the first matching rule. It only answers
    /// "which rule owns this kind on this archetype"; overlap suppression for
    /// live component writes is handled later by
    /// `CachedInterpolatedArchetype::resolve_apply_rules`.
    pub(crate) fn select_rule_for_archetype(
        &self,
        world: &World,
        archetype: &Archetype,
        kind: ComponentKind,
    ) -> Option<InterpolationRuleId> {
        self.rules_by_component
            .get(&kind)?
            .iter()
            .copied()
            .find(|rule_id| {
                self.rules
                    .get(rule_id.0)
                    .is_some_and(|rule| (rule.matches_archetype)(world, archetype))
            })
    }

    /// Builds cached history metadata for `rule_id` on `archetype`.
    ///
    /// Returns `None` when the rule does not own history, the history component
    /// is not registered, or this archetype does not currently contain the
    /// history component.
    pub(crate) fn cached_history_component(
        &self,
        world: &World,
        archetype: &Archetype,
        rule_id: InterpolationRuleId,
    ) -> Option<CachedInterpolationComponent> {
        let rule = self.rules.get(rule_id.0)?;
        if !rule.owns_history() {
            return None;
        }
        let history_component_id = (rule.fns.history_component_id?)(world)?;
        if !archetype.contains(history_component_id) {
            return None;
        }
        let history_storage = archetype.get_storage_type(history_component_id)?;
        let live_component_present = (rule.fns.live_component_id)(world)
            .is_some_and(|component_id| archetype.contains(component_id));
        Some(CachedInterpolationComponent {
            history_component_id,
            history_storage,
            live_component_present,
            update_history: rule.fns.update_history?,
            interpolation: rule.fns.interpolation,
        })
    }

    pub(crate) fn insert_rule<C, F>(
        &mut self,
        fns: InterpolationFns<C>,
        config: InterpolationRuleConfig,
    ) -> InterpolationRuleId
    where
        C: SyncComponent,
        F: InterpolationRuleFilter + 'static,
    {
        self.insert_rule_with_update_history::<C, F>(
            fns,
            config,
            Some(update_history_archetype_erased::<C>),
        )
    }

    pub(crate) fn insert_diff_rule<C, F>(
        &mut self,
        fns: InterpolationFns<C>,
        config: InterpolationRuleConfig,
    ) -> InterpolationRuleId
    where
        C: SyncComponent + RepliconDiffable,
        F: InterpolationRuleFilter + 'static,
    {
        self.insert_rule_with_update_history::<C, F>(
            fns,
            config,
            Some(update_history_diff_archetype_erased::<C>),
        )
    }

    fn insert_rule_with_update_history<C, F>(
        &mut self,
        fns: InterpolationFns<C>,
        config: InterpolationRuleConfig,
        update_history: Option<ErasedUpdateHistoryFn>,
    ) -> InterpolationRuleId
    where
        C: SyncComponent,
        F: InterpolationRuleFilter + 'static,
    {
        let kind = ComponentKind::of::<C>();
        let history_component_id = fns
            .history
            .then_some(confirmed_history_component_id::<C> as ComponentIdFn);
        let fns = ErasedInterpolationFns::from_typed(
            fns,
            update_history,
            history_component_id,
            live_component_id::<C>,
        );
        let rule_id = InterpolationRuleId(self.rules.len());
        self.rules.push(InterpolationRule {
            kind,
            members: alloc::vec![kind],
            priority: config.priority,
            fns,
            matches_archetype: matches_filter::<F>,
        });
        let rules = self.rules_by_component.entry(kind).or_default();
        rules.push(rule_id);
        rules.sort_by(|a, b| {
            self.rules[b.0]
                .priority
                .cmp(&self.rules[a.0].priority)
                .then_with(|| a.0.cmp(&b.0))
        });

        let metadata =
            self.interpolation_map
                .entry(kind)
                .or_insert_with(|| InterpolationMetadata {
                    interpolation: None,
                    custom_interpolation: false,
                });
        if let Some(interpolation) = fns.interpolation {
            metadata.interpolation = Some(interpolation);
        }
        if fns.history || fns.apply {
            metadata.custom_interpolation = !fns.apply;
        }
        rule_id
    }

    pub(crate) fn insert_bundle_rule<S, F>(
        &mut self,
        fns: InterpolationFns<S>,
        config: InterpolationRuleConfig,
        members: Vec<ComponentKind>,
    ) -> InterpolationRuleId
    where
        S: 'static,
        F: InterpolationRuleFilter + 'static,
    {
        let kind = ComponentKind::of::<S>();
        let fns = ErasedInterpolationFns::from_typed(fns, None, None, no_component_id);
        let rule_id = InterpolationRuleId(self.rules.len());
        self.rules.push(InterpolationRule {
            kind,
            members,
            priority: config.priority,
            fns,
            matches_archetype: matches_filter::<F>,
        });
        let rules = self.rules_by_component.entry(kind).or_default();
        rules.push(rule_id);
        rules.sort_by(|a, b| {
            self.rules[b.0]
                .priority
                .cmp(&self.rules[a.0].priority)
                .then_with(|| a.0.cmp(&b.0))
        });
        rule_id
    }

    /// Returns True if the component `C` is interpolated
    pub fn interpolated<C: Component>(&self) -> bool {
        let kind = ComponentKind::of::<C>();
        self.interpolation_map.get(&kind).is_some()
    }

    pub(crate) fn has_interpolation_fn<C: Component>(&self) -> bool {
        let kind = ComponentKind::of::<C>();
        self.interpolation_map
            .get(&kind)
            .is_some_and(|metadata| metadata.interpolation.is_some())
    }

    pub fn interpolate<C: Component>(&self, start: C, end: C, t: f32) -> C {
        let kind = ComponentKind::of::<C>();
        let interpolation_metadata = self
            .interpolation_map
            .get(&kind)
            .expect("the component is not part of the protocol");
        let interpolation_fn: LerpFn<C> =
            unsafe { core::mem::transmute(interpolation_metadata.interpolation.unwrap()) };
        interpolation_fn(start, end, t)
    }

    pub(crate) fn sample_for_rule<C: Component + Clone>(
        &self,
        rule_id: InterpolationRuleId,
        history: &ConfirmedHistory<C>,
        interpolation_tick: Tick,
        interpolation_overstep: f32,
    ) -> Option<HistoryState<C>> {
        let rule = &self.rules[rule_id.0];
        debug_assert_eq!(rule.kind, ComponentKind::of::<C>());
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
        debug_assert_eq!(rule.kind, ComponentKind::of::<S>());
        rule.fns
            .interpolation
            .map(|interpolation| unsafe { core::mem::transmute(interpolation) })
    }

    /// Sample `history` at `interpolation_tick`.
    ///
    /// Returns `None` when no authoritative state exists at or before the
    /// interpolation tick. Otherwise returns the resolved authoritative state:
    /// either a removal, the latest present value, or an interpolated value
    /// between the bracketing present samples.
    ///
    /// If there is no next present sample, sampling returns the resolved start
    /// value instead of extrapolating.
    pub(crate) fn sample<C: Component + Clone>(
        &self,
        history: &ConfirmedHistory<C>,
        interpolation_tick: Tick,
        interpolation_overstep: f32,
    ) -> Option<HistoryState<C>> {
        let kind = ComponentKind::of::<C>();
        let interpolation = self
            .interpolation_map
            .get(&kind)
            .and_then(|metadata| metadata.interpolation);
        sample_history_with_interpolation(
            interpolation,
            history,
            interpolation_tick,
            interpolation_overstep,
        )
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

fn matches_filter<F>(world: &World, archetype: &Archetype) -> bool
where
    F: InterpolationRuleFilter + 'static,
{
    QueryState::<&Archetype, F>::try_new(world)
        .is_some_and(|query| query.matches_component_set(&|id| archetype.contains(id)))
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
    /// Registers a default-priority interpolation rule for component `C`.
    ///
    /// If the selected [`InterpolationFns`] owns history, Lightyear receives
    /// authoritative updates into [`ConfirmedHistory<C>`]. If it owns apply,
    /// Lightyear samples that history and writes the live component during
    /// [`InterpolationSystems::Interpolate`].
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
    /// Use [`Self::interpolate_with`] for the default unfiltered rule.
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
        F: InterpolationRuleFilter + 'static,
    {
        self.interpolate_with_priority_filtered::<C, F>(
            InterpolationRuleConfig::default().priority,
            fns,
        )
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
        F: InterpolationRuleFilter + 'static;

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
        F: InterpolationRuleFilter + 'static,
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
        F: InterpolationRuleFilter + 'static;

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
    fn interpolate_diff_filtered_with<C, F>(&mut self, fns: InterpolationFns<C>) -> &mut Self
    where
        C: SyncComponent + RepliconDiffable,
        F: InterpolationRuleFilter + 'static,
    {
        self.interpolate_diff_with_priority_filtered::<C, F>(
            InterpolationRuleConfig::default().priority,
            fns,
        )
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
        F: InterpolationRuleFilter + 'static;
}

impl AppInterpolationExt for App {
    fn interpolate_with_priority_filtered<C, F>(
        &mut self,
        priority: usize,
        fns: InterpolationFns<C>,
    ) -> &mut Self
    where
        C: SyncComponent,
        F: InterpolationRuleFilter + 'static,
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
        F: InterpolationRuleFilter + 'static,
    {
        add_interpolation_bundle_rule::<B, F>(self, fns, InterpolationRuleConfig { priority });
        self
    }

    fn interpolate_diff_with_priority_filtered<C, F>(
        &mut self,
        priority: usize,
        fns: InterpolationFns<C>,
    ) -> &mut Self
    where
        C: SyncComponent + RepliconDiffable,
        F: InterpolationRuleFilter + 'static,
    {
        add_interpolation_diff_rule::<C, F>(self, fns, InterpolationRuleConfig { priority });
        self
    }
}

fn register_interpolated_marker_fns<C: SyncComponent>(app: &mut bevy_app::App) {
    if !app
        .world()
        .contains_resource::<InterpolatedMarkerFnRegistry>()
    {
        app.world_mut()
            .insert_resource(InterpolatedMarkerFnRegistry::default());
    }
    let kind = ComponentKind::of::<C>();
    let already_registered = {
        let registry = app.world().resource::<InterpolatedMarkerFnRegistry>();
        registry.kinds.contains(&kind)
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
        .resource_mut::<InterpolatedMarkerFnRegistry>()
        .kinds
        .insert(kind);
}

fn register_interpolated_diff_marker_fns<C: SyncComponent + RepliconDiffable>(
    app: &mut bevy_app::App,
) {
    if !app
        .world()
        .contains_resource::<InterpolatedMarkerFnRegistry>()
    {
        app.world_mut()
            .insert_resource(InterpolatedMarkerFnRegistry::default());
    }
    let kind = ComponentKind::of::<C>();
    app.register_marker_with::<Interpolated>(MarkerConfig {
        priority: 100,
        need_history: true,
    });
    app.set_marker_fns::<Interpolated, C>(write_history_diff::<C>, remove_history::<C>);
    app.world_mut()
        .resource_mut::<InterpolatedMarkerFnRegistry>()
        .kinds
        .insert(kind);
}

/// When `Interpolated` is added after component `C` was already replicated onto the entity,
/// seed `ConfirmedHistory<C>` from the current value so interpolation has an anchor immediately.
///
/// Component updates for interpolated entities are normally captured by `write_history::<C>`, but
/// that only runs on future network updates. If `Interpolated` arrives after `C`, synthesize the
/// initial history entry from the existing component value and the entity's latest confirmed
/// Replicon tick.
pub(crate) fn insert_confirmed_history_on_interpolated<C: SyncComponent>(
    trigger: On<Add, Interpolated>,
    mut commands: Commands,
    checkpoints: Res<ReplicationCheckpointMap>,
    query: Query<(&C, &ConfirmHistory), Without<ConfirmedHistory<C>>>,
) {
    let Ok((component, confirm_history)) = query.get(trigger.entity) else {
        return;
    };

    let Some(tick) = checkpoints.get(confirm_history.last_tick()) else {
        debug_assert!(
            false,
            "missing authoritative checkpoint mapping while backfilling ConfirmedHistory"
        );
        return;
    };

    let mut history = ConfirmedHistory::<C>::default();
    history.insert_present(tick, component.clone());
    commands
        .entity(trigger.entity)
        .try_insert(history)
        .try_remove::<C>();
}

pub(crate) fn insert_confirmed_history_on_interpolated_diff<C: SyncComponent + RepliconDiffable>(
    trigger: On<Add, Interpolated>,
    mut commands: Commands,
    checkpoints: Res<ReplicationCheckpointMap>,
    query: Query<(&C, &ConfirmHistory, Option<&ConfirmedHistory<C>>)>,
) {
    let Ok((component, confirm_history, history)) = query.get(trigger.entity) else {
        return;
    };

    let Some(tick) = checkpoints.get(confirm_history.last_tick()) else {
        debug_assert!(
            false,
            "missing authoritative checkpoint mapping while backfilling diff ConfirmedHistory"
        );
        return;
    };

    let entity = trigger.entity;
    let component = component.clone();
    let insert_history = history.is_none();
    commands.queue(move |world: &mut World| {
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
    /// Register an interpolation function for this component using the provided [`LerpFn`]
    ///
    /// This does NOT mean that interpolation systems are added, it simply registers a function to
    /// interpolate between two values, that can be used for example in frame interpolation.
    fn register_interpolation_fn(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent;

    /// Register an interpolation function for this component using the [`Ease`] implementation
    ///
    /// This does NOT mean that interpolation systems are added, it simply registers a function to
    /// interpolate between two values, that can be used for example in frame interpolation.
    fn register_linear_interpolation(self) -> Self
    where
        C: SyncComponent + Ease;

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
    fn register_interpolation_fn(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent,
    {
        Self::from_component_registration(register_interpolation_fn_impl(
            self.into_component_registration(),
            interpolation_fn,
        ))
    }

    fn register_linear_interpolation(self) -> Self
    where
        C: SyncComponent + Ease,
    {
        self.register_interpolation_fn(lerp::<C>)
    }

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

fn ensure_interpolated_archetypes(app: &mut App) {
    app.init_resource::<InterpolatedArchetypes>();
}

fn invalidate_interpolated_archetypes(app: &mut App) {
    if let Some(mut archetypes) = app.world_mut().get_resource_mut::<InterpolatedArchetypes>() {
        archetypes.clear();
    }
}

fn ensure_interpolation_system_registry(app: &mut App) {
    if !app
        .world()
        .contains_resource::<RegisteredInterpolationSystems>()
    {
        app.world_mut()
            .insert_resource(RegisteredInterpolationSystems::default());
    }
}

fn add_prepare_interpolation_systems_once<C: SyncComponent>(app: &mut App) {
    ensure_interpolation_system_registry(app);
    let kind = ComponentKind::of::<C>();
    let should_add = app
        .world_mut()
        .resource_mut::<RegisteredInterpolationSystems>()
        .prepare
        .insert(kind);
    if should_add {
        add_prepare_interpolation_systems::<C>(app);
    }
}

fn add_prepare_interpolation_diff_systems_once<C: SyncComponent + RepliconDiffable>(app: &mut App) {
    ensure_interpolation_system_registry(app);
    let kind = ComponentKind::of::<C>();
    let should_add = app
        .world_mut()
        .resource_mut::<RegisteredInterpolationSystems>()
        .prepare_diff
        .insert(kind);
    if should_add {
        add_prepare_interpolation_diff_systems::<C>(app);
    }
}

fn add_interpolation_systems_once<C: SyncComponent>(app: &mut App) {
    ensure_interpolation_system_registry(app);
    let kind = ComponentKind::of::<C>();
    let should_add = app
        .world_mut()
        .resource_mut::<RegisteredInterpolationSystems>()
        .apply
        .insert(kind);
    if should_add {
        add_interpolation_systems::<C>(app);
    }
}

fn add_interpolation_bundle_systems_once<B: InterpolationBundle>(app: &mut App) {
    ensure_interpolation_system_registry(app);
    let kind = ComponentKind::of::<B>();
    let should_add = app
        .world_mut()
        .resource_mut::<RegisteredInterpolationSystems>()
        .apply
        .insert(kind);
    if should_add {
        B::add_apply_system(app);
    }
}

fn mark_interpolated<C: SyncComponent>(app: &mut App) {
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

fn add_interpolation_rule<C, F>(
    app: &mut App,
    fns: InterpolationFns<C>,
    config: InterpolationRuleConfig,
) where
    C: SyncComponent,
    F: InterpolationRuleFilter + 'static,
{
    QueryState::<&Archetype, F>::new(app.world_mut());
    ensure_interpolation_registry(app);
    ensure_interpolated_archetypes(app);
    if fns.history {
        register_interpolated_marker_fns::<C>(app);
        add_prepare_interpolation_systems_once::<C>(app);
        mark_interpolated::<C>(app);
    }
    if fns.apply {
        add_interpolation_systems_once::<C>(app);
        mark_interpolated::<C>(app);
    }
    app.world_mut()
        .resource_mut::<InterpolationRegistry>()
        .insert_rule::<C, F>(fns, config);
    invalidate_interpolated_archetypes(app);
}

fn add_interpolation_diff_rule<C, F>(
    app: &mut App,
    fns: InterpolationFns<C>,
    config: InterpolationRuleConfig,
) where
    C: SyncComponent + RepliconDiffable,
    F: InterpolationRuleFilter + 'static,
{
    QueryState::<&Archetype, F>::new(app.world_mut());
    ensure_interpolation_registry(app);
    ensure_interpolated_archetypes(app);
    if fns.history {
        register_interpolated_diff_marker_fns::<C>(app);
        add_prepare_interpolation_diff_systems_once::<C>(app);
        mark_interpolated::<C>(app);
    }
    if fns.apply {
        add_interpolation_systems_once::<C>(app);
        mark_interpolated::<C>(app);
    }
    app.world_mut()
        .resource_mut::<InterpolationRegistry>()
        .insert_diff_rule::<C, F>(fns, config);
    invalidate_interpolated_archetypes(app);
}

fn add_interpolation_bundle_rule<B, F>(
    app: &mut App,
    fns: InterpolationFns<B>,
    config: InterpolationRuleConfig,
) where
    B: InterpolationBundle,
    F: InterpolationRuleFilter + 'static,
{
    QueryState::<&Archetype, F>::new(app.world_mut());
    ensure_interpolation_registry(app);
    ensure_interpolated_archetypes(app);
    if fns.history {
        B::add_history_rules::<F>(app, config);
    }
    if fns.apply {
        add_interpolation_bundle_systems_once::<B>(app);
        B::mark_interpolated(app);
    }
    app.world_mut()
        .resource_mut::<InterpolationRegistry>()
        .insert_bundle_rule::<B, F>(fns, config, B::component_kinds());
    invalidate_interpolated_archetypes(app);
}

fn register_interpolation_fn_impl<'a, C>(
    registration: ComponentRegistration<'a, C>,
    interpolation_fn: LerpFn<C>,
) -> ComponentRegistration<'a, C>
where
    C: SyncComponent,
{
    register_interpolated_marker_fns::<C>(registration.app);
    ensure_interpolation_registry(registration.app);
    let mut registry = registration
        .app
        .world_mut()
        .resource_mut::<InterpolationRegistry>();
    registry.set_interpolation::<C>(interpolation_fn);
    registration
}

fn register_interpolation_diff_fn_impl<'a, C>(
    registration: ComponentRegistration<'a, C>,
    interpolation_fn: LerpFn<C>,
) -> ComponentRegistration<'a, C>
where
    C: SyncComponent + RepliconDiffable,
{
    register_interpolated_diff_marker_fns::<C>(registration.app);
    ensure_interpolation_registry(registration.app);
    let mut registry = registration
        .app
        .world_mut()
        .resource_mut::<InterpolationRegistry>();
    registry.set_interpolation::<C>(interpolation_fn);
    registration
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
        InterpolationRuleConfig::default(),
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
        InterpolationRuleConfig::default(),
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
        InterpolationRuleConfig::default(),
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
        InterpolationRuleConfig::default(),
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

    #[derive(Component, Clone, Debug, PartialEq)]
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

    fn registry() -> InterpolationRegistry {
        let mut registry = InterpolationRegistry::default();
        registry.set_interpolation::<TestComp>(lerp);
        registry
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

        let registry = registry();
        assert_eq!(
            registry.sample(&history, Tick(30), 0.0),
            Some(HistoryState::Updated(TestComp(10.0)))
        );
        assert_eq!(
            registry.sample(&history, Tick(20), 0.5),
            Some(HistoryState::Updated(TestComp(10.0)))
        );
    }

    #[test]
    fn sample_returns_start_value_with_single_keyframe() {
        let mut history = ConfirmedHistory::<TestComp>::default();
        history.insert_present(Tick(10), TestComp(42.0));

        let registry = registry();
        assert_eq!(registry.sample(&history, Tick(5), 0.0), None);
        assert_eq!(
            registry.sample(&history, Tick(10), 0.0),
            Some(HistoryState::Updated(TestComp(42.0)))
        );
        assert_eq!(
            registry.sample(&history, Tick(50), 0.5),
            Some(HistoryState::Updated(TestComp(42.0)))
        );
    }

    #[test]
    fn inserts_history_when_interpolated_added_after_component_is_already_replicated() {
        let mut app = App::new();
        app.insert_resource(ReplicationCheckpointMap::default());
        app.add_observer(insert_confirmed_history_on_interpolated::<TestComp>);

        let replicon_tick = RepliconTick::new(11);
        app.world_mut()
            .resource_mut::<ReplicationCheckpointMap>()
            .record(replicon_tick, Tick(42));

        let entity = app
            .world_mut()
            .spawn((TestComp(2.0), ConfirmHistory::new(replicon_tick)))
            .id();
        app.update();
        app.world_mut().entity_mut(entity).insert(Interpolated);
        app.update();

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
