use bevy_ecs::change_detection::Ref;
use bevy_ecs::prelude::{Added, Component, DetectChanges, Entity, ResMut};
use bevy_ecs::system::Query;

use crate::replication::{Replicate, ReplicationSend};
use crate::Protocol;

pub fn send_entity_spawn<P: Protocol>(
    // try doing entity spawn whenever replicate gets added
    query: Query<(Entity, &Replicate), Added<Replicate>>,
    mut sender: ResMut<dyn ReplicationSend<P>>,
) {
    query.iter().for_each(|(entity, replicate)| {
        sender.entity_spawn(entity, vec![], replicate).unwrap();
    })
}

pub fn send_component_update<C: Component + Clone, P: Protocol, R: ReplicationSend<P>>(
    // TODO: only update components that were not newly added
    query: Query<(Entity, Ref<C>, &Replicate)>,
    // TODO: do not use server or client/server here, but a generic class that can send/receive entity actions/updates
    //  we need some traits for stuff that is shared between server/client. Like replication functions
    mut sender: ResMut<R>,
) where
    <P as Protocol>::Components: From<C>,
{
    query.iter().for_each(|(entity, component, replicate)| {
        if component.is_added() {
            sender
                .component_insert(entity, component.clone().into(), replicate)
                .unwrap();
        }
        if component.is_changed() && !component.is_added() {
            sender
                .entity_update_single_component(entity, component.clone().into(), replicate)
                .unwrap();
        }
    })
}
