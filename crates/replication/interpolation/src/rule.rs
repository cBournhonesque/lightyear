use crate::SyncComponent;
use crate::interpolate::present_history_bracket;
use crate::registry::{
    InterpolationRegistry, add_interpolation_bundle_rule, add_interpolation_rule, mark_interpolated,
};
use alloc::vec::Vec;
use bevy_app::App;
use bevy_ecs::archetype::Archetype;
use bevy_ecs::component::{ComponentId, Components, StorageType};
use bevy_ecs::prelude::*;
use bevy_ecs::query::{Or, QueryFilter};
use bevy_ecs::storage::Table;
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy_math::{
    Curve,
    curve::{Ease, EaseFunction, EasingCurve},
};
use bevy_utils::prelude::DebugName;
use core::any::TypeId;
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use lightyear_core::prelude::{
    ConfirmedHistory, FrameInterpolate, FrameInterpolationHistory, Interpolated, Tick,
};
use lightyear_replication::deferred_entity::DeferredEntityCommands;
use lightyear_replication::registry::{ComponentKind, LerpFn};
use tracing::trace;

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
/// Filters do not add to the rule priority. Use an explicit priority when a
/// filtered rule should override a broader rule for the same component or
/// bundle.
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
pub trait InterpolationRuleFilter: QueryFilter {
    #[doc(hidden)]
    fn phase_mask() -> RulePhaseMask;
}

impl InterpolationRuleFilter for () {
    fn phase_mask() -> RulePhaseMask {
        RulePhaseMask::SHARED
    }
}

impl<T: Component> InterpolationRuleFilter for With<T> {
    fn phase_mask() -> RulePhaseMask {
        RulePhaseMask::from_filter_component::<T>()
    }
}

impl<T: Component> InterpolationRuleFilter for Without<T> {
    fn phase_mask() -> RulePhaseMask {
        RulePhaseMask::from_filter_component::<T>()
    }
}

impl<F> InterpolationRuleFilter for Or<F>
where
    F: InterpolationRuleFilter,
    Or<F>: QueryFilter,
{
    fn phase_mask() -> RulePhaseMask {
        F::phase_mask()
    }
}

macro_rules! impl_interpolation_rule_filter {
    ($($name:ident),*) => {
        impl<$($name: InterpolationRuleFilter),*> InterpolationRuleFilter for ($($name,)*) {
            fn phase_mask() -> RulePhaseMask {
                RulePhaseMask::SHARED$(.union($name::phase_mask()))*
            }
        }
    };
}

variadics_please::all_tuples!(impl_interpolation_rule_filter, 1, 15, F);

/// Interpolation phase a rule can be selected for.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RulePhase {
    /// Delayed interpolation between authoritative server updates.
    Interpolation,
    /// Visual interpolation between fixed-update ticks.
    Frame,
}

impl RulePhase {
    const fn bit(self) -> u8 {
        match self {
            RulePhase::Interpolation => 1,
            RulePhase::Frame => 2,
        }
    }
}

/// Phase mask inferred from a rule's archetype filter.
///
/// A mask of `0` means the filter did not mention either interpolation marker,
/// so the rule is shared by both caches. Non-zero masks are explicit: a rule
/// whose filter mentions [`Interpolated`] is considered only for delayed
/// interpolation, and a rule whose filter mentions [`FrameInterpolate`] is
/// considered only for frame interpolation.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct RulePhaseMask(u8);

impl RulePhaseMask {
    pub const SHARED: Self = Self(0);
    pub const INTERPOLATION: Self = Self(1);
    pub const FRAME: Self = Self(2);

    fn from_filter_component<T: Component>() -> Self {
        if TypeId::of::<T>() == TypeId::of::<Interpolated>() {
            Self::INTERPOLATION
        } else if TypeId::of::<T>() == TypeId::of::<FrameInterpolate>() {
            Self::FRAME
        } else {
            Self::SHARED
        }
    }

    pub const fn includes(self, phase: RulePhase) -> bool {
        self.0 == 0 || (self.0 & phase.bit()) != 0
    }

