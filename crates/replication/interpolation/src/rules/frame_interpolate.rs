//! Frame-interpolation callbacks backed by shared interpolation rules.
//!
//! The frame-interpolation crate uses these erased helpers after independently
//! resolving history and apply ownership for entities with `FrameInterpolate`.

use crate::SyncComponent;
use crate::registry::InterpolationRegistry;
use crate::rules::{InterpolationRuleId, InterpolationSampleContext};
use bevy_ecs::archetype::Archetype;
use bevy_ecs::component::{ComponentId, StorageType};
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy_utils::prelude::DebugName;
use lightyear_core::ecs_utils::{
    table_component_slice, table_component_slice_if_table, table_for_archetype,
    write_component_with_change_detection,
};
use lightyear_core::prelude::FrameInterpolationHistory;
use lightyear_replication::deferred_entity::DeferredEntityCommands;
use lightyear_replication::registry::ComponentKind;
use lightyear_utils::ecs::get_component_unchecked;
use tracing::trace;

#[derive(Debug, Clone, Copy)]
#[doc(hidden)]
pub struct FrameInterpolationContext {
    #[doc(hidden)]
    pub overstep: f32,
    #[doc(hidden)]
    pub sample_delta_secs: Option<f32>,
}

/// Type-erased function that updates one component's frame interpolation history.
#[doc(hidden)]
pub type ErasedUpdateFrameHistoryFn = fn(
    UnsafeWorldCell,
    &Archetype,
    &CachedFrameInterpolationHistoryComponent,
    &mut DeferredEntityCommands,
);

/// Type-erased function that restores one component from its frame interpolation history.
#[doc(hidden)]
pub type ErasedRestoreFrameHistoryFn =
    fn(UnsafeWorldCell, &Archetype, &CachedFrameInterpolationHistoryComponent);

/// Type-erased function that applies one selected frame interpolation rule.
#[doc(hidden)]
pub type ErasedApplyFrameInterpolationFn = fn(
    UnsafeWorldCell,
    &Archetype,
    &InterpolationRegistry,
    InterpolationRuleId,
    FrameInterpolationContext,
    bool,
);

/// Cached typed component metadata needed by frame interpolation history systems.
///
/// The component IDs and type-erased callbacks are copied out of the selected
/// rule, so frame-history update and restore do not need the rule or its ID.
#[derive(Debug, Clone, Copy)]
pub struct CachedFrameInterpolationHistoryComponent {
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

impl CachedFrameInterpolationHistoryComponent {
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
///
/// Apply keeps the rule ID because the rule's typed interpolation function
/// remains stored in [`InterpolationRegistry`] and is retrieved at runtime.
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

/// Records each entity's live `C` into `FrameInterpolationHistory<C>`.
///
/// This runs after fixed updates so `current_value` is the latest fixed-tick
/// value and `previous_value` is the value from the prior fixed tick.
pub(crate) fn update_frame_history_archetype_erased<C: SyncComponent>(
    world: UnsafeWorldCell,
    archetype: &Archetype,
    component: &CachedFrameInterpolationHistoryComponent,
    deferred_apply: &mut DeferredEntityCommands,
) {
    if !component.live_component_present() {
        return;
    }
    let Some(table) = table_for_archetype(world, archetype) else {
        return;
    };
    let live_component_id = component.live_component_id();
    let live_component_storage = archetype.get_storage_type(live_component_id);
    let live_components =
        table_component_slice_if_table::<C>(table, live_component_id, live_component_storage);

    let histories = if component.history_component_present() {
        let Some(StorageType::Table) = component.history_storage() else {
            return;
        };
        let Some(histories) = table_component_slice::<FrameInterpolationHistory<C>>(
            table,
            component.history_component_id(),
        ) else {
            return;
        };
        Some(histories)
    } else {
        None
    };

    for entity in archetype.entities() {
        let entity_id = entity.id();
        let row = entity.table_row().index();
        let component_value = match live_component_storage {
            Some(StorageType::Table) => {
                let Some(live_components) = live_components else {
                    continue;
                };
                unsafe { &*live_components.get_unchecked(row).get() }.clone()
            }
            Some(StorageType::SparseSet) => {
                // SAFETY: this cached archetype contains `component_id`, and
                // the system param declares read access to all frame components.
                unsafe {
                    get_component_unchecked(
                        world,
                        entity,
                        archetype.table_id(),
                        StorageType::SparseSet,
                        live_component_id,
                    )
                    .deref::<C>()
                    .clone()
                }
            }
            None => continue,
        };
        if let Some(histories) = histories {
            let history = unsafe { &mut *histories.get_unchecked(row).get() };
            if let Some(current_value) = history.current_value.take() {
                history.previous_value = Some(current_value);
            }
            history.current_value = Some(component_value);
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
                    current_value: Some(component_value),
                },
            );
        }
    }
}

