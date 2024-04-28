use crate::_internal::ReplicationSend;
use crate::prelude::ComponentRegistry;
use crate::protocol::EventContext;
use crate::shared::events::components::{
    ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent, EntityDespawnEvent,
    EntitySpawnEvent,
};
use crate::shared::events::connection::{
    IterComponentInsertEvent, IterComponentRemoveEvent, IterComponentUpdateEvent,
    IterEntityDespawnEvent, IterEntitySpawnEvent,
};
use bevy::prelude::{Component, EventWriter, Events, Res, ResMut, World};

/// System that gathers the replication events received by the local host and sends them to bevy Events
pub(crate) fn push_component_events<C: Component, R: ReplicationSend>(
    component_registry: Res<ComponentRegistry>,
    mut connection_manager: ResMut<R>,
    mut component_insert_events: EventWriter<ComponentInsertEvent<C, R::EventContext>>,
    mut component_remove_events: EventWriter<ComponentRemoveEvent<C, R::EventContext>>,
    mut component_update_events: EventWriter<ComponentUpdateEvent<C, R::EventContext>>,
) {
    component_insert_events.send_batch(
        connection_manager
            .events()
            .iter_component_insert::<C>(component_registry.as_ref())
            .map(|(entity, ctx)| ComponentInsertEvent::new(entity, ctx)),
    );
    component_remove_events.send_batch(
        connection_manager
            .events()
            .iter_component_remove::<C>(component_registry.as_ref())
            .map(|(entity, ctx)| ComponentRemoveEvent::new(entity, ctx)),
    );
    component_update_events.send_batch(
        connection_manager
            .events()
            .iter_component_update::<C>(component_registry.as_ref())
            .map(|(entity, ctx)| ComponentUpdateEvent::new(entity, ctx)),
    );
}

/// System that gathers the replication events received by the local host and sends them to bevy Events
pub(crate) fn push_entity_events<R: ReplicationSend>(
    mut connection_manager: ResMut<R>,
    mut entity_spawn_events: EventWriter<EntitySpawnEvent<R::EventContext>>,
    mut entity_despawn_events: EventWriter<EntityDespawnEvent<R::EventContext>>,
) {
    entity_spawn_events.send_batch(
        connection_manager
            .events()
            .into_iter_entity_spawn()
            .map(|(entity, ctx)| EntitySpawnEvent::new(entity, ctx)),
    );
    entity_despawn_events.send_batch(
        connection_manager
            .events()
            .into_iter_entity_despawn()
            .map(|(entity, ctx)| EntityDespawnEvent::new(entity, ctx)),
    );
}