    const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

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
/// The constructors describe which phases Lightyear owns:
///
/// - [`Self::interpolate`] stores received values in [`ConfirmedHistory`],
///   prepares that history, samples it, and applies the result to the live
///   component. For bundle interpolation, each component is stored in its own
///   history before the tuple interpolation function is called.
/// - [`Self::history_only`] stores and prepares history but does not apply the
///   live component. This is the usual choice when a user system performs
///   custom interpolation.
/// - [`Self::history_only_with_interpolator`] stores and prepares history and
///   keeps an interpolation function. Delayed interpolation still does not
///   apply the live component, but frame interpolation can reuse the function.
/// - [`Self::no_history`] registers an interpolation function without owning
///   delayed interpolation history, so frame interpolation can reuse the same
///   rule on entities with [`FrameInterpolate`].
/// - [`Self::disabled`] intentionally opts matching entities out of Lightyear
///   interpolation for this component.
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
            pipeline: InterpolationPipeline::Full,
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
            pipeline: InterpolationPipeline::HistoryOnly,
            _marker: PhantomData,
        }
    }

    /// Stores and prepares delayed interpolation history and keeps an interpolation function.
    ///
    /// This does not apply `C` during delayed interpolation, so a custom system
    /// can still sample the history and write components itself. If an entity
    /// has [`FrameInterpolate`], frame interpolation can reuse the stored
    /// interpolation function to smooth fixed-update changes.
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
            pipeline: InterpolationPipeline::HistoryOnly,
            _marker: PhantomData,
        }
    }

    /// Registers an interpolation function without delayed interpolation history.
    ///
    /// This is useful when a component is not delayed-interpolated through
    /// [`Interpolated`], but entities with [`FrameInterpolate`] should still
    /// reuse the same interpolation function for visual smoothing and
    /// correction.
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
            pipeline: InterpolationPipeline::Disabled,
            _marker: PhantomData,
        }
    }

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

impl<C> InterpolationFns<C>
where
    C: Ease + Clone,
{
    /// Stores delayed interpolation history and keeps a linear interpolation function.
    ///
    /// This is the linear [`Ease`] equivalent of
    /// [`Self::history_only_with_interpolator`]. Delayed interpolation stores
    /// history without applying the live component, while frame interpolation
    /// can reuse the same linear function.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// # use bevy_ecs::prelude::*;
    /// # use bevy_math::{Curve, curve::Ease};
    /// # use lightyear_interpolation::prelude::*;
    /// # #[derive(Component, Clone, PartialEq)]
    /// # struct Position(f32);
    /// # impl Ease for Position {
    /// #     fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
    /// #         bevy_math::curve::FunctionCurve::new(bevy_math::Interval::UNIT, move |t| {
    /// #             Position(start.0 + (end.0 - start.0) * t)
    /// #         })
    /// #     }
    /// # }
    /// app.interpolate_with::<Position>(InterpolationFns::linear_history_only());
    /// ```
    pub fn linear_history_only() -> Self {
        Self::history_only_with_interpolator(linear_lerp::<C>)
    }
}

fn linear_lerp<C>(start: C, other: C, t: f32) -> C
where
    C: Ease + Clone,
{
    let curve = EasingCurve::new(start, other, EaseFunction::Linear);
    curve.sample_unchecked(t)
}

/// Returns the component ID for a typed component if that component is registered.
#[doc(hidden)]
pub type ComponentIdFn = fn(&Components) -> Option<ComponentId>;

/// Tuple of components that can be interpolated by one rule.
///
/// Tuple interpolation stores each component in its own history, samples every
/// history at the same interpolation tick, and only runs the tuple
/// interpolation function when all member histories have the same bracketing
/// start and end ticks.
///
/// Lightyear implements this trait for tuples of 2 to 8 distinct
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
pub trait InterpolationBundle: private::Sealed + 'static {
    /// Number of components in the interpolation target.
    ///
    /// This is used as the default priority, so a default tuple rule takes
    /// priority over matching rules for smaller overlapping tuples or
    /// individual components.
    #[doc(hidden)]
    const COMPONENT_COUNT: usize;

    /// Registers an interpolation rule for this component or tuple target.
    #[doc(hidden)]
    fn add_rule<F>(app: &mut App, fns: InterpolationFns<Self>, config: InterpolationRuleConfig)
    where
        Self: Sized,
        F: InterpolationRuleFilter + 'static;
}

