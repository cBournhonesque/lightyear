use bevy_app::{App, Update};
use bevy_ecs::prelude::*;
use bevy_ecs::world::EntityWorldMut;
use bevy_replicon::prelude::RepliconTick;
use bevy_replicon::shared::replication::registry::{
    self, DespawnFn, ReplicationRegistry, ctx::DespawnCtx,
};
use lightyear_core::interpolation::Interpolated;
use lightyear_core::prelude::NetworkTimeline;
use lightyear_core::tick::Tick;
use lightyear_replication::checkpoint::ReplicationCheckpointMap;
use tracing::error;

use crate::plugin::InterpolationSystems;
use crate::timeline::InterpolationTimeline;

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DelayedInterpolatedDespawn {
    pub(crate) tick: Tick,
}

#[derive(Resource, Clone, Copy)]
struct ImmediateDespawnFn(DespawnFn);

pub(crate) fn configure_delayed_interpolated_despawn(app: &mut App) {
    app.init_resource::<ReplicationRegistry>();
    if app.world().contains_resource::<ImmediateDespawnFn>() {
        return;
    }

    let previous = {
        let mut registry = app.world_mut().resource_mut::<ReplicationRegistry>();
        let previous = registry.despawn;
        registry.despawn = delay_interpolated_despawn;
        previous
    };
    app.world_mut()
        .insert_resource(ImmediateDespawnFn(previous));
    app.add_systems(
        Update,
        despawn_interpolated_entities.in_set(InterpolationSystems::Prepare),
    );
}

fn immediate_despawn(ctx: &DespawnCtx, entity: EntityWorldMut) {
    let despawn = entity
        .world()
        .get_resource::<ImmediateDespawnFn>()
        .map(|despawn| despawn.0)
        .unwrap_or(registry::despawn);
    despawn(ctx, entity);
}

pub(crate) fn delay_interpolated_despawn(ctx: &DespawnCtx, mut entity: EntityWorldMut) {
    if !entity.contains::<Interpolated>() {
        immediate_despawn(ctx, entity);
        return;
    }

    let Some(tick) = resolve_despawn_tick(entity.world(), ctx.message_tick) else {
        error!(
            entity = ?entity.id(),
            message_tick = ?ctx.message_tick,
            "missing authoritative checkpoint mapping while delaying interpolated despawn"
        );
        debug_assert!(
            false,
            "missing authoritative checkpoint mapping while delaying interpolated despawn"
        );
        immediate_despawn(ctx, entity);
        return;
    };

    entity.insert(DelayedInterpolatedDespawn { tick });
}

fn resolve_despawn_tick(world: &World, message_tick: RepliconTick) -> Option<Tick> {
    world
        .get_resource::<ReplicationCheckpointMap>()
        .and_then(|checkpoints| checkpoints.get(message_tick))
}

pub(crate) fn despawn_interpolated_entities(
    interpolation: Single<&InterpolationTimeline>,
    query: Query<(Entity, &DelayedInterpolatedDespawn)>,
    mut commands: Commands,
) {
    let interpolation_tick = interpolation.now().tick();
    for (entity, despawn) in &query {
        if interpolation_tick >= despawn.tick {
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_app::App;
    use bevy_replicon::shared::replication::registry::test_fns::TestFnsEntityExt;
    use lightyear_core::time::TickInstant;

    fn setup_app(interpolation_tick: Tick) -> App {
        let mut app = App::new();
        app.world_mut()
            .insert_resource(ReplicationCheckpointMap::default());
        let mut timeline = InterpolationTimeline::default();
        timeline.set_now(TickInstant::from(interpolation_tick));
        app.world_mut().spawn(timeline);
        app.add_systems(Update, despawn_interpolated_entities);
        app
    }

    fn set_interpolation_tick(app: &mut App, tick: Tick) {
        let mut timelines = app.world_mut().query::<&mut InterpolationTimeline>();
        let mut timeline = timelines.single_mut(app.world_mut()).unwrap();
        timeline.set_now(TickInstant::from(tick));
    }

    #[test]
    fn interpolated_despawn_waits_until_interpolation_tick_reaches_server_tick() {
        let mut app = setup_app(Tick(9));
        let replicon_tick = RepliconTick::new(1);
        app.world_mut()
            .resource_mut::<ReplicationCheckpointMap>()
            .record(replicon_tick, Tick(10));
        configure_delayed_interpolated_despawn(&mut app);

        let entity = app.world_mut().spawn(Interpolated).id();
        app.world_mut()
            .entity_mut(entity)
            .apply_despawn(replicon_tick);

        assert!(app.world().get_entity(entity).is_ok());
        assert_eq!(
            app.world()
                .entity(entity)
                .get::<DelayedInterpolatedDespawn>(),
            Some(&DelayedInterpolatedDespawn { tick: Tick(10) })
        );

        app.update();
        assert!(app.world().get_entity(entity).is_ok());

        set_interpolation_tick(&mut app, Tick(10));
        app.update();
        assert!(app.world().get_entity(entity).is_err());
    }

    #[test]
    fn non_interpolated_despawn_is_immediate() {
        let mut app = setup_app(Tick(9));
        configure_delayed_interpolated_despawn(&mut app);
        let entity = app.world_mut().spawn_empty().id();

        app.world_mut()
            .entity_mut(entity)
            .apply_despawn(RepliconTick::new(1));

        assert!(app.world().get_entity(entity).is_err());
    }
}
