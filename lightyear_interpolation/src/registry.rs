use crate::SyncComponent;
use crate::interpolation_history::ConfirmedHistory;
use crate::plugin::{add_interpolation_systems, add_prepare_interpolation_systems};
use alloc::format;
use bevy_ecs::component::Mutable;
use bevy_ecs::{component::Component, resource::Resource};
use bevy_math::{
    Curve,
    curve::{Ease, EaseFunction, EasingCurve},
};
use bevy_platform::collections::HashMap;
use bevy_platform::collections::HashSet;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::postcard_utils;
use bevy_replicon::prelude::{AppMarkerExt, OpDeltaComponent, RepliconTick, RuleFns};
use bevy_replicon::shared::replication::deferred_entity::DeferredEntity;
use bevy_replicon::shared::replication::op_delta::{OpDeltaReceiver, OpDeltaWire};
use bevy_replicon::shared::replication::receive_markers::MarkerConfig;
use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, WriteCtx};
use bevy_utils::prelude::DebugName;
use lightyear_core::interpolation::Interpolated;
use lightyear_core::prelude::Tick;
use lightyear_replication::checkpoint::ReplicationCheckpointMap;
use lightyear_replication::registry::replication::ComponentRegistration;
use lightyear_replication::registry::{ComponentKind, ComponentRegistry, LerpFn};
use tracing::error;

fn lerp<C: Ease + Clone>(start: C, other: C, t: f32) -> C {
    let curve = EasingCurve::new(start, other, EaseFunction::Linear);
    curve.sample_unchecked(t)
}

#[derive(Debug, Clone)]
pub struct InterpolationMetadata {
    pub interpolation: Option<unsafe fn()>,
    pub custom_interpolation: bool,
}

#[derive(Resource, Debug, Default)]
pub struct InterpolationRegistry {
    pub(crate) interpolation_map: HashMap<ComponentKind, InterpolationMetadata>,
}

#[derive(Resource, Debug, Default)]
struct InterpolatedMarkerFnRegistry {
    kinds: HashSet<ComponentKind>,
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
}

fn register_interpolated_marker_fns<C: SyncComponent>(app: &mut bevy_app::App) {
    register_interpolated_marker_fns_with::<C>(
        app,
        write_history::<C>,
        remove_history::<C>,
        InterpolatedMarkerRegistration::InsertIfMissing,
    );
}

fn register_interpolated_marker_fns_op_delta<C: SyncComponent + OpDeltaComponent>(
    app: &mut bevy_app::App,
) {
    register_interpolated_marker_fns_with::<C>(
        app,
        write_history_op_delta::<C>,
        remove_history_op_delta::<C>,
        InterpolatedMarkerRegistration::ReplaceExisting,
    );
}

#[derive(Clone, Copy)]
enum InterpolatedMarkerRegistration {
    InsertIfMissing,
    ReplaceExisting,
}

fn register_interpolated_marker_fns_with<C: SyncComponent>(
    app: &mut bevy_app::App,
    write: fn(
        &mut WriteCtx,
        &RuleFns<C>,
        &mut DeferredEntity,
        &mut Bytes,
    ) -> bevy_ecs::error::Result<()>,
    remove: fn(&mut RemoveCtx, &mut DeferredEntity),
    registration: InterpolatedMarkerRegistration,
) {
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
    if already_registered
        && matches!(
            registration,
            InterpolatedMarkerRegistration::InsertIfMissing
        )
    {
        return;
    }
    if !already_registered {
        app.register_marker_with::<Interpolated>(MarkerConfig {
            priority: 100,
            need_history: true,
        });
    }
    app.set_marker_fns::<Interpolated, C>(write, remove);
    if !already_registered {
        app.world_mut()
            .resource_mut::<InterpolatedMarkerFnRegistry>()
            .kinds
            .insert(kind);
    }
}

fn resolve_message_tick(
    checkpoints: &ReplicationCheckpointMap,
    tick: RepliconTick,
) -> Option<Tick> {
    checkpoints.get(tick)
}

pub trait InterpolationRegistrationExt<C> {
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

    /// Enable interpolation systems for this component using the [`Ease`] implementation
    ///
    /// This will register interpolation systems to interpolate between two confirmed states.
    fn add_linear_interpolation(self) -> Self
    where
        C: SyncComponent + Ease;