mod private {
    pub trait Sealed {}
}

pub(crate) trait TupleInterpolationBundle: InterpolationBundle {
    /// Component kinds written by the tuple interpolation apply system.
    fn component_kinds() -> Vec<ComponentKind>;

    /// Component ID lookup functions for the live components written by the tuple.
    fn component_id_fns() -> Vec<ComponentIdFn>;

    /// Applies interpolation for one cached archetype that selected this rule.
    fn apply_archetype(
        world: UnsafeWorldCell,
        archetype: &Archetype,
        interpolation_registry: &InterpolationRegistry,
        rule_id: InterpolationRuleId,
        ctx: ApplyInterpolationContext,
        deferred_apply: &mut DeferredEntityCommands,
    );

    /// Applies frame interpolation for one cached archetype that selected this rule.
    fn apply_frame_archetype(
        world: UnsafeWorldCell,
        archetype: &Archetype,
        interpolation_registry: &InterpolationRegistry,
        rule_id: InterpolationRuleId,
        ctx: FrameInterpolationContext,
        skip_interpolation: bool,
        deferred_apply: &mut DeferredEntityCommands,
    );

    /// Adds per-component history rules for every component in the bundle.
    fn add_history_rules<F>(
        app: &mut App,
        config: InterpolationRuleConfig,
        include_interpolation_history: bool,
    ) where
        F: InterpolationRuleFilter + 'static;

    /// Registers the live component IDs written by bundle apply rules.
    fn register_live_components(app: &mut App);

    /// Marks every member component as interpolated in Lightyear's component registry.
    fn mark_interpolated(app: &mut App);
}

#[derive(Clone, Copy)]
enum ComponentTableColumn<'w, C> {
    Table(&'w [UnsafeCell<C>]),
    Missing,
    NonTable,
}

fn table_for_archetype<'w>(world: UnsafeWorldCell<'w>, archetype: &Archetype) -> Option<&'w Table> {
    unsafe { world.storages().tables.get(archetype.table_id()) }
}

