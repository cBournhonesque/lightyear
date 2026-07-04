//! Frame-interpolation callbacks backed by shared interpolation rules.
//!
//! The frame-interpolation crate uses these erased helpers when selecting the
//! highest-priority interpolation rule for entities with `FrameInterpolate`.

use crate::SyncComponent;
use crate::registry::InterpolationRegistry;
use crate::rules::{
    ComponentTableColumn, InterpolationRuleId, component_table_column, table_for_archetype,
};
use alloc::vec::Vec;
use bevy_ecs::archetype::Archetype;
use bevy_ecs::component::{ComponentId, StorageType};
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy_utils::prelude::DebugName;
use lightyear_core::prelude::FrameInterpolationHistory;
use lightyear_replication::deferred_entity::DeferredEntityCommands;
use lightyear_replication::registry::ComponentKind;
use tracing::trace;

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

/// Type-erased functions and component access used by frame interpolation.
///
/// This is stored as a single optional value on
/// [`crate::rules::ErasedInterpolationFns`] so frame-specific state is kept
/// together. The presence of the history and apply callbacks determines which
/// frame work the rule owns.
#[derive(Debug, Clone)]
pub(crate) struct FrameInterpolationFns {
    pub(crate) history_component_id: Option<ComponentId>,
    pub(crate) write_component_ids: Vec<ComponentId>,
    pub(crate) update_history: Option<ErasedUpdateFrameHistoryFn>,
    pub(crate) restore_history: Option<ErasedRestoreFrameHistoryFn>,
    pub(crate) apply_interpolation: Option<ErasedApplyFrameInterpolationFn>,
}

impl FrameInterpolationFns {
    pub(crate) fn new(
        history_component_id: Option<ComponentId>,
        write_component_ids: Vec<ComponentId>,
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
    let component = component_table_column::<C>(world, archetype, table);

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
        match component {
            ComponentTableColumn::Table(component) => {
                let component = unsafe { &mut *component.get_unchecked(row).get() };
                *component = interpolated;
            }
            ComponentTableColumn::Missing => deferred_apply.insert(entity.id(), interpolated),
            ComponentTableColumn::NonTable => {}
        }
    }
}
