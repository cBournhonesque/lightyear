use crate::SyncComponent;
use crate::interpolation_history::ConfirmedHistory;
use crate::plugin::{add_interpolation_systems, add_prepare_interpolation_systems};
use bevy_ecs::component::Mutable;
use bevy_ecs::{component::Component, resource::Resource};
use bevy_math::{
    Curve,
    curve::{Ease, EaseFunction, EasingCurve},
};
use bevy_platform::collections::HashMap;
use bevy_platform::collections::HashSet;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::prelude::RepliconTick;
use bevy_replicon::prelude::{AppMarkerExt, RuleFns};
use bevy_replicon::shared::replication::deferred_entity::DeferredEntity;
use bevy_replicon::shared::replication::receive_markers::MarkerConfig;
use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, WriteCtx};
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
}

impl<C> InterpolationRegistrationExt<C> for ComponentRegistration<'_, C> {
    fn register_interpolation_fn(self, interpolation_fn: LerpFn<C>) -> Self
    where
        C: SyncComponent,
    {
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

/// Removes component `C`
fn remove_history<C: Component>(_ctx: &mut RemoveCtx, entity: &mut DeferredEntity) {
    // TODO: only remove once the tick is reached1
    entity.remove::<C>();
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