    /// The remote updates will be stored in a [`ConfirmedHistory<C>`](crate::interpolation_history::ConfirmedHistory) component
    /// but the user has to define the interpolation logic themselves
    /// (`lightyear` won't perform any kind of interpolation)
    fn add_custom_interpolation(self) -> Self
    where
        C: SyncComponent;

    /// Like [`Self::add_custom_interpolation`], but for components replicated through
    /// Replicon's op-delta rule.
    fn add_custom_interpolation_op_delta(self) -> Self
    where
        C: SyncComponent + OpDeltaComponent;
}

impl<C> InterpolationRegistrationExt<C> for ComponentRegistration<'_, C> {
    fn register_interpolation_fn(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent,
    {
        if !self
            .app
            .world()
            .contains_resource::<InterpolationRegistry>()
        {
            self.app
                .world_mut()
                .insert_resource(InterpolationRegistry::default());
        }
        let mut registry = self.app.world_mut().resource_mut::<InterpolationRegistry>();
        registry.set_interpolation::<C>(interpolation_fn);
        self
    }

    fn register_linear_interpolation(self) -> Self
    where
        C: SyncComponent + Ease,
    {
        self.register_interpolation_fn(lerp::<C>)
    }

    fn add_interpolation_with(mut self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent,
    {
        self = self.register_interpolation_fn(interpolation_fn);
        register_interpolated_marker_fns::<C>(self.app);
        add_prepare_interpolation_systems::<C>(self.app);
        add_interpolation_systems::<C>(self.app);

        let mut registry = self.app.world_mut().resource_mut::<ComponentRegistry>();
        registry
            .component_metadata_map
            .get_mut(&ComponentKind::of::<C>())
            .unwrap()
            .replication
            .as_mut()
            .unwrap()
            .set_interpolated(true);
        self
    }

    fn add_linear_interpolation(self) -> Self
    where
        C: SyncComponent + Ease,
    {
        self.add_interpolation_with(lerp::<C>)
    }

    fn add_custom_interpolation(self) -> Self
    where
        C: SyncComponent,
    {
        let kind = ComponentKind::of::<C>();
        register_interpolated_marker_fns::<C>(self.app);
        if !self
            .app
            .world()
            .contains_resource::<InterpolationRegistry>()
        {
            self.app
                .world_mut()
                .insert_resource(InterpolationRegistry::default());
        }
        let mut registry = self.app.world_mut().resource_mut::<InterpolationRegistry>();
        registry
            .interpolation_map
            .entry(kind)
            .and_modify(|r| r.custom_interpolation = true)
            .or_insert_with(|| InterpolationMetadata {
                interpolation: None,
                custom_interpolation: true,
            });
        add_prepare_interpolation_systems::<C>(self.app);

        let mut registry = self.app.world_mut().resource_mut::<ComponentRegistry>();
        registry
            .component_metadata_map
            .get_mut(&ComponentKind::of::<C>())
            .unwrap()
            .replication
            .as_mut()
            .unwrap()
            .set_interpolated(true);
        self
    }

    fn add_custom_interpolation_op_delta(self) -> Self
    where
        C: SyncComponent + OpDeltaComponent,
    {
        let kind = ComponentKind::of::<C>();
        register_interpolated_marker_fns_op_delta::<C>(self.app);
        if !self
            .app
            .world()
            .contains_resource::<InterpolationRegistry>()
        {
            self.app
                .world_mut()
                .insert_resource(InterpolationRegistry::default());
        }
        let mut registry = self.app.world_mut().resource_mut::<InterpolationRegistry>();
        registry
            .interpolation_map
            .entry(kind)
            .and_modify(|r| r.custom_interpolation = true)
            .or_insert_with(|| InterpolationMetadata {
                interpolation: None,
                custom_interpolation: true,
            });
        add_prepare_interpolation_systems::<C>(self.app);

        let mut registry = self.app.world_mut().resource_mut::<ComponentRegistry>();
        registry
            .component_metadata_map
            .get_mut(&ComponentKind::of::<C>())
            .unwrap()
            .replication
            .as_mut()
            .unwrap()
            .set_interpolated(true);
        self
    }
}

// TODO: ideally we would update the LastConfirmedTick at this point?
/// Instead of writing into a component directly, it writes data into [`ConfirmedHistory<C>`].
fn write_history<C: Component<Mutability = Mutable>>(
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
    if let Some(mut history) = entity.get_mut::<ConfirmedHistory<C>>() {
        history.push(tick, component);
    } else {
        let mut history = ConfirmedHistory::<C>::default();
        history.push(tick, component);
        entity.insert(history);
    }
    Ok(())
}

fn write_history_op_delta<C: Component<Mutability = Mutable> + Clone + OpDeltaComponent>(
    ctx: &mut WriteCtx,
    _rule_fns: &RuleFns<C>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<()> {
    let Some(component) = op_delta_interpolation_component::<C>(entity, message)? else {
        return Ok(());
    };
    push_confirmed_history(ctx, entity, component)
}

fn op_delta_interpolation_component<
    C: Component<Mutability = Mutable> + Clone + OpDeltaComponent,
>(
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<Option<C>> {
    match postcard_utils::from_buf(message)? {
        OpDeltaWire::<C, C::Op>::Snapshot { cursor, value } => {
            entity.insert(OpDeltaReceiver::<C>::new(cursor));
            Ok(Some(value))
        }
        OpDeltaWire::<C, C::Op>::Ops { ops, .. } => {
            let ready_ops = {
                let mut receiver = entity.get_mut::<OpDeltaReceiver<C>>().ok_or_else(|| {
                    format!(
                        "received op-delta operations for `{}` before an interpolation snapshot",
                        DebugName::type_name::<C>()
                    )
                })?;
                receiver.queue_and_take_ready(ops)
            };
            if ready_ops.is_empty() {
                return Ok(None);
            }

            let mut value = entity
                .get::<ConfirmedHistory<C>>()
                .and_then(|history| history.newest())
                .map(|(_, value)| value.clone())
                .or_else(|| entity.get::<C>().map(|value| value.clone()))
                .ok_or_else(|| {
                    format!(
                        "received op-delta operations for `{}` without a confirmed interpolation base",
                        DebugName::type_name::<C>()
                    )
                })?;
            for op in ready_ops {
                value.apply_op(&op)?;
            }
            Ok(Some(value))
        }
    }
}

fn push_confirmed_history<C: Component<Mutability = Mutable>>(
    ctx: &mut WriteCtx,
    entity: &mut DeferredEntity,
    component: C,
) -> bevy_ecs::error::Result<()> {
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
    if let Some(mut history) = entity.get_mut::<ConfirmedHistory<C>>() {
        history.push(tick, component);
    } else {
        let mut history = ConfirmedHistory::<C>::default();
        history.push(tick, component);
        entity.insert(history);
    }
    Ok(())
}

/// Records a component removal in `ConfirmedHistory<C>`.
///
/// The live component is removed later by interpolation systems once the interpolation timeline
/// reaches the server tick that produced this removal.
fn remove_history<C: Component>(ctx: &mut RemoveCtx, entity: &mut DeferredEntity) {
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
        history.push_remove(tick);
    } else {
        let mut history = ConfirmedHistory::<C>::default();
        history.push_remove(tick);
        entity.insert(history);
    }
}

fn remove_history_op_delta<C: Component + OpDeltaComponent>(
    ctx: &mut RemoveCtx,
    entity: &mut DeferredEntity,
) {
    entity.remove::<OpDeltaReceiver<C>>();
    remove_history::<C>(ctx, entity);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_message_tick_uses_authoritative_tick_for_large_replicon_gap() {
        let mut checkpoints = ReplicationCheckpointMap::default();
        checkpoints.record(RepliconTick::new(200), Tick(20));

        assert_eq!(
            resolve_message_tick(&checkpoints, RepliconTick::new(200)),
            Some(Tick(20))
        );
    }

    #[test]
    fn resolve_message_tick_collapses_multiple_replicon_ticks_for_same_authoritative_tick() {
        let mut checkpoints = ReplicationCheckpointMap::default();
        checkpoints.record(RepliconTick::new(100), Tick(20));
        checkpoints.record(RepliconTick::new(101), Tick(20));

        assert_eq!(
            resolve_message_tick(&checkpoints, RepliconTick::new(100)),
            Some(Tick(20))
        );
        assert_eq!(
            resolve_message_tick(&checkpoints, RepliconTick::new(101)),
            Some(Tick(20))
        );
    }
}
