//! Bundle interpolation support for tuple rules.
//!
//! This module contains the tuple trait and macro-generated implementations
//! for `(C1, C2, ...)` rules, keeping the single-component rule definitions in
//! the parent module.

use super::{
    ApplyInterpolationContext, InterpolationFns, InterpolationRuleConfig, InterpolationRuleId,
    InterpolationSampleContext,
};
use crate::SyncComponent;
use crate::interpolate::history_bracket;
use crate::registry::{
    InterpolationRegistry, add_interpolation_bundle_rule, interpolation_rule_member,
};
use crate::rules::InterpolationRuleComponent;
use crate::rules::frame_interpolate::FrameInterpolationContext;
use alloc::vec::Vec;
use bevy_app::App;
use bevy_ecs::archetype::Archetype;
use bevy_ecs::query::ArchetypeFilter;
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy_utils::prelude::DebugName;
use lightyear_core::ecs_utils::{table_for_archetype, write_component_with_change_detection};
use lightyear_core::history_buffer::HistoryState;
use lightyear_core::prelude::{ConfirmedHistory, FrameInterpolationHistory, Tick};
use tracing::trace;

/// Tuple of components that can be interpolated by one rule.
///
/// Tuple interpolation stores each component in its own history, samples every
/// history at shared ticks around the interpolation tick. Members without an
/// update at a shared tick carry their latest present value forward, so tuple
/// interpolation does not require identical per-component history entries.
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
        F: ArchetypeFilter + 'static;
}

mod private {
    pub trait Sealed {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PresentHistoryBracket {
    start_tick: Tick,
    end_tick: Option<Tick>,
}

impl PresentHistoryBracket {
    fn intersection(self, other: Self) -> Self {
        Self {
            start_tick: self.start_tick.max(other.start_tick),
            end_tick: match (self.end_tick, other.end_tick) {
                (Some(first), Some(second)) => Some(first.min(second)),
                (Some(tick), None) | (None, Some(tick)) => Some(tick),
                (None, None) => None,
            },
        }
    }
}

fn present_history_bracket<C>(
    history: &ConfirmedHistory<C>,
    interpolation_tick: Tick,
) -> Option<PresentHistoryBracket> {
    let bracket = history_bracket(history, interpolation_tick)?;
    bracket.start_state.value()?;
    Some(PresentHistoryBracket {
        start_tick: bracket.start_tick,
        end_tick: bracket.end.map(|(tick, _)| tick),
    })
}

/// Returns a present bundle endpoint value at `tick`.
///
/// If `tick` is a removal boundary, the preceding value is returned so the
/// bundle remains continuous until the presence pass removes the component.
fn present_history_value_at<C>(history: &ConfirmedHistory<C>, tick: Tick) -> Option<&C> {
    match history.get_state_at(tick) {
        Some(HistoryState::Updated(value)) => Some(value),
        Some(HistoryState::Removed) => history.get_state_before(tick)?.value(),
        None => history.get_state_at_or_before(tick)?.value(),
    }
}

pub(crate) trait TupleInterpolationBundle: InterpolationBundle {
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
    );

