//! Bundle interpolation support for tuple rules.
//!
//! This module contains the tuple trait and macro-generated implementations
//! for `(C1, C2, ...)` rules, keeping the single-component rule definitions in
//! the parent module.

use super::{
    ApplyInterpolationContext, ComponentTableColumn, InterpolationFns, InterpolationRuleConfig,
    InterpolationRuleId, component_table_column, table_for_archetype,
};
use crate::SyncComponent;
use crate::interpolate::present_history_bracket;
use crate::registry::{
    InterpolationRegistry, add_interpolation_bundle_rule, add_interpolation_rule, mark_interpolated,
};
use crate::rules::frame_interpolate::FrameInterpolationContext;
use alloc::vec::Vec;
use bevy_app::App;
use bevy_ecs::archetype::Archetype;
use bevy_ecs::component::ComponentId;
use bevy_ecs::query::QueryFilter;
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy_utils::prelude::DebugName;
use lightyear_core::prelude::{ConfirmedHistory, FrameInterpolationHistory};
use lightyear_replication::deferred_entity::DeferredEntityCommands;
use lightyear_replication::registry::ComponentKind;
use tracing::trace;

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
        F: QueryFilter + 'static;
}

mod private {
    pub trait Sealed {}
}

pub(crate) trait TupleInterpolationBundle: InterpolationBundle {
    /// Component kinds written by the tuple interpolation apply system.
    fn component_kinds() -> Vec<ComponentKind>;

    /// Registers and returns component IDs for the live components written by the tuple.
    fn component_ids(app: &mut App) -> Vec<ComponentId>;

    /// Applies interpolation for one cached archetype that selected this rule.
    fn apply_archetype(
        world: UnsafeWorldCell,
        archetype: &Archetype,
        interpolation_registry: &InterpolationRegistry,
        rule_id: InterpolationRuleId,
        ctx: ApplyInterpolationContext,
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
        F: QueryFilter + 'static;

    /// Marks every member component as interpolated in Lightyear's component registry.
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
                F: QueryFilter + 'static,
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

            fn component_ids(app: &mut App) -> Vec<ComponentId> {
                alloc::vec![
                    app.world_mut().register_component::<$C0>(),
                    $(app.world_mut().register_component::<$C>()),+
                ]
            }

            fn apply_archetype(
                world: UnsafeWorldCell,
                archetype: &Archetype,
                interpolation_registry: &InterpolationRegistry,
                rule_id: InterpolationRuleId,
                ctx: ApplyInterpolationContext,
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
                        ComponentTableColumn::Missing => {}
                        ComponentTableColumn::NonTable => {}
                    }
                    $(
                        match $component {
                            ComponentTableColumn::Table($component) => {
                                let $component = unsafe { &mut *$component.get_unchecked(row).get() };
                                *$component = $output;
                            }
                            ComponentTableColumn::Missing => {}
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
                let $component0 = component_table_column::<$C0>(world, archetype, table);
                $(
                    let $component = component_table_column::<$C>(world, archetype, table);
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

            fn add_history_rules<F>(
                app: &mut App,
                config: InterpolationRuleConfig,
                include_interpolation_history: bool,
            )
            where
                F: QueryFilter + 'static,
            {
                // Bundle rules store each member component in its own history.
                // For `no_history` bundle rules we still need synthetic
                // per-member frame-history rules so FrameInterpolate can reuse
                // the tuple interpolation function without also creating
                // delayed-interpolation `ConfirmedHistory<C>` state.
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
