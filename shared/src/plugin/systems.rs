use crate::replication::{Replicate, ReplicationSend};
use crate::Protocol;
use bevy_ecs::change_detection::Ref;
use bevy_ecs::prelude::{Component, DetectChanges, Entity, ResMut, With};
use bevy_ecs::system::Query;

pub fn send_component_update<C: Component + Clone, P: Protocol>(
    // TODO: only update components that were not newly added
    query: Query<(Entity, Ref<C>, &Replicate)>,
    // TODO: do not use server or client/server here, but a generic class that can send/receive entity actions/updates
    //  we need some traits for stuff that is shared between server/client. Like replication functions
    mut sender: ResMut<dyn ReplicationSend<P>>,
    // server: ResMut<Server<P>>,
) where
    <P as Protocol>::Components: From<C>,
{
    query.iter().for_each(|entity| {
        for (entity, component, replicate) in query.iter() {
            if component.is_changed() && !component.is_added() {
                sender
                    .entity_update(entity, vec![component.clone().into()], replicate)
                    .unwrap();
            }
            if component.is_added() {
                todo!("insert component");
            }
        }
    })
}