/// Restores live `C` from `FrameInterpolationHistory<C>::current_value`.
///
/// Frame interpolation temporarily writes visual values during `PostUpdate`.
/// Before the next fixed loop starts, this restores the authoritative fixed
/// value so simulation systems do not read interpolated visuals.
pub(crate) fn restore_frame_history_archetype_erased<C: SyncComponent>(
    world: UnsafeWorldCell,
    archetype: &Archetype,
    component: &CachedFrameInterpolationHistoryComponent,
) {
    if !component.history_component_present() || !component.live_component_present() {
        return;
    }
    let Some(StorageType::Table) = component.history_storage() else {
        return;
    };
    let Some(table) = table_for_archetype(world, archetype) else {
        return;
    };
    let Some(histories) = table_component_slice::<FrameInterpolationHistory<C>>(
        table,
        component.history_component_id(),
    ) else {
        return;
    };
    for entity in archetype.entities() {
        let row = entity.table_row().index();
        let history = unsafe { &*histories.get_unchecked(row).get() };
        let Some(current_value) = &history.current_value else {
            continue;
        };
        // SAFETY: this erased system declares write access to C, and the only
        // other live reference here is to FrameInterpolationHistory<C>.
        if !unsafe {
            write_component_with_change_detection::<C>(world, entity.id(), current_value.clone())
        } {
            continue;
        }
        trace!(
            target: "lightyear_debug::frame_interpolation",
            kind = "frame_interpolation_restore",
            schedule = "RunFixedMainLoop",
            sample_point = "RunFixedMainLoop",
            component = ?DebugName::type_name::<C>(),
            entity = ?entity.id(),
            "restored non-interpolated component value"
        );
    }
}

/// Applies the selected interpolation rule to live `C` for one archetype.
///
/// Apply ownership guarantees that `C` is present on this archetype.
pub(crate) fn apply_frame_interpolation_archetype_erased<C: SyncComponent>(
    world: UnsafeWorldCell,
    archetype: &Archetype,
    interpolation_registry: &InterpolationRegistry,
    rule_id: InterpolationRuleId,
    ctx: FrameInterpolationContext,
    skip_interpolation: bool,
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
        table_component_slice::<FrameInterpolationHistory<C>>(table, history_component_id)
    else {
        return;
    };
    let interpolation = interpolation_registry.interpolation_fn_for_rule::<C>(rule_id);
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
        } else if let Some(previous_value) = &history.previous_value {
            interpolation.interpolate(
                previous_value.clone(),
                current_value,
                InterpolationSampleContext::new(ctx.overstep, ctx.sample_delta_secs),
            )
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
        // SAFETY: apply ownership guarantees that this archetype contains C,
        // the erased system declares write access to C, and the only other live
        // reference here is to FrameInterpolationHistory<C>.
        let written =
            unsafe { write_component_with_change_detection::<C>(world, entity.id(), interpolated) };
        debug_assert!(
            written,
            "frame interpolation apply ownership requires every live component"
        );
    }
}