fn component_table_column<'w, C: Component>(
    world: UnsafeWorldCell<'w>,
    archetype: &Archetype,
    table: &'w Table,
) -> ComponentTableColumn<'w, C> {
    let Some(component_id) = world.components().component_id::<C>() else {
        return ComponentTableColumn::Missing;
    };
    if !archetype.contains(component_id) {
        return ComponentTableColumn::Missing;
    }
    let Some(StorageType::Table) = archetype.get_storage_type(component_id) else {
        return ComponentTableColumn::NonTable;
    };
    unsafe {
        table
            .get_data_slice_for::<C>(component_id)
            .map_or(ComponentTableColumn::NonTable, ComponentTableColumn::Table)
    }
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
        impl<$C0, $($C),+> private::Sealed for ($C0, $($C,)+)
        where
            $C0: SyncComponent,
            $($C: SyncComponent),+
        {}

        impl<$C0, $($C),+> InterpolationBundle for ($C0, $($C,)+)
        where
            $C0: SyncComponent,
            $($C: SyncComponent),+
        {
            const COMPONENT_COUNT: usize = $N;

            fn add_rule<F>(
                app: &mut App,
                fns: InterpolationFns<Self>,
                config: InterpolationRuleConfig,
            )
            where
                F: InterpolationRuleFilter + 'static,
            {
                add_interpolation_bundle_rule::<Self, F>(app, fns, config);
            }
        }

        impl<$C0, $($C),+> TupleInterpolationBundle for ($C0, $($C,)+)
        where
            $C0: SyncComponent,
            $($C: SyncComponent),+
        {

            fn component_kinds() -> Vec<ComponentKind> {
                alloc::vec![ComponentKind::of::<$C0>(), $(ComponentKind::of::<$C>()),+]
            }

            fn component_id_fns() -> Vec<ComponentIdFn> {
                alloc::vec![
                    live_component_id::<$C0> as ComponentIdFn,
                    $(live_component_id::<$C> as ComponentIdFn),+
                ]
            }

            fn apply_archetype(
                world: UnsafeWorldCell,
                archetype: &Archetype,
                interpolation_registry: &InterpolationRegistry,
                rule_id: InterpolationRuleId,
                ctx: ApplyInterpolationContext,
                deferred_apply: &mut DeferredEntityCommands,
            ) {
                let Some(table) = table_for_archetype(world, archetype) else {
                    return;
                };
                let components = world.components();
                let Some($history0) = components.component_id::<ConfirmedHistory<$C0>>() else {
                    return;
                };
                let Some($history0) = (unsafe {
                    table.get_data_slice_for::<ConfirmedHistory<$C0>>($history0)
                }) else {
                    return;
                };
                let $component0 = component_table_column::<$C0>(world, archetype, table);
                $(
                    let Some($history) = components.component_id::<ConfirmedHistory<$C>>() else {
                        return;
                    };
                    let Some($history) = (unsafe {
                        table.get_data_slice_for::<ConfirmedHistory<$C>>($history)
                    }) else {
                        return;
                    };
                    let $component = component_table_column::<$C>(world, archetype, table);
                )+

                for entity in archetype.entities() {
                    let row = entity.table_row().index();
                    let $history0 = unsafe { &*$history0.get_unchecked(row).get() };
                    let Some(($start_tick0, $start0, $end0)) = ({
                        present_history_bracket($history0, ctx.interpolation_tick)
                    }) else {
                        continue;
                    };
                    $(
                        let $history = unsafe { &*$history.get_unchecked(row).get() };
                        let Some(($start_tick, $start, $end)) = ({
                            present_history_bracket($history, ctx.interpolation_tick)
                        }) else {
                            continue;
                        };
                    )+
                    if false $(|| $start_tick0 != $start_tick)+ {
                        continue;
                    }

                    let interpolated = match ($end0, $($end,)+) {
                        (
                            Some(($end_tick0, $end_value0)),
                            $(Some(($end_tick, $end_value)),)+
                        ) if true $(&& $end_tick0 == $end_tick)+ => {
                            let fraction = (((ctx.interpolation_tick - $start_tick0) as f32
                                + ctx.interpolation_overstep)
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
                        _ => continue,
                    };

                    let ($output0, $($output,)+) = interpolated;
                    match $component0 {
                        ComponentTableColumn::Table($component0) => {
                            let $component0 = unsafe { &mut *$component0.get_unchecked(row).get() };
                            *$component0 = $output0;
                        }
                        ComponentTableColumn::Missing => deferred_apply.insert(entity.id(), $output0),
                        ComponentTableColumn::NonTable => {}
                    }
                    $(
                        match $component {
                            ComponentTableColumn::Table($component) => {
                            let $component = unsafe { &mut *$component.get_unchecked(row).get() };
                                *$component = $output;
                            }
                            ComponentTableColumn::Missing => deferred_apply.insert(entity.id(), $output),
                            ComponentTableColumn::NonTable => {}
                        }
                    )+
                }
            }

            fn apply_frame_archetype(
                world: UnsafeWorldCell,
                archetype: &Archetype,
                interpolation_registry: &InterpolationRegistry,
                rule_id: InterpolationRuleId,
                ctx: FrameInterpolationContext,
                skip_interpolation: bool,
                deferred_apply: &mut DeferredEntityCommands,
            ) {
                let Some(table) = table_for_archetype(world, archetype) else {
                    return;
                };
                let components = world.components();
                let Some($history0) = components.component_id::<FrameInterpolationHistory<$C0>>() else {
                    return;
                };
                let Some($history0) = (unsafe {
                    table.get_data_slice_for::<FrameInterpolationHistory<$C0>>($history0)
                }) else {
                    return;
                };
                $(
                    let Some($history) = components.component_id::<FrameInterpolationHistory<$C>>() else {
                        return;
                    };
                    let Some($history) = (unsafe {
                        table.get_data_slice_for::<FrameInterpolationHistory<$C>>($history)
                    }) else {
                        return;
                    };
                )+

                let interpolation =
                    interpolation_registry.interpolation_for_rule::<($C0, $($C,)+)>(rule_id);
                for entity in archetype.entities() {
                    let row = entity.table_row().index();
                    let $history0 = unsafe { &mut *$history0.get_unchecked(row).get() };
                    let Some($end_value0) = $history0.current_value.clone() else {
                        continue;
                    };
                    $(
                        let $history = unsafe { &mut *$history.get_unchecked(row).get() };
                        let Some($end_value) = $history.current_value.clone() else {
                            continue;
                        };
                    )+

                    let interpolated = if skip_interpolation {
                        trace!(
                            target: "lightyear_debug::frame_interpolation",
                            kind = "frame_interpolation_skipped",
                            schedule = "PostUpdate",
                            sample_point = "PostUpdate",
                            component = ?DebugName::type_name::<($C0, $($C,)+)>(),
                            entity = ?entity.id(),
                            current_value_present = true,
                            "skipped frame interpolation"
                        );
                        $history0.previous_value = Some($end_value0.clone());
                        $(
                            $history.previous_value = Some($end_value.clone());
                        )+
                        ($end_value0, $($end_value,)+)
                    } else if let (Some($start0), $(Some($start),)+ Some(interpolation)) = (
                        $history0.previous_value.clone(),
                        $($history.previous_value.clone(),)+
                        interpolation,
                    ) {
                        interpolation(
                            ($start0, $($start,)+),
                            ($end_value0, $($end_value,)+),
                            ctx.overstep,
                        )
                    } else {
                        trace!(
                            component = ?DebugName::type_name::<($C0, $($C,)+)>(),
                            entity = ?entity.id(),
                            "No previous value, skipping visual interpolation"
                        );
                        ($end_value0, $($end_value,)+)
                    };

                    let ($output0, $($output,)+) = interpolated;
                    deferred_apply.insert(entity.id(), $output0);
                    $(
                        deferred_apply.insert(entity.id(), $output);
                    )+
                }
            }

            fn add_history_rules<F>(
                app: &mut App,
                config: InterpolationRuleConfig,
                include_interpolation_history: bool,
            )
            where
                F: InterpolationRuleFilter + 'static,
            {
                add_interpolation_rule::<$C0, F>(
                    app,
                    if include_interpolation_history {
                        InterpolationFns::history_only()
                    } else {
                        InterpolationFns::frame_history_only()
                    },
                    config,
                );
                $(
                    add_interpolation_rule::<$C, F>(
                        app,
                        if include_interpolation_history {
                            InterpolationFns::history_only()
                        } else {
                            InterpolationFns::frame_history_only()
                        },
                        config,
                    );
                )+
            }

            fn register_live_components(app: &mut App) {
                app.world_mut().register_component::<$C0>();
                $(app.world_mut().register_component::<$C>();)+
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
    &mut DeferredEntityCommands,
);

/// Context passed to type-erased frame interpolation apply functions.
#[derive(Debug, Clone, Copy)]
#[doc(hidden)]
pub struct FrameInterpolationContext {
    #[doc(hidden)]
    pub overstep: f32,
}

/// Type-erased function that updates one component's frame interpolation history.
#[doc(hidden)]
pub type ErasedUpdateFrameHistoryFn = fn(
    UnsafeWorldCell,
    &Archetype,
    &CachedFrameInterpolationComponent,
    &mut DeferredEntityCommands,
);

/// Type-erased function that restores one component from its frame interpolation history.
#[doc(hidden)]
pub type ErasedRestoreFrameHistoryFn =
    fn(UnsafeWorldCell, &Archetype, &CachedFrameInterpolationComponent);

/// Type-erased function that applies one selected frame interpolation rule.
#[doc(hidden)]
pub type ErasedApplyFrameInterpolationFn = fn(
    UnsafeWorldCell,
    &Archetype,
    &InterpolationRegistry,
    InterpolationRuleId,
    FrameInterpolationContext,
    bool,
    &mut DeferredEntityCommands,
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
    /// Whether a selected apply rule is responsible for inserting this live component.
    pub(crate) apply_controls_live_insert: bool,
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

    pub(crate) fn apply_controls_live_insert(&self) -> bool {
        self.apply_controls_live_insert
    }

    pub(crate) fn set_apply_controls_live_insert(&mut self) {
        self.apply_controls_live_insert = true;
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

/// Cached typed component metadata needed by frame interpolation history systems.
#[derive(Debug, Clone, Copy)]
pub struct CachedFrameInterpolationComponent {
    /// Component kind whose frame history is updated.
    pub(crate) kind: ComponentKind,
    /// Component ID for `FrameInterpolationHistory<C>`.
    pub(crate) history_component_id: ComponentId,
    /// Storage backing `FrameInterpolationHistory<C>` on the cached archetype.
    pub(crate) history_storage: Option<StorageType>,
    /// Whether the frame history component is present on the cached archetype.
    pub(crate) history_component_present: bool,
    /// Component ID for the live component `C`.
    pub(crate) live_component_id: ComponentId,
    /// Whether the live component `C` is present on the cached archetype.
    pub(crate) live_component_present: bool,
    /// Type-erased frame history update function for `C`.
    pub(crate) update_frame_history: ErasedUpdateFrameHistoryFn,
    /// Type-erased frame history restore function for `C`.
    pub(crate) restore_frame_history: ErasedRestoreFrameHistoryFn,
}

impl CachedFrameInterpolationComponent {
    #[doc(hidden)]
    pub fn kind(&self) -> ComponentKind {
        self.kind
    }

    #[doc(hidden)]
    pub fn history_component_id(&self) -> ComponentId {
        self.history_component_id
    }

    #[doc(hidden)]
    pub fn history_storage(&self) -> Option<StorageType> {
        self.history_storage
    }

    #[doc(hidden)]
    pub fn history_component_present(&self) -> bool {
        self.history_component_present
    }

    #[doc(hidden)]
    pub fn live_component_id(&self) -> ComponentId {
        self.live_component_id
    }

    #[doc(hidden)]
    pub fn live_component_present(&self) -> bool {
        self.live_component_present
    }

    #[doc(hidden)]
    pub fn update_frame_history(&self) -> ErasedUpdateFrameHistoryFn {
        self.update_frame_history
    }

    #[doc(hidden)]
    pub fn restore_frame_history(&self) -> ErasedRestoreFrameHistoryFn {
        self.restore_frame_history
    }
}

/// Cached type-erased apply metadata for one selected frame interpolation rule.
#[derive(Debug, Clone, Copy)]
pub struct CachedFrameInterpolationApply {
    /// ID of the selected rule whose interpolation function should run.
    pub(crate) rule_id: InterpolationRuleId,
    /// Type-erased function that writes this rule's live component(s).
    pub(crate) apply_frame_interpolation: ErasedApplyFrameInterpolationFn,
}

impl CachedFrameInterpolationApply {
    #[doc(hidden)]
    pub fn rule_id(&self) -> InterpolationRuleId {
        self.rule_id
    }

    #[doc(hidden)]
    pub fn apply_frame_interpolation(&self) -> ErasedApplyFrameInterpolationFn {
        self.apply_frame_interpolation
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ErasedInterpolationFns {
    pub(crate) interpolation: Option<ErasedLerpFn>,
    pub(crate) update_history: Option<ErasedUpdateHistoryFn>,
    pub(crate) apply_interpolation: Option<ErasedApplyInterpolationFn>,
    pub(crate) history_component_id: Option<ComponentIdFn>,
    pub(crate) live_component_id: ComponentIdFn,
    pub(crate) write_component_ids: Vec<ComponentIdFn>,
    pub(crate) frame: Option<FrameInterpolationFns>,
}

impl ErasedInterpolationFns {
    pub(crate) fn from_typed<S: 'static>(
        fns: InterpolationFns<S>,
        update_history: Option<ErasedUpdateHistoryFn>,
        apply_interpolation: Option<ErasedApplyInterpolationFn>,
        history_component_id: Option<ComponentIdFn>,
        live_component_id: ComponentIdFn,
        write_component_ids: Vec<ComponentIdFn>,
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

/// Type-erased functions and component access used by frame interpolation.
///
/// This is stored as a single optional value on [`ErasedInterpolationFns`] so
/// frame-only state is kept together. The presence of the history and apply
/// callbacks determines which frame phases the rule owns.
#[derive(Debug, Clone)]
pub(crate) struct FrameInterpolationFns {
    pub(crate) history_component_id: Option<ComponentIdFn>,
    pub(crate) write_component_ids: Vec<ComponentIdFn>,
    pub(crate) update_history: Option<ErasedUpdateFrameHistoryFn>,
    pub(crate) restore_history: Option<ErasedRestoreFrameHistoryFn>,
    pub(crate) apply_interpolation: Option<ErasedApplyFrameInterpolationFn>,
}

impl FrameInterpolationFns {
    pub(crate) fn new(
        history_component_id: Option<ComponentIdFn>,
        write_component_ids: Vec<ComponentIdFn>,
        update_history: Option<ErasedUpdateFrameHistoryFn>,
        restore_history: Option<ErasedRestoreFrameHistoryFn>,
        apply_interpolation: Option<ErasedApplyFrameInterpolationFn>,
    ) -> Option<Self> {
        (history_component_id.is_some()
            || !write_component_ids.is_empty()
            || update_history.is_some()
            || restore_history.is_some()
            || apply_interpolation.is_some())
        .then_some(Self {
            history_component_id,
            write_component_ids,
            update_history,
            restore_history,
            apply_interpolation,
        })
    }

    pub(crate) fn owns_history(&self) -> bool {
        self.history_component_id.is_some()
            && self.update_history.is_some()
            && self.restore_history.is_some()
    }

    pub(crate) fn applies_component(&self) -> bool {
        self.apply_interpolation.is_some()
    }
}

pub(crate) fn confirmed_history_component_id<C: Component + Clone>(
    components: &Components,
) -> Option<ComponentId> {
    components.component_id::<ConfirmedHistory<C>>()
}

pub(crate) fn frame_history_component_id<C: Component + Clone>(
    components: &Components,
) -> Option<ComponentId> {
    components.component_id::<FrameInterpolationHistory<C>>()
}

pub(crate) fn live_component_id<C: Component>(components: &Components) -> Option<ComponentId> {
    components.component_id::<C>()
}

pub(crate) fn no_component_id(_: &Components) -> Option<ComponentId> {
    None
}

#[doc(hidden)]
pub fn update_frame_history_archetype_erased<C: SyncComponent>(
    world: UnsafeWorldCell,
    archetype: &Archetype,
    component: &CachedFrameInterpolationComponent,
    deferred_apply: &mut DeferredEntityCommands,
) {
    if !component.live_component_present() {
        return;
    }
    let Some(StorageType::Table) = archetype.get_storage_type(component.live_component_id()) else {
        return;
    };
    let Some(table) = table_for_archetype(world, archetype) else {
        return;
    };
    let Some(live_components) =
        (unsafe { table.get_data_slice_for::<C>(component.live_component_id()) })
    else {
        return;
    };

    let histories = if component.history_component_present() {
        let Some(StorageType::Table) = component.history_storage() else {
            return;
        };
        let Some(histories) = (unsafe {
            table.get_data_slice_for::<FrameInterpolationHistory<C>>(
                component.history_component_id(),
            )
        }) else {
            return;
        };
        Some(histories)
    } else {
        None
    };

    for entity in archetype.entities() {
        let entity_id = entity.id();
        let row = entity.table_row().index();
        let component_value = unsafe { &*live_components.get_unchecked(row).get() };
        if let Some(histories) = histories {
            let history = unsafe { &mut *histories.get_unchecked(row).get() };
            if let Some(current_value) = history.current_value.take() {
                history.previous_value = Some(current_value);
            }
            history.current_value = Some(component_value.clone());
            trace!(
                target: "lightyear_debug::frame_interpolation",
                kind = "frame_interpolation_update_history",
                schedule = "FixedPostUpdate",
                sample_point = "FixedPostUpdate",
                component = ?DebugName::type_name::<C>(),
                "updated frame interpolation history"
            );
        } else {
            deferred_apply.insert(
                entity_id,
                FrameInterpolationHistory::<C> {
                    previous_value: None,
                    current_value: Some(component_value.clone()),
                },
            );
        }
    }
}

#[doc(hidden)]
pub fn restore_frame_history_archetype_erased<C: SyncComponent>(
    world: UnsafeWorldCell,
    archetype: &Archetype,
    component: &CachedFrameInterpolationComponent,
) {
    if !component.history_component_present() || !component.live_component_present() {
        return;
    }
    let Some(StorageType::Table) = component.history_storage() else {
        return;
    };
    let Some(StorageType::Table) = archetype.get_storage_type(component.live_component_id()) else {
        return;
    };
    let Some(table) = table_for_archetype(world, archetype) else {
        return;
    };
    let Some(histories) = (unsafe {
        table.get_data_slice_for::<FrameInterpolationHistory<C>>(component.history_component_id())
    }) else {
        return;
    };
    let Some(live_components) =
        (unsafe { table.get_data_slice_for::<C>(component.live_component_id()) })
    else {
        return;
    };

    for entity in archetype.entities() {
        let row = entity.table_row().index();
        let history = unsafe { &*histories.get_unchecked(row).get() };
        let Some(current_value) = &history.current_value else {
            continue;
        };
        let component = unsafe { &mut *live_components.get_unchecked(row).get() };
        trace!(
            target: "lightyear_debug::frame_interpolation",
            kind = "frame_interpolation_restore",
            schedule = "RunFixedMainLoop",
            sample_point = "RunFixedMainLoop",
            component = ?DebugName::type_name::<C>(),
            entity = ?entity.id(),
            "restored non-interpolated component value"
        );
        *component = current_value.clone();
    }
}

#[doc(hidden)]
pub fn apply_frame_interpolation_archetype_erased<C: SyncComponent>(
    world: UnsafeWorldCell,
    archetype: &Archetype,
    interpolation_registry: &InterpolationRegistry,
    rule_id: InterpolationRuleId,
    ctx: FrameInterpolationContext,
    skip_interpolation: bool,
    deferred_apply: &mut DeferredEntityCommands,
) {
    let Some(history_component_id) = world
        .components()
        .component_id::<FrameInterpolationHistory<C>>()
    else {
        return;
    };
    if !archetype.contains(history_component_id) {
        return;
    }
    let Some(StorageType::Table) = archetype.get_storage_type(history_component_id) else {
        return;
    };
    let Some(table) = table_for_archetype(world, archetype) else {
        return;
    };
    let Some(histories) =
        (unsafe { table.get_data_slice_for::<FrameInterpolationHistory<C>>(history_component_id) })
    else {
        return;
    };

    let interpolation = interpolation_registry.interpolation_for_rule::<C>(rule_id);
    for entity in archetype.entities() {
        let row = entity.table_row().index();
        let history = unsafe { &mut *histories.get_unchecked(row).get() };
        let Some(current_value) = history.current_value.clone() else {
            continue;
        };
        let interpolated = if skip_interpolation {
            trace!(
                target: "lightyear_debug::frame_interpolation",
                kind = "frame_interpolation_skipped",
                schedule = "PostUpdate",
                sample_point = "PostUpdate",
                component = ?DebugName::type_name::<C>(),
                entity = ?entity.id(),
                current_value_present = true,
                "skipped frame interpolation"
            );
            history.previous_value = Some(current_value.clone());
            current_value
        } else if let (Some(previous_value), Some(interpolation)) =
            (&history.previous_value, interpolation)
        {
            interpolation(previous_value.clone(), current_value, ctx.overstep)
        } else {
            trace!(
                component = ?DebugName::type_name::<C>(),
                entity = ?entity.id(),
                "No previous value, skipping visual interpolation"
            );
            current_value
        };
        trace!(
            target: "lightyear_debug::frame_interpolation",
            kind = "frame_interpolation_apply",
            schedule = "PostUpdate",
            sample_point = "PostUpdate",
            component = ?DebugName::type_name::<C>(),
            entity = ?entity.id(),
            overstep = ctx.overstep,
            "applied frame interpolation"
        );
        deferred_apply.insert(entity.id(), interpolated);
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
/// `members` it owns or writes, erased functions describing which phases
/// Lightyear runs, and an archetype filter. Rules are sorted by priority per
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
    /// Interpolation phases this rule can be selected for.
    pub(crate) phase_mask: RulePhaseMask,
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
    F: InterpolationRuleFilter + 'static,
{
    F::get_state(components)
        .is_some_and(|state| F::matches_component_set(&state, &|id| archetype.contains(id)))
}
