use crate::_reexport::ReplicationSend;
use crate::prelude::ComponentRegistry;
use crate::protocol::{EventContext, Protocol};
use crate::shared::events::components::{
    ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent,
};
use crate::shared::events::connection::{
    IterComponentInsertEvent, IterComponentRemoveEvent, IterComponentUpdateEvent,
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