    /// Builds the self-contained component members stored by the bundle rule.
    fn rule_members(
        app: &mut App,
        include_interpolation_history: bool,
        include_frame_history: bool,
    ) -> Vec<InterpolationRuleComponent>;
}

macro_rules! impl_interpolation_bundle {
    (
        $N:tt,
        (
            $C0:ident,
            $history0:ident,
            $start0:ident,
            $end_value0:ident,
            $output0:ident
        ),
        $(
            (
                $C:ident,
                $history:ident,
                $start:ident,
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
                F: ArchetypeFilter + 'static,
            {
                add_interpolation_bundle_rule::<Self, F>(app, fns, config);
            }
        }

        impl<$C0, $($C),+> TupleInterpolationBundle for ($C0, $($C,)+)
        where
            $C0: SyncComponent,
            $($C: SyncComponent),+
        {
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
                $(
                    let Some($history) = components.component_id::<ConfirmedHistory<$C>>() else {
                        return;
                    };
                    let Some($history) = (unsafe {
                        table.get_data_slice_for::<ConfirmedHistory<$C>>($history)
                    }) else {
                        return;
                    };
                )+

                let interpolation =
                    interpolation_registry.interpolation_fn_for_rule::<($C0, $($C,)+)>(rule_id);
                for entity in archetype.entities() {
                    let row = entity.table_row().index();
                    let $history0 = unsafe { &*$history0.get_unchecked(row).get() };
                    let Some(mut shared_bracket) = ({
                        present_history_bracket($history0, ctx.interpolation_tick)
                    }) else {
                        continue;
                    };
                    $(
                        let $history = unsafe { &*$history.get_unchecked(row).get() };
                        shared_bracket = match present_history_bracket(
                            $history,
                            ctx.interpolation_tick,
                        ) {
                            Some(bracket) => shared_bracket.intersection(bracket),
                            None => continue,
                        };
                    )+
                    // Bundle members may have different immediate brackets
                    // when replication omits unchanged components. Use the
                    // intersection around the interpolation tick: the latest
                    // member start and the earliest available member end. A
                    // member without an entry at either shared tick carries
                    // its latest present value forward.
                    let Some($start0) = present_history_value_at(
                        $history0,
                        shared_bracket.start_tick,
                    ).cloned() else {
                        continue;
                    };
                    $(
                        let Some($start) = present_history_value_at(
                            $history,
                            shared_bracket.start_tick,
                        ).cloned() else {
                            continue;
                        };
                    )+

                    let interpolated = if let Some(shared_end_tick) = shared_bracket.end_tick {
                        let Some($end_value0) = present_history_value_at(
                            $history0,
                            shared_end_tick,
                        ).cloned() else {
                            continue;
                        };
                        $(
                            let Some($end_value) = present_history_value_at(
                                $history,
                                shared_end_tick,
                            ).cloned() else {
                                continue;
                            };
                        )+
                        interpolation.interpolate(
                            ($start0, $($start,)+),
                            ($end_value0, $($end_value,)+),
                            InterpolationSampleContext::from_ticks(
                                shared_bracket.start_tick,
                                shared_end_tick,
                                ctx.interpolation_tick,
                                ctx.interpolation_overstep,
                                ctx.tick_duration,
                            ),
                        )
                    } else {
                        ($start0, $($start,)+)
                    };

                    let ($output0, $($output,)+) = interpolated;
                    // SAFETY: the erased interpolation system declares write
                    // access to every bundle member, and no live-component
                    // references are held while these writes occur.
                    unsafe {
                        write_component_with_change_detection::<$C0>(
                            world,
                            entity.id(),
                            $output0,
                        );
                    }
                    $(
                        // SAFETY: same as for the first bundle member above.
                        unsafe {
                            write_component_with_change_detection::<$C>(
                                world,
                                entity.id(),
                                $output,
                            );
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
                    interpolation_registry.interpolation_fn_for_rule::<($C0, $($C,)+)>(rule_id);
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
                    } else if let (Some($start0), $(Some($start),)+) = (
                        $history0.previous_value.clone(),
                        $($history.previous_value.clone(),)+
                    ) {
                        interpolation.interpolate(
                            ($start0, $($start,)+),
                            ($end_value0, $($end_value,)+),
                            InterpolationSampleContext::new(ctx.overstep, ctx.sample_delta_secs),
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
                    // SAFETY: apply ownership guarantees that this archetype
                    // contains every bundle member, the erased system declares
                    // write access to them, and no references to their live
                    // values are held while these writes occur.
                    let written = unsafe {
                        write_component_with_change_detection::<$C0>(
                            world,
                            entity.id(),
                            $output0,
                        )
                    };
                    debug_assert!(
                        written,
                        "frame interpolation apply ownership requires every bundle member"
                    );
                    $(
                        // SAFETY: same as for the first bundle member above.
                        let written = unsafe {
                            write_component_with_change_detection::<$C>(
                                world,
                                entity.id(),
                                $output,
                            )
                        };
                        debug_assert!(
                            written,
                            "frame interpolation apply ownership requires every bundle member"
                        );
                    )+
                }
            }

            fn rule_members(
                app: &mut App,
                include_interpolation_history: bool,
                include_frame_history: bool,
            ) -> Vec<InterpolationRuleComponent> {
                alloc::vec![
                    interpolation_rule_member::<$C0>(
                        app,
                        include_interpolation_history,
                        include_frame_history,
                    ),
                    $(interpolation_rule_member::<$C>(
                        app,
                        include_interpolation_history,
                        include_frame_history,
                    )),+
                ]
            }
        }
    };
}

variadics_please::all_tuples_with_size!(
    impl_interpolation_bundle,
    2,
    8,
    C,
    history,
    start,
    end_value,
    output
);

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_ecs::prelude::Component;

    #[derive(Component, Clone, Debug, PartialEq)]
    struct Value(f32);

    #[derive(Clone, Copy)]
    enum Entry {
        Present(f32),
        Removed,
        Unchanged,
    }

    #[derive(Debug, PartialEq)]
    struct SampledBracket {
        bracket: PresentHistoryBracket,
        start: (Value, Value),
        end: Option<(Value, Value)>,
    }

    struct Case {
        name: &'static str,
        first: &'static [(u32, Entry)],
        second: &'static [(u32, Entry)],
        interpolation_tick: u32,
        expected: Option<SampledBracket>,
    }

    fn history(entries: &[(u32, Entry)]) -> ConfirmedHistory<Value> {
        let mut history = ConfirmedHistory::default();
        for &(tick, entry) in entries {
            match entry {
                Entry::Present(value) => history.insert_present(Tick(tick), Value(value)),
                Entry::Removed => history.insert_removed(Tick(tick)),
                Entry::Unchanged => {
                    assert!(history.add_unchanged(Tick(tick)));
                }
            }
        }
        history
    }

    fn sample_pair(
        first: &ConfirmedHistory<Value>,
        second: &ConfirmedHistory<Value>,
        interpolation_tick: Tick,
    ) -> Option<SampledBracket> {
        let bracket = present_history_bracket(first, interpolation_tick)?
            .intersection(present_history_bracket(second, interpolation_tick)?);
        let start = (
            present_history_value_at(first, bracket.start_tick)?.clone(),
            present_history_value_at(second, bracket.start_tick)?.clone(),
        );
        let end = match bracket.end_tick {
            Some(end_tick) => Some((
                present_history_value_at(first, end_tick)?.clone(),
                present_history_value_at(second, end_tick)?.clone(),
            )),
            None => None,
        };
        Some(SampledBracket {
            bracket,
            start,
            end,
        })
    }

    /// Checks shared bundle brackets across mismatched, unchanged, removed, and open histories.
    #[test]
    fn shared_present_history_bracket_cases() {
        let cases = [
            Case {
                name: "different immediate start and end ticks",
                first: &[(10, Entry::Present(1.0)), (30, Entry::Present(3.0))],
                second: &[(15, Entry::Present(10.0)), (20, Entry::Present(20.0))],
                interpolation_tick: 17,
                expected: Some(SampledBracket {
                    bracket: PresentHistoryBracket {
                        start_tick: Tick(15),
                        end_tick: Some(Tick(20)),
                    },
                    start: (Value(1.0), Value(10.0)),
                    end: Some((Value(1.0), Value(20.0))),
                }),
            },
            Case {
                name: "one member has no future entry",
                first: &[(10, Entry::Present(0.0)), (20, Entry::Present(10.0))],
                second: &[(10, Entry::Present(7.0))],
                interpolation_tick: 15,
                expected: Some(SampledBracket {
                    bracket: PresentHistoryBracket {
                        start_tick: Tick(10),
                        end_tick: Some(Tick(20)),
                    },
                    start: (Value(0.0), Value(7.0)),
                    end: Some((Value(10.0), Value(7.0))),
                }),
            },
            Case {
                name: "removal is a constant present endpoint",
                first: &[(10, Entry::Present(0.0)), (20, Entry::Present(10.0))],
                second: &[(10, Entry::Present(3.0)), (20, Entry::Removed)],
                interpolation_tick: 15,
                expected: Some(SampledBracket {
                    bracket: PresentHistoryBracket {
                        start_tick: Tick(10),
                        end_tick: Some(Tick(20)),
                    },
                    start: (Value(0.0), Value(3.0)),
                    end: Some((Value(10.0), Value(3.0))),
                }),
            },
            Case {
                name: "both members have no future entry",
                first: &[(10, Entry::Present(2.0))],
                second: &[(5, Entry::Present(4.0))],
                interpolation_tick: 15,
                expected: Some(SampledBracket {
                    bracket: PresentHistoryBracket {
                        start_tick: Tick(10),
                        end_tick: None,
                    },
                    start: (Value(2.0), Value(4.0)),
                    end: None,
                }),
            },
            Case {
                name: "member is removed at the interpolation tick",
                first: &[(10, Entry::Present(0.0)), (20, Entry::Present(10.0))],
                second: &[(10, Entry::Present(3.0)), (20, Entry::Removed)],
                interpolation_tick: 20,
                expected: None,
            },
            Case {
                name: "member is present again at its reinsertion tick",
                first: &[(10, Entry::Present(0.0)), (30, Entry::Present(20.0))],
                second: &[
                    (10, Entry::Removed),
                    (20, Entry::Present(4.0)),
                    (30, Entry::Present(8.0)),
                ],
                interpolation_tick: 20,
                expected: Some(SampledBracket {
                    bracket: PresentHistoryBracket {
                        start_tick: Tick(20),
                        end_tick: Some(Tick(30)),
                    },
                    start: (Value(0.0), Value(4.0)),
                    end: Some((Value(20.0), Value(8.0))),
                }),
            },
            Case {
                name: "unchanged member carries across the other member's endpoint",
                first: &[(10, Entry::Present(3.0)), (20, Entry::Unchanged)],
                second: &[
                    (10, Entry::Present(0.0)),
                    (15, Entry::Present(10.0)),
                    (20, Entry::Unchanged),
                ],
                interpolation_tick: 12,
                expected: Some(SampledBracket {
                    bracket: PresentHistoryBracket {
                        start_tick: Tick(10),
                        end_tick: Some(Tick(15)),
                    },
                    start: (Value(3.0), Value(0.0)),
                    end: Some((Value(3.0), Value(10.0))),
                }),
            },
        ];

        for case in cases {
            assert_eq!(
                sample_pair(
                    &history(case.first),
                    &history(case.second),
                    Tick(case.interpolation_tick),
                ),
                case.expected,
                "{}",
                case.name,
            );
        }
    }
}
